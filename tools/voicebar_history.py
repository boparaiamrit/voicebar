"""Build a local aggregate writing profile. No messages are saved or transmitted."""

import argparse
from collections import Counter, defaultdict
from contextlib import closing, contextmanager
import hashlib
import io
import json
import os
from pathlib import Path
import re
import sqlite3
import sys
import tempfile
import unicodedata
import zipfile


MAX_LINE = 8 * 1024 * 1024
MAX_EXPORT_ITEM = 128 * 1024 * 1024
EXCLUDED_PREFIXES = (
    "# agents.md instructions", "<environment_context", "<environment_details",
    "<local-command", "<system-reminder", "<command-", "<session_context",
    "<task-notification", "<user_instructions", "<subagent_notification",
    "<ide_opened_file", "<user_shell_command", "[request interrupted",
    "this session is being continued from a previous conversation",
    "you are a newly spawned", "<teammate-message", "<hook", "<permissions instructions",
    "<suggested_answers", "<suggested-answers", "<user_input_response", "<app-context",
    "<codex_apps_client_time_context", "<system_instruction", "<agent_context",
)
FENCES = re.compile(r"^`{3,}|^~{3,}")
DIRECTIVES = {
    "explicit_concise_wording": (
        r"\b(?:keep (?:my|the|this) (?:wording|message|rewrite) (?:short|concise)|"
        r"(?:make|rewrite) (?:my|this) (?:message|wording) (?:more concise|shorter))\b",
        "When polishing, favor concise wording while retaining the speaker's requirements.",
    ),
    "explicit_keep_my_words": (
        r"\b(?:use|keep|preserve) my (?:own )?(?:words|wording|voice|tone)\b",
        "Retain the speaker's own words and tone when their meaning is already clear.",
    ),
    "explicit_plain_language": (
        r"\b(?:rewrite (?:this|it)|(?:say|write) (?:this|it)) in (?:plain|simple) (?:english|language)\b",
        "When polishing English, favor plain language.",
    ),
    "can_you_framing": (
        r"^(?:please\s+)?(?:can|could) you\b",
        "Keep a request phrased as a question when the speaker opens with 'can you' or 'could you'.",
    ),
    "first_person_want_or_need": (
        r"\bi (?:want|need)(?: you)? to\b",
        "Keep first-person goals in the first person when the speaker says what they want or need.",
    ),
    "make_sure": (
        r"\bmake sure\b",
        "Keep explicit checks and conditions when the speaker says 'make sure'.",
    ),
}
PATTERNS = {key: re.compile(pat) for key, (pat, _) in DIRECTIVES.items()}
OBSERVATIONS = {
    "requests_verification": re.compile(r"\b(?:verify|double[- ]check|check (?:if|that|whether))\b"),
    "requests_rephrasing": re.compile(r"\b(?:rephrase|rewrite|reword)\b"),
    "requests_translation": re.compile(r"\btranslate\b"),
    "spoken_hesitation_markers": re.compile(r"\b(?:uh|um|umm|uhh)\b"),
}
ALL_FEATURES = PATTERNS | OBSERVATIONS
# Every allowed spelling here is public. Never persist a mined free-text token.
PUBLIC_TERMS = ("Claude Code", "Codex", "ChatGPT", "Gemini", "VoiceBar", "Ollama",
                "Qwen", "LoRA", "GitHub", "TypeScript", "Rust", "PowerShell", "QEMU",
                "Docker", "WSL", "Vulkan", "PostgreSQL", "React", "Next.js", "CUDA",
                "Windows", "macOS", "Linux", "Python", "OpenAI", "FreeCAD", "Fusion 360",
                "Parakeet", "Nemotron", "GitHub Copilot", "OpenCode", "Cursor")
TERM_PATTERNS = {term: re.compile(r"(?<!\w)" + re.escape(term.casefold()) + r"(?!\w)")
                 for term in PUBLIC_TERMS}


def text_parts(value, allowed=("text", "input_text")):
    if isinstance(value, str):
        return value
    if not isinstance(value, list):
        return ""
    return "\n".join(
        part if isinstance(part, str) else part["text"]
        for part in value
        if isinstance(part, str) or (
            isinstance(part, dict) and part.get("type", "text") in allowed
            and isinstance(part.get("text"), str)
        )
    )


def clean_prose(message):
    """Ignore literal code, quoted material, and common generated user-role envelopes."""
    message = unicodedata.normalize("NFKC", message).strip()
    if not message or message.casefold().startswith(EXCLUDED_PREFIXES):
        return ""
    lines = []
    fence = None
    for line in message.splitlines():
        line = line.strip()
        if match := FENCES.match(line):
            token = match.group()[0]
            if fence is None:
                fence = token
            elif token == fence:
                fence = None
            continue
        if not fence and line and not line.startswith((">", "<", "{", "[")):
            lines.append(re.sub(r"`[^`]+`", "", line))
    return " ".join(" ".join(lines).casefold().split())


def read_lines(stream, issues):
    while raw := stream.readline(MAX_LINE + 1):
        if len(raw) > MAX_LINE:
            while raw and not raw.endswith(b"\n"):
                raw = stream.readline(MAX_LINE + 1)
            issues["oversize_records"] += 1
            continue
        try:
            obj = json.loads(raw)
            if isinstance(obj, dict):
                yield obj
        except (ValueError, UnicodeError, RecursionError):
            issues["malformed_records"] += 1


@contextmanager
def open_session(path):
    if path.suffix == ".zst":
        try:
            from compression import zstd  # Python 3.14+
        except ImportError:
            try:
                import zstandard as zstd  # optional on older Python
            except ImportError:
                raise ValueError("zstd_unavailable") from None
        with zstd.open(path, "rb") as stream:
            yield stream
    else:
        with path.open("rb") as stream:
            yield stream


def codex_turns(rows):
    for row in rows:
        kind = row.get("type")
        msg = row.get("payload")
        if not isinstance(msg, dict):
            continue
        if kind == "session_meta":
            source = msg.get("source")
            if (isinstance(source, dict) and "subagent" in source) or msg.get("agent_path") not in (None, "/root"):
                return
        if kind == "event_msg" and msg.get("type") == "user_message":
            yield msg.get("message")
        if kind == "response_item" and msg.get("role") == "user" and msg.get("type") == "message":
            parts = msg.get("content", [])
            metadata = msg.get("internal_chat_message_metadata_passthrough") or {}
            kinds = metadata.get("content_item_kinds") or []
            sources = metadata.get("content_item_metadata") or []
            if kinds or sources:
                accepted = []
                for index, part in enumerate(parts):
                    key = kinds[index] if index < len(kinds) else None
                    item_meta = sources[index] if index < len(sources) else {}
                    provenance = ((item_meta or {}).get("provenance") or {}).get("type")
                    if (key in ("user.text", None) and provenance in ("user", None)
                            and (key == "user.text" or provenance == "user")):
                        accepted.append(part)
                parts = accepted
            yield text_parts(parts)


def claude_turns(rows):
    for row in rows:
        if (row.get("type") == "user" and not row.get("isMeta") and not row.get("isSidechain")
                and row.get("turnOrigin", "human") == "human" and not row.get("sourceToolAssistantUUID")):
            msg = row.get("message", {})
            if msg.get("role") == "user":
                yield text_parts(msg.get("content"))


def gemini_turns(rows):
    for row in rows:
        metadata = row.get("$set", {})
        if row.get("kind") == "subagent" or metadata.get("kind") == "subagent":
            return
        records = metadata.get("messages", row.get("messages", [row]))
        for msg in records:
            if isinstance(msg, dict) and msg.get("type") == "user":
                yield text_parts(msg.get("content"))


def json_items(stream):
    """Read each top-level array item separately, or a single JSON object."""
    decoder = json.JSONDecoder()
    with io.TextIOWrapper(stream, encoding="utf-8-sig") as reader:
        buf = reader.read(65536).lstrip()
        if not buf:
            return
        array = buf.startswith("[")
        if array:
            buf = buf[1:]
        while True:
            buf = buf.lstrip()
            if not buf:
                buf = reader.read(65536).lstrip()
                if not buf:
                    if array:
                        raise ValueError("truncated_export")
                    return
            if array and buf.startswith("]"):
                return
            try:
                item, end = decoder.raw_decode(buf)
            except json.JSONDecodeError:
                if len(buf) >= MAX_EXPORT_ITEM:
                    raise ValueError("oversize_export_item")
                more = reader.read(min(max(len(buf), 65536), MAX_EXPORT_ITEM - len(buf)))
                if not more:
                    raise
                buf += more
                continue
            if isinstance(item, dict):
                yield item
            buf = buf[end:].lstrip()
            if not array:
                if buf or reader.read(1).strip():
                    raise ValueError("invalid_export")
                return
            while not buf:
                more = reader.read(65536)
                if not more:
                    raise ValueError("truncated_export")
                buf = more.lstrip()
            if buf.startswith("]"):
                return
            if not buf.startswith(","):
                raise ValueError("invalid_export")
            buf = buf[1:]


def export_turns(provider, stream):
    found_format = False
    for record in json_items(stream):
        if provider == "ChatGPT export" and isinstance(record.get("mapping"), dict):
            found_format = True
            for node in record["mapping"].values():
                msg = (node.get("message") or {}) if isinstance(node, dict) else {}
                if msg.get("author", {}).get("role") == "user" and not (
                    msg.get("metadata") or {}
                ).get("is_visually_hidden_from_conversation"):
                    yield text_parts(msg.get("content", {}).get("parts"))
        elif provider == "Claude export" and isinstance(record.get("chat_messages"), list):
            found_format = True
            for msg in record["chat_messages"]:
                if msg.get("sender") == "human":
                    yield msg.get("text") or text_parts(msg.get("content"))
        elif provider == "Gemini export" and "title" in record and "header" in record:
            found_format = True
            title = record.get("title", "")
            if record.get("header") in ("Gemini Apps", "Bard", "Gemini") and title.startswith("Prompted "):
                yield title.removeprefix("Prompted ")
        elif provider == "Gemini export" and isinstance(record.get("messages"), list):
            found_format = True
            yield from gemini_turns([record])
    if not found_format:
        raise ValueError("unrecognized_export_schema")


def export_name(provider, name):
    leaf = Path(name).name.lower()
    if provider == "Gemini export":
        return leaf == "myactivity.json" and any(s in name.lower() for s in ("gemini", "bard"))
    return bool(re.fullmatch(r"conversations(?:[-_]\d+)?\.json", leaf))


def codex_desktop_turns(path):
    with closing(sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True)) as db:
        state = path.parent / "state_5.sqlite"
        # Only a known, top-level conversation is sufficient provenance.
        db.execute("ATTACH DATABASE ? AS state", (state.resolve().as_uri() + "?mode=ro",))
        query = """SELECT item_json FROM thread_items i JOIN state.threads t ON t.id=i.thread_id
                   WHERE i.item_type='userMessage' AND (t.agent_path IS NULL OR t.agent_path='/root')
                   AND (t.source IS NULL OR t.source NOT LIKE '%subagent%')"""
        for (raw,) in db.execute(query):
            msg = json.loads(raw)
            if msg.get("type") == "userMessage":
                yield text_parts(msg.get("content"))


def protobuf_fields(data):
    """Read standard wire fields without deserializing unrelated context or blobs."""
    pos = 0

    def varint():
        nonlocal pos
        value = 0
        for shift in range(0, 70, 7):
            if pos >= len(data):
                break
            byte = data[pos]
            pos += 1
            value |= (byte & 127) << shift
            if byte < 128:
                return value
        raise ValueError("invalid_wire_format")

    while pos < len(data):
        number, wire = divmod(varint(), 8)
        if not number:
            raise ValueError("invalid_wire_format")
        if wire == 0:
            yield number, varint()
        elif wire in (1, 2, 5):
            size = varint() if wire == 2 else 8 if wire == 1 else 4
            end = pos + size
            if end > len(data):
                raise ValueError("invalid_wire_format")
            if wire == 2:
                yield number, data[pos:end]
            pos = end
        else:
            raise ValueError("invalid_wire_format")


def antigravity_turns(path):
    # The bundled gemini_coder.Step descriptor defines USER_INPUT=14 and
    # user_input=19. CortexStepUserInput.items=3, TextOrScopeItem.text=1.
    # This excludes context attachments, tool output, and generated responses.
    if path.suffix == ".pb":
        raise ValueError("unsupported_legacy_container")
    with closing(sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True)) as db:
        if not db.execute("SELECT 1 FROM trajectory_meta WHERE trajectory_type IN (1,2,4,17) "
                          "AND source IN (1,12,17) LIMIT 1").fetchone():
            return
        for (raw,) in db.execute("SELECT step_payload FROM steps WHERE step_type=14 AND step_format=0 ORDER BY idx"):
            fields = list(protobuf_fields(raw))
            if (1, 14) not in fields:
                continue
            for key, content in fields:
                if key == 19 and isinstance(content, bytes):
                    user_items = [item for k, item in protobuf_fields(content) if k == 3 and isinstance(item, bytes)]
                    chunks = [text.decode("utf-8") for item in user_items
                              for k, text in protobuf_fields(item) if k == 1 and isinstance(text, bytes)]
                    if chunks:
                        yield "".join(chunks)


def read_source(source, path, issues):
    if source == "Antigravity voice":
        with path.open("rb") as stream:
            for record in json_items(stream):
                for chat in record.get("chats", []):
                    for line in chat.get("lines", []):
                        if line.get("role") == "ROLE_USER":
                            yield line.get("text")
        return
    if source == "Antigravity":
        yield from antigravity_turns(path)
        return
    if source == "Codex" and path.suffix == ".sqlite":
        yield from codex_desktop_turns(path)
        return
    if source.endswith(" export"):
        if path.suffix.lower() == ".zip":
            with zipfile.ZipFile(path) as archive:
                members = [entry for entry in archive.infolist() if export_name(source, entry.filename)]
                if not members:
                    raise ValueError("missing_export_member")
                for member in members:
                    with archive.open(member) as stream:
                        yield from export_turns(source, stream)
        else:
            with path.open("rb") as stream:
                yield from export_turns(source, stream)
        return
    if source == "Codex dictation" and path.suffix == ".json":
        with path.open("rb") as stream:
            for row in json_items(stream):
                yield row.get("text")
        return
    if source not in ("Codex", "Codex archive", "Codex dictation", "Claude Code", "Gemini CLI"):
        from voicebar_history_extra import extra_turns
        yield from extra_turns(source, path, issues)
        return
    with open_session(path) as stream:
        if path.suffix == ".json":
            yield from gemini_turns(json_items(stream))
            return
        rows = read_lines(stream, issues)
        if source == "Codex dictation":
            for row in rows:
                yield row.get("text")
        elif path.name == "history.jsonl":
            for row in rows:
                yield row.get("display" if source == "Claude Code" else "text")
        elif source == "Claude Code":
            yield from claude_turns(rows)
        elif source == "Gemini CLI":
            yield from gemini_turns(rows)
        else:
            yield from codex_turns(rows)


def discover(home):
    codex = Path(os.environ.get("CODEX_HOME") or home / ".codex")
    claude = Path(os.environ.get("CLAUDE_CONFIG_DIR") or home / ".claude")
    yield "Codex", codex / "history.jsonl"
    yield "Codex", codex / "thread_history_1.sqlite"
    yield "Claude Code", claude / "history.jsonl"
    yield "Codex dictation", codex / "transcription-history.jsonl"
    for label, root, pattern in [
        ("Codex", codex / "sessions", "**/rollout-*.jsonl*"),
        ("Codex archive", codex / "archived_sessions", "**/rollout-*.jsonl*"),
        ("Codex dictation", codex / "dictation-history", "*/*.json"),
        ("Claude Code", claude / "projects", "*/*.jsonl"),
        ("Gemini CLI", home / ".gemini" / "tmp", "*/chats/session-*.json*"),
        ("Antigravity", home / ".gemini/antigravity/conversations", "*"),
        ("Antigravity", home / ".gemini/antigravity-ide/conversations", "*"),
        ("Antigravity voice", home / ".gemini/antigravity/voice_chats", "*.json"),
    ]:
        if not root.is_dir():
            yield label, root
            continue
        for path in sorted(root.glob(pattern)):
            if path.suffix in (".json", ".jsonl", ".zst", ".db", ".pb"):
                yield label, path
    from voicebar_history_extra import extra_sources
    yield from extra_sources(home)


def profile(inputs):
    sources = {}
    seen = set()
    by_provider = defaultdict(set)
    features = Counter()
    vocabulary = Counter()
    scanned = set()
    for source, path in inputs:
        stats = sources.setdefault(source, {
            "files_read": 0, "files_unavailable": 0, "user_text_records": 0,
            "files_with_errors": 0, "files_skipped": 0, "issues": Counter(),
        })
        key = (source, path.resolve())
        if key in scanned:
            continue
        scanned.add(key)
        if not path.is_file():
            stats["files_unavailable"] += 1
            continue
        if source == "Antigravity" and path.suffix == ".pb":
            stats["files_skipped"] += 1
            stats["issues"]["unsupported_legacy_container"] += 1
            continue
        try:
            for turn in read_source(source, path, stats["issues"]):
                if not isinstance(turn, str) or not turn.strip():
                    continue
                prose = clean_prose(turn)
                if not prose:
                    stats["issues"]["filtered_envelopes_or_nonprose"] += 1
                    continue
                # Deduplicate actual normalized turns, independent of provider/format.
                norm = " ".join(unicodedata.normalize("NFKC", turn).casefold().split())
                digest = hashlib.sha256(norm.encode("utf-8", "surrogatepass")).digest()
                stats["user_text_records"] += 1
                by_provider[source].add(digest)
                if digest in seen:
                    continue
                seen.add(digest)
                for name, pattern in ALL_FEATURES.items():
                    if pattern.search(prose):
                        features[name] += 1
                for name, pattern in TERM_PATTERNS.items():
                    if pattern.search(prose):
                        vocabulary[name] += 1
            stats["files_read"] += 1
        except Exception as exc:
            # Report categories only. Exceptions can contain prompts, paths or SQL.
            known = ("zstd_unavailable", "truncated_export", "oversize_export_item",
                     "invalid_export", "unrecognized_export_schema", "missing_export_member",
                     "invalid_wire_format", "unsupported_legacy_container",
                     "oversize_history_file")
            code = exc.args[0] if isinstance(exc, ValueError) and exc.args and exc.args[0] in known else type(exc).__name__
            stats["issues"][code] += 1
            stats["files_with_errors"] += 1
    rules = [rule for key, (_, rule) in DIRECTIVES.items() if features[key] >= 3]
    terms = dict((term, count) for term, count in sorted(vocabulary.items(), key=lambda x: (-x[1],x[0]))
                 if count >= 3)
    return {"schema_version": 1, "unique_prompts": len(seen),
            "provider_counts": {source: len(by_provider[source]) for source in sources},
            "sources": sources, "feature_counts": dict(features), "style_rules": rules,
            "vocabulary": list(terms)[:12], "vocabulary_counts": {k: terms[k] for k in list(terms)[:12]},
            "approved_pairs": 0}


def write_report(path, report):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent,
                                     prefix=".voicebar-profile-", delete=False) as tmp:
        temp_path = Path(tmp.name)
        try:
            json.dump(report, tmp, indent=2, ensure_ascii=False)
            tmp.write("\n")
        except BaseException:
            tmp.close()
            temp_path.unlink(missing_ok=True)
            raise
    try:
        os.replace(temp_path, path)
    finally:
        temp_path.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--no-local", action="store_true", help="Only analyze explicitly supplied exports.")
    for label in ("chatgpt", "claude", "gemini"):
        parser.add_argument(f"--{label}-export", action="append", type=Path, default=[],
                            help="JSON, ZIP or unpacked export directory (repeatable).")
    parser.add_argument("--codex-rollouts", action="append", type=Path, default=[],
                        help="Additional rollout JSONL/Zstandard file or directory (repeatable).")
    parser.add_argument("--gemini-sessions", action="append", type=Path, default=[],
                        help="Additional Gemini CLI session JSON/JSONL file or directory.")
    home = Path.home()
    data = Path(os.environ.get("LOCALAPPDATA") or (home / "Library/Application Support" if sys.platform == "darwin"
                else os.environ.get("XDG_DATA_HOME") or home / ".local/share"))
    parser.add_argument("--output", type=Path, default=data / "voicebar/personal/history.json")
    args = parser.parse_args()
    inputs = [] if args.no_local else list(discover(home))
    for source, paths in [("ChatGPT export", args.chatgpt_export), ("Claude export", args.claude_export),
                          ("Gemini export", args.gemini_export), ("Codex archive", args.codex_rollouts),
                          ("Gemini CLI", args.gemini_sessions)]:
        for path in paths:
            if path.is_dir():
                if source.endswith(" export"):
                    matches = [f for f in path.rglob("*.json") if export_name(source, str(f))]
                elif source == "Codex archive":
                    matches = [f for f in path.rglob("rollout-*.jsonl*") if f.suffix in (".jsonl", ".zst")]
                else:
                    matches = [f for f in path.rglob("session-*.json*") if f.suffix in (".jsonl", ".json")]
                inputs.extend((source, f) for f in matches or [path])
            else:
                inputs.append((source, path))
    report = profile(inputs)
    write_report(args.output, report)
    print(json.dumps(report, ensure_ascii=True))
    return 1 if any(stats["files_with_errors"] for stats in report["sources"].values()) else 0


if __name__ == "__main__":
    raise SystemExit(main())
