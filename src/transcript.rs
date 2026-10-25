//! Best-effort Codex rollout adapter. Persist sizes and counters, never payload text.
//! Supports event_msg/token_count and token_usage_record cumulative snapshots.
//! File length detects truncation, not in-place rewrites of equal or larger size;
//! counter regression provides an additional accounting boundary in capture.rs.
use crate::{
    Result,
    model::{Usage, content_size, field},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Default, Serialize, Deserialize)]
pub struct Inventory {
    #[serde(default)]
    pub observed_ms: u128,
    pub bytes: BTreeMap<String, usize>,
    pub items: usize,
    pub compactions: usize,
    pub unsupported: usize,
}
impl Inventory {
    fn add(&mut self, source: &str, bytes: usize) {
        *self.bytes.entry(source.into()).or_default() += bytes;
    }
    fn observe(&mut self, record: &Value) {
        let p = &record["payload"];
        if record["type"] == "compacted" {
            self.compactions += 1;
        }
        if record["type"] != "response_item" {
            return;
        }
        self.items += 1;
        match field(p, "type") {
            "message" => {
                let source = match field(p, "role") {
                    "user" => "user_input",
                    "assistant" => "assistant_output",
                    "system" | "developer" => "instructions_visible",
                    _ => {
                        self.unsupported += 1;
                        return;
                    }
                };
                if let Some(content) = p["content"].as_array() {
                    for part in content {
                        if let Some(text) = part["text"].as_str() {
                            self.add(source, text.len());
                        } else {
                            self.unsupported += 1;
                        }
                    }
                } else {
                    self.unsupported += 1;
                }
            }
            "function_call" => self.add("tool_input", content_size(&p["arguments"])),
            "custom_tool_call" => self.add("tool_input", content_size(&p["input"])),
            "function_call_output" | "custom_tool_call_output" => {
                self.add("tool_result", content_size(&p["output"]))
            }
            "additional_tools" => self.add("tool_schema_visible", content_size(&p["tools"])),
            // Encrypted reasoning and images cannot be inferred from serialized byte size.
            _ => self.unsupported += 1,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct UsagePoint {
    pub turn: String,
    pub timestamp: String,
    pub total: Usage,
    #[serde(default)]
    pub break_before: bool,
}
pub struct Scan {
    pub cursor: u64,
    pub points: Vec<UsagePoint>,
    pub inventory: Option<Inventory>,
    pub malformed: usize,
    pub usage_status: Option<bool>,
    pub truncated: bool,
    pub session_id: Option<String>,
    pub last_turn: String,
}
pub fn scan(path: &Path, offset: u64, inventory: bool, initial_turn: &str) -> Result<Scan> {
    let file = File::open(path)?;
    let truncated = file.metadata()?.len() < offset;
    let offset = if truncated { 0 } else { offset };
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(offset))?;
    let mut result = Scan {
        cursor: offset,
        points: vec![],
        inventory: inventory.then(Inventory::default),
        malformed: 0,
        usage_status: None,
        truncated,
        session_id: None,
        last_turn: initial_turn.into(),
    };
    let mut turn = initial_turn.to_owned();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        // Do not advance over an in-flight final JSONL record. Retry next refresh.
        if !buf.ends_with(b"\n") {
            break;
        }
        result.cursor += n as u64;
        let Ok(v) = serde_json::from_slice::<Value>(&buf) else {
            result.malformed += 1;
            continue;
        };
        if v["type"] == "session_meta" {
            result.session_id = v["payload"]["id"]
                .as_str()
                .or_else(|| v["payload"]["session_id"].as_str())
                .map(str::to_owned);
        }
        if let Some(inv) = &mut result.inventory {
            inv.observe(&v);
        }
        let p = &v["payload"];
        if v["type"] == "turn_context" || p["type"] == "task_started" {
            turn = field(p, "turn_id").to_owned();
        }
        let candidate = if v["type"] == "token_usage_record" {
            Some(p.get("thread_token_usage"))
        } else if v["type"] == "event_msg" && p["type"] == "token_count" {
            p.pointer("/info/total_token_usage").map(Some)
        } else {
            None
        };
        let usage = match candidate {
            Some(Some(value)) => match Usage::from_value(value) {
                Some(usage) => Some(usage),
                None => {
                    result.malformed += 1;
                    result.usage_status = Some(false);
                    None
                }
            },
            Some(None) => {
                result.malformed += 1;
                result.usage_status = Some(false);
                None
            }
            None => None,
        };
        if let Some(total) = usage {
            result.usage_status = Some(true);
            let point = UsagePoint {
                turn: p["turn_id"].as_str().unwrap_or(&turn).to_owned(),
                timestamp: field(&v, "timestamp").into(),
                total,
                break_before: false,
            };
            if result.points.last().is_none_or(|p| p.total != point.total) {
                result.points.push(point);
            }
        }
    }
    result.last_turn = turn;
    Ok(result)
}
// Discovery reads only filename candidates and their first session metadata record.
// It never searches other conversations' content or falls back to the newest session.
pub fn discover(session: &str) -> Option<PathBuf> {
    let root = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".codex")))?;
    let mut matches = vec![];
    fn visit(dir: &Path, suffix: &str, session: &str, matches: &mut Vec<PathBuf>, depth: usize) {
        if depth > 5 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                visit(&path, suffix, session, matches, depth + 1);
            } else if kind.is_file() && entry.file_name().to_string_lossy().ends_with(suffix) {
                let Ok(file) = File::open(&path) else {
                    continue;
                };
                let mut reader = BufReader::new(file);
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if v["type"] == "session_meta"
                    && (v["payload"]["id"] == session || v["payload"]["session_id"] == session)
                {
                    matches.push(path);
                }
            }
        }
    }
    for subdir in ["sessions", "archived_sessions"] {
        visit(
            &root.join(subdir),
            &format!("-{session}.jsonl"),
            session,
            &mut matches,
            0,
        );
    }
    // Ambiguity is safer to expose than to choose a potentially wrong transcript.
    if matches.len() == 1 {
        matches.pop()
    } else {
        None
    }
}
