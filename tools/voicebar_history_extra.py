"""Read user-authored turns from additional local assistant histories."""

import json
import os
import re
import sqlite3
import sys
from collections import Counter
from contextlib import closing
from pathlib import Path

MAX_HISTORY_JSON = 64 * 1024 * 1024


def extra_sources(home: Path):
    windows = home / "AppData/Roaming"
    mac = home / "Library/Application Support"
    linux = home / ".config"
    preferred = windows if sys.platform == "win32" else mac if sys.platform == "darwin" else linux
    configs = [preferred, windows, mac, linux]
    data = [home / ".local/share"]
    if home.resolve() == Path.home().resolve():
        # Alternate fixture homes stay isolated from the active user's stores.
        for name, roots in (("APPDATA", configs), ("XDG_CONFIG_HOME", configs), ("XDG_DATA_HOME", data)):
            path = Path(os.environ.get(name, ""))
            if path.is_absolute():
                roots.insert(0, path)
    configs = list(dict.fromkeys(configs))
    grok = home / ".grok/sessions"
    groups = {
        "OpenCode": [(root / "opencode", "opencode.db") for root in dict.fromkeys(data)],
        "Cursor": [(root / "Cursor/User/globalStorage", "state.vscdb") for root in configs]
                  + [(home / ".cursor/projects", "*/agent-transcripts/**/*.jsonl")],
        "Grok CLI": [(grok, "**/prompt_history.jsonl"), (grok, "**/chat_history.jsonl")],
        "Cline CLI": [(home / ".cline/data/sessions", "*/*.messages.json")],
        "Cline": [(root / "Code/User/globalStorage/saoudrizwan.claude-dev/tasks", "*/ui_messages.json")
                  for root in configs],
        "GitHub Copilot CLI": [(home / ".copilot/session-state", "*/events.jsonl")],
        "VS Code Chat": [(root / "Code/User/workspaceStorage", "*/chatSessions/*.json*") for root in configs],
    }
    for provider, stores in groups.items():
        found = False
        for root, pattern in stores:
            for path in sorted(root.glob(pattern)):
                if path.is_file() and "subagents" not in path.relative_to(root).parts:
                    found = True
                    yield provider, path
        if not found:
            yield provider, stores[0][0]


def _lines(path, issues):
    from voicebar_history import read_lines
    with path.open("rb") as stream:
        yield from read_lines(stream, issues)


def _json(path):
    with path.open("rb") as stream:
        if os.fstat(stream.fileno()).st_size > MAX_HISTORY_JSON:
            raise ValueError("oversize_history_file")
        data = stream.read(MAX_HISTORY_JSON + 1)
        if len(data) > MAX_HISTORY_JSON:
            raise ValueError("oversize_history_file")
        return json.loads(data)


def _wrapped(text, tags):
    if isinstance(text, str):
        # Only the message between the application-supplied markers, excluding
        # neighboring reminders, tool output and attached workspace context.
        for tag in tags:
            match = re.search(rf"<{tag}(?:\s+[^>]*)?>\s*(.*?)\s*</{tag}>", text, re.DOTALL)
            if match and match[1]:
                return match[1]
    return None


def _opencode(connection):
    sql = """
        SELECT m.id, json_extract(p.data, '$.text')
        FROM message m
        JOIN session s ON s.id = m.session_id
        JOIN part p ON p.message_id = m.id AND p.session_id = s.id
        WHERE s.parent_id IS NULL
          AND json_extract(m.data, '$.role') = 'user'
          AND json_extract(p.data, '$.type') = 'text'
          AND NOT coalesce(json_extract(p.data, '$.synthetic'), 0)
          AND NOT coalesce(json_extract(p.data, '$.ignored'), 0)
        ORDER BY m.time_created, m.id, p.id
    """
    prev, parts = None, []
    for message, text in connection.execute(sql):
        if message != prev and parts:
            yield "\n".join(parts)
            parts = []
        prev = message
        if isinstance(text, str) and text.strip():
            parts.append(text)
    if parts:
        yield "\n".join(parts)


def _cursor_db(connection):
    sql = """
        SELECT json_extract(value, '$.text') FROM cursorDiskKV
        WHERE key LIKE 'bubbleId:%' AND json_valid(value)
          AND json_extract(value, '$.type') = 1
          AND NOT coalesce(json_extract(value, '$.isNudge'), 0)
          AND NOT coalesce(json_extract(value, '$.skipRendering'), 0)
          AND NOT coalesce(json_extract(value, '$.isPlanExecution'), 0)
    """
    for (text,) in connection.execute(sql):
        if isinstance(text, str) and text.strip():
            yield text


def _text_blocks(event):
    content = event.get("content")
    if isinstance(content, list):
        for block in content:
            if isinstance(block, dict) and block.get("type") == "text":
                yield block.get("text")


def _vscode_requests(event):
    # Only submitted requests. inputState.inputText is an unsent editor draft.
    kind = event.get("kind")
    obj = event.get("v") if kind is not None else event
    key = event.get("k", [])
    if kind in (None, 0) and isinstance(obj, dict):
        yield from obj.get("requests", [])
    elif kind in (1, 2) and key == ["requests"] and isinstance(obj, list):
        yield from obj
    elif kind in (1, 2) and len(key) == 2 and key[0] == "requests" and isinstance(key[1], int):
        yield obj


def extra_turns(source_name: str, path: Path, issues=None):
    if issues is None:
        issues = Counter()
    if source_name == "OpenCode" or (source_name == "Cursor" and path.suffix == ".vscdb"):
        with closing(sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True)) as connection:
            connection.execute("PRAGMA query_only = ON")
            if source_name == "OpenCode":
                yield from _opencode(connection)
            else:
                yield from _cursor_db(connection)
        return
    if source_name == "Cline":
        for event in _json(path):
            if event.get("type") == "say" and event.get("say") in ("task", "user_feedback"):
                if isinstance(event.get("text"), str):
                    yield event["text"]
        return
    if source_name == "Cline CLI":
        for message in _json(path).get("messages", []):
            if message.get("role") == "user":
                for block in _text_blocks(message):
                    value = _wrapped(block, ("user_input", "user_message", "task", "feedback"))
                    if value:
                        yield value
        return
    if source_name == "VS Code Chat":
        events = _lines(path, issues) if path.suffix == ".jsonl" else [_json(path)]
        for event in events:
            for request in _vscode_requests(event):
                if isinstance(request, dict):
                    message = request.get("message")
                    value = message.get("text") if isinstance(message, dict) else message
                    if isinstance(value, str) and value.strip():
                        yield value
        return
    for event in _lines(path, issues):
        if source_name == "Cursor" and event.get("role") == "user":
            for block in _text_blocks(event.get("message", {})):
                value = _wrapped(block, ("user_query",))
                if value:
                    yield value
        elif source_name == "Grok CLI":
            if path.name == "prompt_history.jsonl":
                if event.get("is_bash") is False and isinstance(event.get("prompt"), str):
                    yield event["prompt"]
            elif event.get("type") == "user" and isinstance(event.get("prompt_index"), int):
                for block in _text_blocks(event):
                    value = _wrapped(block, ("user_query",))
                    if value:
                        yield value
        elif source_name == "GitHub Copilot CLI" and event.get("type") == "user.message":
            data = event.get("data")
            if isinstance(data, dict) and isinstance(data.get("content"), str):
                yield data["content"]
