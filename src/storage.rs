//! Local capture persistence: events.jsonl stores sizes and bounded labels;
//! state.json stores capture boundaries, transcript progress and counter snapshots.
//! Callers hold the capture lock for reads/writes. Rendered reports use a separate
//! lock and replace each output atomically; the output set is not one transaction.
use crate::{Result, model::Event};
use anyhow::{Context, bail};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub(crate) fn session_dir(cwd: &Path, session: &str) -> Result<PathBuf> {
    if session.is_empty()
        || session.len() > 160
        || !session
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        bail!("Invalid session ID; use letters, digits, underscores or hyphens");
    }
    Ok(cwd.join(".token-profiler").join(session))
}
pub(crate) fn lock(dir: &Path) -> Result<File> {
    named_lock(dir, "lock")
}
pub(crate) fn named_lock(dir: &Path, name: &str) -> Result<File> {
    fs::create_dir_all(dir)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(name))?;
    file.lock()?;
    Ok(file)
}
pub(crate) fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    atomic_write(path, &serde_json::to_vec_pretty(value)?)
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)?;
    Ok(())
}
pub(crate) fn state(dir: &Path) -> Result<crate::capture::State> {
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("state.json"))?)?;
    let legacy = value.get("pending_boundary").is_none();
    let latest = value.get_mut("latest").map(serde_json::Value::take);
    let mut state: crate::capture::State = serde_json::from_value(value)?;
    // Read the old redundant latest field once; new states derive it from points.
    if let Some(latest) = latest.filter(|v| !v.is_null()) {
        let total = serde_json::from_value(latest)?;
        if state.latest() != Some(&total) {
            state.usage_points.push(crate::transcript::UsagePoint {
                turn: String::new(),
                timestamp: String::new(),
                total,
                break_before: state.counter_discontinuity,
            });
        }
    }
    if legacy && state.counter_discontinuity {
        // Legacy captures did not locate gaps. Do not invent valid old intervals.
        for point in &mut state.usage_points {
            point.break_before = true;
        }
        state.pending_boundary = true;
    }
    Ok(state)
}
pub(crate) fn append(dir: &Path, event: &Event) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("events.jsonl"))?;
    serde_json::to_writer(&mut file, event)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}
/// Deduplicate by kind, turn and call ID. Keep the first log position and time
/// as the stable event identity, but use the newest delivered payload size.
pub(crate) fn events(dir: &Path) -> Result<Vec<Event>> {
    let path = dir.join("events.jsonl");
    if !path.exists() {
        return Ok(vec![]);
    }
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = String::new();
    let mut events = Vec::new();
    let mut seen = HashMap::<(String, String, String), usize>::new();
    let mut sequence = 0;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || !line.ends_with('\n') {
            break;
        }
        let mut event: Event =
            serde_json::from_str(&line).context("Invalid complete event record")?;
        sequence += 1;
        event.id = sequence;
        if event.call.is_empty() {
            events.push(event);
            continue;
        }
        let key = (event.kind.clone(), event.turn.clone(), event.call.clone());
        if let Some(index) = seen.get(&key) {
            event.id = events[*index].id;
            event.timestamp_ms = events[*index].timestamp_ms;
            events[*index] = event;
        } else {
            seen.insert(key, events.len());
            events.push(event);
        }
    }
    Ok(events)
}
/// Remove an unfinished append before the next hook writes to the JSONL log.
pub(crate) fn recover_event_tail(dir: &Path) -> Result<bool> {
    let path = dir.join("events.jsonl");
    if !path.exists() {
        return Ok(false);
    }
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0];
    file.read_exact(&mut last)?;
    if last[0] == b'\n' {
        return Ok(false);
    }
    let mut end = len;
    while end > 0 {
        let start = end.saturating_sub(8192);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = vec![0; (end - start) as usize];
        file.read_exact(&mut bytes)?;
        if let Some(pos) = bytes.iter().rposition(|b| *b == b'\n') {
            file.set_len(start + pos as u64 + 1)?;
            return Ok(true);
        }
        end = start;
    }
    file.set_len(0)?;
    Ok(true)
}
