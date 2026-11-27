use super::{Source, children, json_file, json_lines, parts};
use prost::Message;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn add(sources: &mut Vec<Source>, provider: &'static str, path: PathBuf) {
    if path.symlink_metadata().is_ok_and(|m| m.is_file()) {
        Source::add(sources, provider, path);
    }
}

fn files(root: &Path, recursive: bool, visit: &mut impl FnMut(PathBuf)) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_file() {
                visit(entry.path());
            } else if recursive && kind.is_dir() && entry.file_name() != "subagents" {
                pending.push(entry.path());
            }
        }
    }
}

pub(super) fn discover(home: &Path, sources: &mut Vec<Source>) {
    let mut configs = vec![
        home.join("AppData/Roaming"),
        home.join("Library/Application Support"),
        home.join(".config"),
    ];
    let mut data = vec![home.join(".local/share")];
    let mut codex = home.join(".codex");
    if env::home_dir().as_deref() == Some(home) {
        for (name, roots) in [("APPDATA", &mut configs), ("XDG_DATA_HOME", &mut data)] {
            if let Some(path) = env::var_os(name)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
            {
                roots.push(path);
            }
        }
        if let Some(path) = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
        {
            configs.push(path);
        }
        if let Some(path) = env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
        {
            codex = path;
        }
    }
    configs.sort();
    configs.dedup();
    data.sort();
    data.dedup();
    for root in data {
        add(sources, "OpenCode", root.join("opencode/opencode.db"));
    }
    if codex.join("state_5.sqlite").is_file() {
        add(
            sources,
            "Codex desktop",
            codex.join("thread_history_1.sqlite"),
        );
    }
    for root in configs {
        add(
            sources,
            "Cursor",
            root.join("Cursor/User/globalStorage/state.vscdb"),
        );
        for task in children(
            &root.join("Code/User/globalStorage/saoudrizwan.claude-dev/tasks"),
            true,
        ) {
            add(sources, "Cline", task.join("ui_messages.json"));
        }
        for workspace in children(&root.join("Code/User/workspaceStorage"), true) {
            files(&workspace.join("chatSessions"), false, &mut |p| {
                if matches!(
                    p.extension().and_then(OsStr::to_str),
                    Some("json" | "jsonl")
                ) {
                    add(sources, "VS Code Chat", p);
                }
            });
        }
    }
    for project in children(&home.join(".cursor/projects"), true) {
        files(&project.join("agent-transcripts"), true, &mut |p| {
            if p.extension() == Some(OsStr::new("jsonl")) {
                add(sources, "Cursor", p);
            }
        });
    }
    files(&home.join(".grok/sessions"), true, &mut |p| {
        if matches!(
            p.file_name().and_then(OsStr::to_str),
            Some("prompt_history.jsonl" | "chat_history.jsonl")
        ) {
            add(sources, "Grok CLI", p);
        }
    });
    for session in children(&home.join(".copilot/session-state"), true) {
        add(sources, "GitHub Copilot CLI", session.join("events.jsonl"));
    }
    files(&home.join(".cline/data/sessions"), true, &mut |p| {
        if p.file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.ends_with(".messages.json"))
        {
            add(sources, "Cline CLI", p);
        }
    });
    for base in [
        ".gemini/antigravity/conversations",
        ".gemini/antigravity-ide/conversations",
    ] {
        files(&home.join(base), false, &mut |p| {
            if p.extension() == Some(OsStr::new("db")) {
                add(sources, "Antigravity", p);
            }
        });
    }
}

fn wrapped<'a>(text: &'a str, tags: &[&str]) -> Option<&'a str> {
    for tag in tags {
        let prefix = format!("<{tag}");
        let close = format!("</{tag}>");
        // Match the actual opening tag, including tags with attributes.
        for (start, _) in text.match_indices(&prefix) {
            let rest = &text[start + prefix.len()..];
            if !(rest.starts_with('>') || rest.starts_with(char::is_whitespace)) {
                continue;
            }
            let Some((_, body)) = rest.split_once('>') else {
                continue;
            };
            let Some((value, _)) = body.split_once(&close) else {
                continue;
            };
            if !value.trim().is_empty() {
                return Some(value.trim());
            }
        }
    }
    None
}

fn blocks<'a>(v: &'a Value) -> impl Iterator<Item = &'a str> {
    v.as_array().into_iter().flatten().filter_map(|block| {
        (block["type"] == "text")
            .then(|| block["text"].as_str())
            .flatten()
    })
}

fn vscode_request(request: &Value, emit: &mut dyn FnMut(&str)) {
    if let Some(text) = request
        .pointer("/message/text")
        .or_else(|| request.get("message"))
        .and_then(Value::as_str)
    {
        emit(text);
    }
}

fn vscode(event: &Value, emit: &mut dyn FnMut(&str)) {
    let key = event["k"].as_array().map(Vec::as_slice).unwrap_or_default();
    let requests = match event["kind"].as_i64() {
        None => event["requests"].as_array(),
        Some(0) => event["v"]["requests"].as_array(),
        Some(1 | 2) if key.len() == 1 && key[0] == "requests" => event["v"].as_array(),
        Some(1 | 2) if key.len() == 2 && key[0] == "requests" && key[1].is_u64() => {
            vscode_request(&event["v"], emit);
            None
        }
        _ => None,
    };
    for request in requests.into_iter().flatten() {
        vscode_request(request, emit);
    }
}

fn event_turns(provider: &str, prompt_history: bool, event: &Value, emit: &mut dyn FnMut(&str)) {
    match provider {
        "VS Code Chat" => vscode(event, emit),
        "Cursor" if event["role"] == "user" => {
            for text in
                blocks(&event["message"]["content"]).filter_map(|s| wrapped(s, &["user_query"]))
            {
                emit(text);
            }
        }
        "Grok CLI" if prompt_history && event["is_bash"] == false => {
            if let Some(text) = event["prompt"].as_str() {
                emit(text);
            }
        }
        "Grok CLI"
            if !prompt_history && event["type"] == "user" && event["prompt_index"].is_i64() =>
        {
            for text in blocks(&event["content"]).filter_map(|s| wrapped(s, &["user_query"])) {
                emit(text);
            }
        }
        "GitHub Copilot CLI" if event["type"] == "user.message" => {
            if let Some(text) = event["data"]["content"].as_str() {
                emit(text);
            }
        }
        _ => {}
    }
}

pub(super) fn read(provider: &str, path: &Path, emit: &mut dyn FnMut(&str)) -> io::Result<usize> {
    match provider {
        "Codex desktop" | "OpenCode" | "Antigravity" => read_db(provider, path, emit)
            .map(|_| 0)
            .map_err(io::Error::other),
        "Cursor" if path.extension() == Some(OsStr::new("vscdb")) => read_db(provider, path, emit)
            .map(|_| 0)
            .map_err(io::Error::other),
        "Cline" => {
            let value = json_file(path)?;
            for event in value.as_array().into_iter().flatten() {
                if event["type"] == "say"
                    && matches!(event["say"].as_str(), Some("task" | "user_feedback"))
                {
                    if let Some(text) = event["text"].as_str() {
                        emit(text);
                    }
                }
            }
            Ok(0)
        }
        "Cline CLI" => {
            let value = json_file(path)?;
            for msg in value["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["role"] == "user")
            {
                for text in blocks(&msg["content"])
                    .filter_map(|t| wrapped(t, &["user_input", "user_message", "task", "feedback"]))
                {
                    emit(text);
                }
            }
            Ok(0)
        }
        "VS Code Chat" if path.extension() == Some(OsStr::new("json")) => {
            vscode(&json_file(path)?, emit);
            Ok(0)
        }
        "Cursor" | "Grok CLI" | "GitHub Copilot CLI" | "VS Code Chat" => {
            let is_prompt = path.file_name() == Some(OsStr::new("prompt_history.jsonl"));
            json_lines(path, |value| {
                event_turns(provider, is_prompt, value, emit);
                true
            })
        }
        _ => Err(io::Error::other("unsupported history provider")),
    }
}

fn open_db(path: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(std::time::Duration::from_millis(500))?;
    db.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
    Ok(db)
}

fn read_db(provider: &str, path: &Path, emit: &mut dyn FnMut(&str)) -> rusqlite::Result<()> {
    let db = open_db(path)?;
    match provider {
        "OpenCode" => {
            let mut select = db.prepare(
                "SELECT m.id, json_extract(p.data, '$.text') FROM message m
                 JOIN session s ON s.id=m.session_id JOIN part p ON p.message_id=m.id AND p.session_id=s.id
                 WHERE s.parent_id IS NULL AND json_extract(m.data, '$.role')='user'
                 AND json_extract(p.data, '$.type')='text' AND NOT coalesce(json_extract(p.data, '$.synthetic'),0)
                 AND NOT coalesce(json_extract(p.data, '$.ignored'),0) ORDER BY m.time_created, m.id, p.id")?;
            let mut rows = select.query([])?;
            let (mut previous, mut parts) = (String::new(), String::new());
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                if id != previous {
                    if !parts.is_empty() {
                        emit(&parts);
                    }
                    parts.clear();
                    previous = id;
                }
                if let Some(text) = r
                    .get::<_, Option<String>>(1)?
                    .filter(|s| !s.trim().is_empty())
                {
                    if !parts.is_empty() {
                        parts.push('\n');
                    }
                    parts.push_str(&text);
                }
            }
            if !parts.is_empty() {
                emit(&parts);
            }
        }
        "Cursor" => {
            let mut select = db.prepare(
                "SELECT json_extract(value,'$.text') FROM cursorDiskKV
                 WHERE key LIKE 'bubbleId:%' AND json_valid(value) AND json_extract(value,'$.type')=1
                 AND NOT coalesce(json_extract(value,'$.isNudge'),0)
                 AND NOT coalesce(json_extract(value,'$.skipRendering'),0)
                 AND NOT coalesce(json_extract(value,'$.isPlanExecution'),0)
                 ORDER BY json_extract(value,'$.createdAt'), key")?;
            let mut rows = select.query([])?;
            while let Some(row) = rows.next()? {
                if let Some(text) = row.get::<_, Option<String>>(0)? {
                    emit(&text);
                }
            }
        }
        "Codex desktop" => {
            let state = open_db(&path.with_file_name("state_5.sqlite"))?;
            let mut threads = state.prepare("SELECT id FROM threads WHERE (agent_path IS NULL OR agent_path='/root')
                                               AND (source IS NULL OR source NOT LIKE '%subagent%') ORDER BY updated_at, id")?;
            let mut select = db.prepare(
                "SELECT item_json FROM thread_items WHERE thread_id=?1 AND item_type='userMessage'",
            )?;
            let mut thread_rows = threads.query([])?;
            while let Some(thread) = thread_rows.next()? {
                let mut rows = select.query([thread.get::<_, String>(0)?])?;
                while let Some(row) = rows.next()? {
                    let Ok(json) = row.get::<_, String>(0) else {
                        continue;
                    };
                    let Ok(item) = serde_json::from_str::<Value>(&json) else {
                        continue;
                    };
                    if item["type"] == "userMessage" {
                        emit(&parts(&item["content"], |_| true));
                    }
                }
            }
        }
        "Antigravity" => {
            let supported: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM trajectory_meta WHERE trajectory_type IN (1,2,4,17) AND source IN (1,12,17))", [], |r| r.get(0))?;
            if !supported {
                return Ok(());
            }
            let mut select = db.prepare(
                "SELECT step_payload FROM steps WHERE step_type=14 AND step_format=0 ORDER BY idx",
            )?;
            let mut rows = select.query([])?;
            while let Some(row) = rows.next()? {
                let Ok(bytes) = row.get::<_, Vec<u8>>(0) else {
                    continue;
                };
                let Ok(step) = Step::decode(bytes.as_slice()) else {
                    continue;
                };
                if step.kind == 14
                    && let Some(input) = step.user_input
                {
                    let text: String = input.items.into_iter().map(|item| item.text).collect();
                    if !text.is_empty() {
                        emit(&text);
                    }
                }
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

// Only the typed user-input fields of the Gemini Coder Step projection.
#[derive(Clone, PartialEq, Message)]
struct Step {
    #[prost(int32, tag = "1")]
    kind: i32,
    #[prost(message, optional, tag = "19")]
    user_input: Option<UserInput>,
}
#[derive(Clone, PartialEq, Message)]
struct UserInput {
    #[prost(message, repeated, tag = "3")]
    items: Vec<TextItem>,
}
#[derive(Clone, PartialEq, Message)]
struct TextItem {
    #[prost(string, tag = "1")]
    text: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn collected(provider: &str, path: &Path) -> Vec<String> {
        let mut results = vec![];
        assert_eq!(
            read(provider, path, &mut |s| results.push(s.to_string())).unwrap(),
            0
        );
        results
    }

    #[test]
    fn opencode_reads_root_user_text_in_message_order_and_never_creates_missing_db() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("opencode.db");
        assert!(open_db(&path).is_err());
        assert!(!path.exists());
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            r#"
            CREATE TABLE session(id TEXT, parent_id TEXT);
            CREATE TABLE message(id TEXT,session_id TEXT,time_created INTEGER,data TEXT);
            CREATE TABLE part(id INTEGER,message_id TEXT,session_id TEXT,data TEXT);
            INSERT INTO session VALUES('s',NULL),('child','s');
            INSERT INTO message VALUES ('m','s',1,'{"role":"user"}'),
                  ('a','s',2,'{"role":"assistant"}'),('c','child',3,'{"role":"user"}');
            INSERT INTO part VALUES(2,'m','s','{"type":"text","text":"second"}'),
                (1,'m','s','{"type":"text","text":"first"}'),
                (3,'m','s','{"type":"text","text":"hidden","synthetic":true}'),
                (4,'a','s','{"type":"text","text":"assistant"}'),
                (5,'c','child','{"type":"text","text":"child task"}');
        "#,
        )
        .unwrap();
        drop(db);
        assert_eq!(collected("OpenCode", &path), ["first\nsecond"]);
    }

    #[test]
    fn native_db_projections_exclude_assistants_agents_and_attached_context() {
        let temp = tempfile::tempdir().unwrap();
        let state = Connection::open(temp.path().join("state_5.sqlite")).unwrap();
        state
            .execute_batch(
                "CREATE TABLE threads(id TEXT,agent_path TEXT,source TEXT,updated_at INTEGER);
            INSERT INTO threads VALUES('root',NULL,NULL,1),('sub','/root/worker','subagent',2);",
            )
            .unwrap();
        let path = temp.path().join("thread_history_1.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(r#"CREATE TABLE thread_items(thread_id TEXT,item_type TEXT,item_json TEXT);
            INSERT INTO thread_items VALUES('root','userMessage','{"type":"userMessage","content":[{"type":"text","text":"request"},{"type":"image","text":"ignore"}]}'),
            ('sub','userMessage','{"type":"userMessage","content":"generated"}');"#).unwrap();
        assert_eq!(collected("Codex desktop", &path), ["request"]);

        let path = temp.path().join("history.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE trajectory_meta(trajectory_type INTEGER,source INTEGER);
            INSERT INTO trajectory_meta VALUES(1,12); CREATE TABLE steps(step_payload BLOB,step_type INTEGER,step_format INTEGER,idx INTEGER);").unwrap();
        for kind in [14, 13] {
            let data = Step {
                kind,
                user_input: Some(UserInput {
                    items: vec![TextItem { text: "say".into() }],
                }),
            }
            .encode_to_vec();
            db.execute("INSERT INTO steps VALUES(?1,14,0,0)", [data])
                .unwrap();
        }
        assert_eq!(collected("Antigravity", &path), ["say"]);
        db.execute("DELETE FROM trajectory_meta", []).unwrap();
        assert!(collected("Antigravity", &path).is_empty());

        let path = temp.path().join("state.vscdb");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(r#"CREATE TABLE cursorDiskKV(key TEXT,value TEXT);
            INSERT INTO cursorDiskKV VALUES('bubbleId:1','{"type":1,"text":"my input"}'),
            ('bubbleId:2','{"type":2,"text":"assistant"}'),('bubbleId:3','{"type":1,"text":"internal","isNudge":true}');"#).unwrap();
        assert_eq!(collected("Cursor", &path), ["my input"]);
    }

    #[test]
    fn discovery_and_json_readers_use_only_submitted_user_content() {
        let temp = tempfile::tempdir().unwrap();
        let cases = [
            (
                "Grok CLI",
                ".grok/sessions/project/prompt_history.jsonl",
                json!({"is_bash":false,"prompt":"from grok"}),
                "from grok",
            ),
            (
                "GitHub Copilot CLI",
                ".copilot/session-state/one/events.jsonl",
                json!({"type":"user.message","data":{"content":"from copilot"}}),
                "from copilot",
            ),
            (
                "Cursor",
                ".cursor/projects/test/agent-transcripts/one/one.jsonl",
                json!({"role":"user","message":{"content":[{"type":"text","text":"<timestamp>omit</timestamp><user_query>from cursor</user_query>"}]}}),
                "from cursor",
            ),
            (
                "Cline CLI",
                ".cline/data/sessions/one/a.messages.json",
                json!({"messages":[{"role":"user","content":[{"type":"text","text":"<user_input mode=\"act\">from cline</user_input><environment_details>omit</environment_details>"}]}]}),
                "from cline",
            ),
            (
                "Cline",
                "AppData/Roaming/Code/User/globalStorage/saoudrizwan.claude-dev/tasks/one/ui_messages.json",
                json!([{"type":"say","say":"task","text":"from task"},{"type":"say","say":"text","text":"assistant"}]),
                "from task",
            ),
            (
                "VS Code Chat",
                "Library/Application Support/Code/User/workspaceStorage/one/chatSessions/chat.json",
                json!({"inputState":{"inputText":"draft"},"requests":[{"message":{"text":"submitted"}}]}),
                "submitted",
            ),
        ];
        let mut sources = vec![];
        for (_, relative, event, _) in &cases {
            let path = temp.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, serde_json::to_vec(event).unwrap()).unwrap();
        }
        discover(temp.path(), &mut sources);
        for (provider, relative, _, expected) in cases {
            let path = temp.path().join(relative);
            assert!(
                sources
                    .iter()
                    .any(|source| source.label == provider && source.files.contains(&path))
            );
            assert_eq!(collected(provider, &path), [expected]);
        }
    }

    #[test]
    fn wrapped_recovers_when_earlier_tag_is_unclosed_in_prose() {
        let text = "<task>Fix unclosed <user_input> parser edge case</task>";
        assert_eq!(
            wrapped(text, &["user_input", "user_message", "task", "feedback"]),
            Some("Fix unclosed <user_input> parser edge case")
        );
    }
}
