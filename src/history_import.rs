//! Discover local history files by name. Only an explicitly selected source is opened.

mod extra;

use serde_json::Value;
use std::collections::{HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

const MAX_ENTRIES: usize = 50;
const MAX_TEXT_BYTES: usize = crate::personalization::MAX_TRANSCRIPT_BYTES;
const MAX_JSON_LINE: usize = 8 * 1024 * 1024;
const MAX_JSON_FILE: u64 = 32 * 1024 * 1024;
const MAX_STREAM_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Source {
    pub label: &'static str,
    files: Vec<PathBuf>,
}

impl Source {
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn is_voice(&self) -> bool {
        matches!(self.label, "Codex dictation" | "Antigravity voice")
    }

    fn add(sources: &mut Vec<Self>, label: &'static str, path: PathBuf) {
        if !path.symlink_metadata().is_ok_and(|m| m.is_file()) {
            return;
        }
        if let Some(source) = sources.iter_mut().find(|source| source.label == label) {
            source.files.push(path);
        } else {
            sources.push(Self {
                label,
                files: vec![path],
            });
        }
    }
}

pub struct ImportReport {
    pub entries: Vec<String>,
    pub summary: String,
}

fn children(root: &Path, directories: bool) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
            (if directories {
                kind.is_dir()
            } else {
                kind.is_file()
            })
            .then(|| entry.path())
        })
        .collect()
}

fn named(path: &Path, prefix: &str, endings: &[&str]) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with(prefix) && endings.iter().any(|end| name.ends_with(end))
        })
}

fn rollout_files(root: &Path, levels: usize, sources: &mut Vec<Source>) {
    for file in children(root, false) {
        if named(&file, "rollout-", &[".jsonl", ".jsonl.zst"]) {
            Source::add(sources, "Codex", file);
        }
    }
    if levels > 0 {
        for dir in children(root, true) {
            rollout_files(&dir, levels - 1, sources);
        }
    }
}

pub fn discover() -> Vec<Source> {
    let Some(home) = std::env::home_dir() else {
        return Vec::new();
    };
    let codex = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    discover_at(&home, &codex, &claude)
}

fn discover_at(home: &Path, codex: &Path, claude: &Path) -> Vec<Source> {
    let mut sources = Vec::new();
    Source::add(&mut sources, "Codex", codex.join("history.jsonl"));
    for folder in ["sessions", "archived_sessions"] {
        rollout_files(&codex.join(folder), 3, &mut sources);
    }
    Source::add(
        &mut sources,
        "Codex dictation",
        codex.join("transcription-history.jsonl"),
    );
    for dir in children(&codex.join("dictation-history"), true) {
        for file in children(&dir, false) {
            if named(&file, "", &[".json"]) {
                Source::add(&mut sources, "Codex dictation", file);
            }
        }
    }
    Source::add(&mut sources, "Claude Code", claude.join("history.jsonl"));
    for project in children(&claude.join("projects"), true) {
        // Top-level conversations only; nested folders contain subagent turns.
        for file in children(&project, false) {
            if named(&file, "", &[".jsonl"]) {
                Source::add(&mut sources, "Claude Code", file);
            }
        }
    }
    for project in children(&home.join(".gemini/tmp"), true) {
        for file in children(&project.join("chats"), false) {
            if named(&file, "session-", &[".json", ".jsonl"]) {
                Source::add(&mut sources, "Gemini CLI", file);
            }
        }
    }
    for agent in ["antigravity", "antigravity-ide"] {
        let root = home.join(".gemini").join(agent);
        for file in children(&root.join("voice_chats"), false) {
            if named(&file, "", &[".json"]) {
                Source::add(&mut sources, "Antigravity voice", file);
            }
        }
        // Some older conversations also have a role-labelled text transcript.
        for file in children(&root.join("conversations"), false) {
            if named(&file, "", &[".pb"])
                && let Some(id) = file.file_stem()
            {
                let logs = root.join("brain").join(id).join(".system_generated/logs");
                let transcript = logs.join("transcript_full.jsonl");
                Source::add(
                    &mut sources,
                    "Antigravity chat logs",
                    if transcript.is_file() {
                        transcript
                    } else {
                        logs.join("transcript.jsonl")
                    },
                );
            }
        }
    }
    extra::discover(home, &mut sources);
    for source in &mut sources {
        source.files.sort_unstable();
        source.files.dedup();
    }
    sources.sort_by_key(|source| source.label);
    sources
}

/// Feed only complete JSON records to the callback. Oversized records are drained in chunks.
pub(super) fn json_lines(
    path: &Path,
    mut callback: impl FnMut(&Value) -> bool,
) -> io::Result<usize> {
    let file = File::open(path)?;
    let stream: Box<dyn Read> = if named(path, "", &[".zst"]) {
        Box::new(zstd::Decoder::new(file)?)
    } else {
        Box::new(file)
    };
    let mut reader = BufReader::new(stream.take(MAX_STREAM_BYTES));
    let mut line = Vec::new();
    let mut oversized = false;
    let mut skipped = 0;
    let mut first_line = true;
    loop {
        let bytes = reader.fill_buf()?;
        let eof = bytes.is_empty();
        let newline = bytes.iter().position(|b| *b == b'\n');
        let n = newline.map_or(bytes.len(), |index| index + 1);
        if line.len() + n > MAX_JSON_LINE {
            oversized = true;
        }
        if !oversized {
            line.extend_from_slice(&bytes[..n]);
        }
        reader.consume(n);
        if newline.is_some() || eof {
            if eof && reader.get_ref().limit() == 0 {
                let mut extra = [0];
                if reader.get_mut().get_mut().read(&mut extra)? != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::FileTooLarge,
                        "history byte limit reached",
                    ));
                }
            }
            if oversized {
                skipped += 1;
            } else if !line.iter().all(u8::is_ascii_whitespace) {
                let raw = if first_line {
                    line.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&line)
                } else {
                    &line
                };
                match serde_json::from_slice::<Value>(raw) {
                    Ok(row) => {
                        if !callback(&row) {
                            break;
                        }
                    }
                    Err(_) => skipped += 1,
                }
            }
            line.clear();
            oversized = false;
            first_line = false;
        }
        if eof {
            break;
        }
    }
    Ok(skipped)
}

fn json_file(path: &Path) -> io::Result<Value> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_JSON_FILE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON_FILE {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "history JSON is too large",
        ));
    }
    serde_json::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes))
        .map_err(io::Error::other)
}

fn parts(value: &Value, accept: impl Fn(usize) -> bool) -> String {
    if let Some(text) = value.as_str() {
        return if accept(0) {
            text.to_owned()
        } else {
            String::new()
        };
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, part)| {
            if !accept(index) {
                return None;
            }
            match part.get("type").and_then(Value::as_str) {
                None | Some("text" | "input_text") => {
                    part.as_str().or_else(|| part["text"].as_str())
                }
                _ => None,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn codex_row(row: &Value, emit: &mut dyn FnMut(&str)) -> bool {
    let msg = &row["payload"];
    match row["type"].as_str() {
        Some("session_meta") => {
            let is_subagent_str = msg["source"]
                .as_str()
                .is_some_and(|s| s.to_ascii_lowercase().contains("subagent"));
            if msg["source"].get("subagent").is_some()
                || is_subagent_str
                || msg["agent_path"]
                    .as_str()
                    .is_some_and(|agent| agent != "/root")
            {
                return false;
            }
        }
        Some("event_msg") if msg["type"] == "user_message" => {
            if let Some(text) = msg["message"].as_str() {
                emit(text);
            }
        }
        Some("response_item") if msg["role"] == "user" && msg["type"] == "message" => {
            let meta = &msg["internal_chat_message_metadata_passthrough"];
            let kinds = meta.get("content_item_kinds").and_then(Value::as_array);
            let origins = meta.get("content_item_metadata").and_then(Value::as_array);
            let labelled =
                kinds.is_some_and(|v| !v.is_empty()) || origins.is_some_and(|v| !v.is_empty());
            emit(&parts(&msg["content"], |index| {
                if !labelled {
                    return true;
                }
                let kind = kinds.and_then(|v| v.get(index)).and_then(Value::as_str);
                let origin = origins
                    .and_then(|v| v.get(index))
                    .and_then(|v| v.pointer("/provenance/type"))
                    .and_then(Value::as_str);
                matches!(kind, None | Some("user.text"))
                    && matches!(origin, None | Some("user"))
                    && (kind.is_some() || origin.is_some())
            }));
        }
        _ => {}
    }
    true
}

fn gemini_row(row: &Value, emit: &mut dyn FnMut(&str)) -> bool {
    if row["kind"] == "subagent" || row["$set"]["kind"] == "subagent" {
        return false;
    }
    let mut message = |msg: &Value| {
        if msg["type"] == "user" {
            emit(&parts(&msg["content"], |_| true));
        }
    };
    if let Some(items) = row["$set"]["messages"]
        .as_array()
        .or_else(|| row["messages"].as_array())
    {
        items.iter().for_each(message);
    } else {
        message(row);
    }
    true
}

fn read_file(source: &str, path: &Path, emit: &mut dyn FnMut(&str)) -> io::Result<usize> {
    match source {
        "Codex" | "Claude Code" if named(path, "history.", &[".jsonl"]) => {
            json_lines(path, |row| {
                if let Some(text) = row[if source == "Codex" { "text" } else { "display" }].as_str()
                {
                    emit(text);
                }
                true
            })
        }
        "Codex" => json_lines(path, |row| codex_row(row, emit)),
        "Codex dictation" if named(path, "", &[".json"]) => {
            if let Some(text) = json_file(path)?["text"].as_str() {
                emit(text);
            }
            Ok(0)
        }
        "Codex dictation" => json_lines(path, |row| {
            if let Some(text) = row["text"].as_str() {
                emit(text);
            }
            true
        }),
        "Claude Code" => json_lines(path, |row| {
            if row["type"] == "user"
                && row["message"]["role"] == "user"
                && row["isSidechain"] != true
                && row["isMeta"] != true
                && row["isCompactSummary"] != true
                && row.get("turnOrigin").is_none_or(|v| v == "human")
                && row["sourceToolAssistantUUID"].is_null()
            {
                let content = &row["message"]["content"];
                if !content
                    .as_array()
                    .is_some_and(|v| v.iter().any(|item| item["type"] == "tool_result"))
                {
                    emit(&parts(content, |_| true));
                }
            }
            true
        }),
        "Gemini CLI" if named(path, "", &[".json"]) => {
            gemini_row(&json_file(path)?, emit);
            Ok(0)
        }
        "Gemini CLI" => json_lines(path, |row| gemini_row(row, emit)),
        "Antigravity voice" => {
            for chat in json_file(path)?["chats"].as_array().into_iter().flatten() {
                for line in chat["lines"].as_array().into_iter().flatten() {
                    if line["role"] == "ROLE_USER"
                        && let Some(text) = line["text"].as_str()
                    {
                        emit(text);
                    }
                }
            }
            Ok(0)
        }
        "Antigravity chat logs" => json_lines(path, |row| {
            if row["type"] == "USER_INPUT"
                && row["source"] == "USER_EXPLICIT"
                && let Some(body) = row["content"]
                    .as_str()
                    .and_then(|s| s.trim_start().strip_prefix("<USER_REQUEST>"))
                && let Some((user_text, _)) = body.split_once("</USER_REQUEST>")
            {
                emit(user_text);
            }
            true
        }),
        _ => extra::read(source, path, emit),
    }
}

fn eligible(text: &str) -> Option<&str> {
    let text = text.trim();
    if text.is_empty()
        || text.len() > MAX_TEXT_BYTES
        || text.starts_with(['<', '[', '{', '>', '`', '/'])
        || text.starts_with("# AGENTS.md")
        || text.starts_with("This session is being continued from a previous conversation")
        || text.contains("<environment_context")
        || text.contains("<environment_details")
        || text.contains("<system-reminder")
    {
        return None;
    }
    Some(text)
}

fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
pub fn read_selected(sources: &[Source]) -> ImportReport {
    let never = AtomicBool::new(false);
    read_selected_cancellable(sources, &never).unwrap_or(ImportReport {
        entries: Vec::new(),
        summary: String::new(),
    })
}

pub fn read_selected_cancellable(sources: &[Source], cancel: &AtomicBool) -> Option<ImportReport> {
    let mut buckets = Vec::new();
    let mut details = Vec::new();
    for source in sources {
        if cancel.load(Ordering::Acquire) {
            return None;
        }
        let mut recent: Vec<_> = source
            .files
            .iter()
            .map(|path| {
                let modified = fs::metadata(path)
                    .and_then(|meta| meta.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                (modified, path)
            })
            .collect();
        recent.sort_unstable_by(|a, b| b.cmp(a));
        let count = recent.len();
        let mut kept = Vec::new();
        let mut seen = HashSet::new();
        let (mut eligible_count, mut errors, mut skipped, mut over_limit) = (0, 0, 0, 0);
        let (mut long_text, mut excluded) = (0, 0);
        let mut checked_files = 0usize;
        const EARLY_STOP_MIN_FILES: usize = 80;
        for (_, path) in recent {
            if cancel.load(Ordering::Acquire) {
                return None;
            }
            // Once we already collected the full sample budget from the newest sessions,
            // stop parsing hundreds of older multi-MB rollout logs so import finishes in <1s.
            if kept.len() >= MAX_ENTRIES && checked_files >= EARLY_STOP_MIN_FILES {
                break;
            }
            checked_files += 1;
            let mut file_entries = VecDeque::new();
            let result = read_file(source.label, path, &mut |message| {
                if cancel.load(Ordering::Acquire) {
                    return;
                }
                if let Some(text) = eligible(message) {
                    eligible_count += 1;
                    if kept.len() == MAX_ENTRIES {
                        return;
                    }
                    let key = normalized(text);
                    if let Some(index) = file_entries
                        .iter()
                        .position(|(existing, _)| existing == &key)
                    {
                        file_entries.remove(index);
                    } else if file_entries.len() == MAX_ENTRIES {
                        file_entries.pop_front();
                    }
                    file_entries.push_back((key, text.to_owned()));
                } else if message.trim().len() > MAX_TEXT_BYTES {
                    long_text += 1;
                } else if !message.trim().is_empty() {
                    excluded += 1;
                }
            });
            match result {
                Ok(n) => skipped += n,
                Err(err) if err.kind() == io::ErrorKind::FileTooLarge => over_limit += 1,
                Err(_) => errors += 1,
            }
            for (key, text) in file_entries.into_iter().rev() {
                if kept.len() < MAX_ENTRIES && seen.insert(key) {
                    kept.push(text);
                }
            }
        }
        let origin = if source.is_voice() { "voice" } else { "chat" };
        let files_note = if checked_files < count {
            format!("{checked_files} newest of {count} files checked")
        } else {
            format!("{count} files checked")
        };
        let mut summary = format!(
            "{} ({origin}) {eligible_count} eligible records, {files_note}",
            source.label
        );
        if skipped > 0 {
            summary.push_str(&format!(", {skipped} unreadable or oversized records"));
        }
        if long_text > 0 {
            summary.push_str(&format!(", {long_text} long texts"));
        }
        if excluded > 0 {
            summary.push_str(&format!(", {excluded} context or nonprose entries"));
        }
        if over_limit > 0 {
            summary.push_str(&format!(", {over_limit} files exceeded size limits"));
        }
        if errors > 0 {
            summary.push_str(&format!(", {errors} incomplete or unreadable files"));
        }
        details.push(summary);
        buckets.push(kept.into_iter());
    }
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    while entries.len() < MAX_ENTRIES {
        let before = entries.len();
        for source in &mut buckets {
            if let Some(text) = source.find(|text| seen.insert(normalized(text))) {
                entries.push(text);
            }
            if entries.len() == MAX_ENTRIES {
                break;
            }
        }
        if entries.len() == before {
            break;
        }
    }
    Some(ImportReport {
        entries,
        summary: details.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn lines(path: &Path, values: &[Value]) {
        let mut file = File::create(path).unwrap();
        for value in values {
            writeln!(file, "{value}").unwrap();
        }
    }

    #[test]
    fn compressed_codex_checks_author_provenance_and_excludes_agent_sessions() {
        let home = tempfile::tempdir().unwrap();
        let runs = home.path().join(".codex/sessions/2026/01/01");
        fs::create_dir_all(&runs).unwrap();
        let record = json!({"type":"response_item","payload":{
            "type":"message","role":"user","content":[
                {"type":"input_text","text":"harness instructions"},
                {"type":"input_text","text":"Please check the report."}
            ],"internal_chat_message_metadata_passthrough":{
                "content_item_kinds":["unknown","user.text"],
                "content_item_metadata":[{"provenance":{"type":"harness"}},{"provenance":{"type":"user"}}]}}});
        let file = File::create(runs.join("rollout-main.jsonl.zst")).unwrap();
        let mut encoder = zstd::Encoder::new(file, 1).unwrap();
        writeln!(encoder, "{record}").unwrap();
        encoder.finish().unwrap();
        lines(
            &runs.join("rollout-child.jsonl"),
            &[
                json!({"type":"session_meta","payload":{"source":{"subagent":{}}}}),
                record,
            ],
        );
        let sources = discover_at(
            home.path(),
            &home.path().join(".codex"),
            &home.path().join(".claude"),
        );
        let report = read_selected(&sources);
        assert_eq!(report.entries, ["Please check the report."]);
    }

    #[test]
    fn selection_controls_which_content_is_read_and_deduplicates_samples() {
        let dir = tempfile::tempdir().unwrap();
        let voice = dir.path().join("voice.json");
        fs::write(
            &voice,
            json!({"chats":[{"lines":[
            {"role":"ROLE_USER","text":"First spoken phrase."},
            {"role":"ROLE_VOICE","text":"Model reply."}]}]})
            .to_string(),
        )
        .unwrap();
        let user = dir.path().join("chat.jsonl");
        lines(
            &user,
            &[
                json!({"type":"user","message":{"role":"user","content":"first   spoken phrase."}}),
                json!({"type":"user","message":{"role":"user","content":"Could you check the spelling?"}}),
                json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","text":"output"}]}}),
                json!({"type":"user","isMeta":true,"message":{"role":"user","content":"injected"}}),
            ],
        );
        let mut sources = Vec::new();
        Source::add(&mut sources, "Antigravity voice", voice);
        Source::add(&mut sources, "Claude Code", user);
        assert_eq!(
            read_selected(&sources[0..1]).entries,
            ["First spoken phrase."]
        );
        let report = read_selected(&sources);
        assert_eq!(
            report.entries,
            ["First spoken phrase.", "Could you check the spelling?"]
        );
        assert!(report.summary.contains("(voice) 1 eligible"));
        assert!(report.summary.contains("(chat) 2 eligible"));
    }

    #[test]
    fn jsonl_recovers_from_oversized_and_malformed_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut file = File::create(&path).unwrap();
        file.write_all(&vec![b'x'; MAX_JSON_LINE + 1]).unwrap();
        writeln!(file, "\n!").unwrap();
        writeln!(
            file,
            "{}",
            json!({"type":"user","content":[{"text":"Newest message."}]})
        )
        .unwrap();
        drop(file);
        let mut output = Vec::new();
        let count = read_file("Gemini CLI", &path, &mut |s| output.push(s.to_owned())).unwrap();
        assert_eq!(output, ["Newest message."]);
        assert_eq!(count, 2);
    }

    #[test]
    fn scans_all_selected_files_and_caps_only_the_shared_sample() {
        let dir = tempfile::tempdir().unwrap();
        let mut sources = Vec::new();
        for index in 0..58 {
            let file = dir.path().join(format!("session-{index}.jsonl"));
            lines(
                &file,
                &[json!({"type":"user","content":format!("Message number {index}.")})],
            );
            Source::add(&mut sources, "Gemini CLI", file);
        }
        let file = dir.path().join("dictation.json");
        fs::write(&file, br#"{"text":"Spoken sample."}"#).unwrap();
        Source::add(&mut sources, "Codex dictation", file);
        let report = read_selected(&sources);
        assert_eq!(report.entries.len(), MAX_ENTRIES);
        assert!(report.entries.iter().any(|t| t == "Spoken sample."));
        assert!(
            report
                .summary
                .contains("58 eligible records, 58 files checked")
        );
    }

    #[test]
    fn excludes_string_subagent_sessions_and_slash_commands() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("rollout-sub-str.jsonl");
        lines(
            &sub,
            &[
                json!({"type":"session_meta","payload":{"source":"subagent"}}),
                json!({"type":"event_msg","payload":{"type":"user_message","message":"subagent prompt"}}),
            ],
        );
        let main = dir.path().join("history.jsonl");
        lines(
            &main,
            &[
                json!({"text":"/compact"}),
                json!({"text":"/clear"}),
                json!({"text":"Please deploy ForgeDesk after tests pass."}),
            ],
        );
        let mut sources = Vec::new();
        Source::add(&mut sources, "Codex", sub);
        Source::add(&mut sources, "Codex", main);
        let report = read_selected(&sources);
        assert_eq!(
            report.entries,
            ["Please deploy ForgeDesk after tests pass."]
        );
    }
}
