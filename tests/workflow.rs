use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "token-profiler-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn cli(&self, command: &str) {
        let output = Command::new(env!("CARGO_BIN_EXE_token-profiler"))
            .args([
                "--cwd",
                self.0.to_str().unwrap(),
                "--session",
                "test",
                command,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn hook(&self, mut value: Value) {
        value["cwd"] = json!(self.0);
        value["session_id"] = json!("test");
        let mut child = Command::new(env!("CARGO_BIN_EXE_token-profiler"))
            .arg("hook")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(value.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({})
        );
    }
    fn dir(&self) -> PathBuf {
        self.0.join(".token-profiler/test")
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn usage(path: &Path, input: u64, output: u64) {
    let v = json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":input,"cached_input_tokens":0,"output_tokens":output,"reasoning_output_tokens":0}}}});
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(f, "{v}").unwrap();
}
#[test]
fn collection_is_opt_in_idempotent_and_stops() {
    let w = Workspace::new();
    w.hook(json!({"hook_event_name":"PostToolUse","tool_response":"ignored"}));
    assert!(!w.dir().exists());
    w.cli("start");
    let transcript = w.0.join("rollout.jsonl");
    usage(&transcript, 100, 20);
    w.hook(json!({"hook_event_name":"PreToolUse","transcript_path":transcript,"turn_id":"t1","tool_use_id":"c1","tool_name":"Bash","tool_input":{"command":format!("cargo check {} private-tail", "arg ".repeat(80))}}));
    let result = json!({"hook_event_name":"PostToolUse","turn_id":"t1","tool_use_id":"c1","tool_name":"Bash","tool_response":"x".repeat(20000)});
    w.hook(result.clone());
    w.hook(result);
    usage(&transcript, 1100, 220);
    usage(&transcript, 1100, 220);
    w.hook(json!({"hook_event_name":"Stop","turn_id":"t1"}));
    let data: Value =
        serde_json::from_slice(&fs::read(w.dir().join("data.json")).unwrap()).unwrap();
    assert_eq!(data["sources_estimated_tokens"]["tool_result"], 5000);
    assert_eq!(data["runtime_usage"]["input"], 1000);
    assert_eq!(data["runtime_usage"]["output"], 200);
    let report = fs::read_to_string(w.dir().join("report.md")).unwrap();
    assert!(report.contains("cargo check"));
    assert!(!report.contains("HOT"));
    let bytes = fs::read(w.dir().join("charts/tools.png")).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert!(bytes.len() > 1000);
    let log = fs::read_to_string(w.dir().join("events.jsonl")).unwrap();
    assert!(log.contains("cargo check"));
    assert!(!log.contains("private-tail"));
    assert!(!log.contains(&"x".repeat(100)));
    w.cli("stop");
    w.hook(json!({"hook_event_name":"PostToolUse","tool_response":"ignored"}));
    assert_eq!(
        log,
        fs::read_to_string(w.dir().join("events.jsonl")).unwrap()
    );
}
#[test]
fn parallel_hooks_preserve_all_events() {
    let w = Workspace::new();
    w.cli("start");
    std::thread::scope(|scope| {
        for i in 0..12 {
            let w = &w;
            scope.spawn(move|| w.hook(json!({"hook_event_name":"PostToolUse","tool_use_id":format!("c{i}"),"tool_response":"abcd"})));
        }
    });
    w.cli("report");
    let data: Value =
        serde_json::from_slice(&fs::read(w.dir().join("data.json")).unwrap()).unwrap();
    assert_eq!(data["sources_estimated_tokens"]["tool_result"], 12);
    assert!(data["runtime_usage"].is_null());
}

impl Workspace {
    fn start_with_transcript(&self, path: &Path) {
        let output = Command::new(env!("CARGO_BIN_EXE_token-profiler"))
            .args([
                "--cwd",
                self.0.to_str().unwrap(),
                "--session",
                "test",
                "--transcript",
                path.to_str().unwrap(),
                "start",
            ])
            .env("CODEX_HOME", self.0.join("no-codex-home"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn data(&self) -> Value {
        serde_json::from_slice(&fs::read(self.dir().join("data.json")).unwrap()).unwrap()
    }
}
fn record(path: &Path, v: Value) {
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(f, "{v}").unwrap();
}
#[test]
fn mid_session_inventory_is_separate_and_cache_is_a_subset() {
    let w = Workspace::new();
    let path = w.0.join("transcript.jsonl");
    record(
        &path,
        json!({"type":"session_meta","payload":{"id":"test"}}),
    );
    record(
        &path,
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"old secret"}]}}),
    );
    record(
        &path,
        json!({"type":"response_item","payload":{"type":"function_call_output","output":"x".repeat(4000)}}),
    );
    record(&path, json!({"type":"compacted","payload":{}}));
    usage(&path, 1000, 100);
    w.start_with_transcript(&path);
    record(
        &path,
        json!({"type":"token_usage_record","timestamp":"2026-10-25T05:00:00Z","payload":{"turn_id":"t2","thread_token_usage":{"input_tokens":3000,"cached_input_tokens":1200,"output_tokens":200,"reasoning_output_tokens":0}}}),
    );
    w.hook(json!({"hook_event_name":"PostToolUse","tool_use_id":"new","turn_id":"t2","tool_response":"abcd"}));
    w.cli("report");
    let d = w.data();
    assert_eq!(d["runtime_usage"]["input"], 2000);
    assert_eq!(d["runtime_usage"]["cached"], 1200);
    assert_eq!(d["pre_existing_inventory"]["bytes"]["user_input"], 10);
    assert_eq!(d["pre_existing_inventory"]["bytes"]["tool_result"], 4000);
    assert_eq!(d["pre_existing_inventory"]["compactions"], 1);
    assert_eq!(d["sources_estimated_tokens"]["tool_result"], 1);
    assert_eq!(d["usage_intervals"].as_array().unwrap().len(), 1);
    assert!(
        !fs::read_to_string(w.dir().join("data.json"))
            .unwrap()
            .contains("old secret")
    );
    assert!(
        fs::read_to_string(w.dir().join("report.md"))
            .unwrap()
            .contains("60.0%")
    );
    assert!(!w.dir().join("charts/pre-existing.png").exists());
    w.cli("stop");
    let frozen = w.data()["runtime_usage"].clone();
    usage(&path, 99999, 9999);
    w.cli("report");
    assert_eq!(w.data()["runtime_usage"], frozen);
}
#[test]
fn absent_subset_counters_stay_unknown_and_reset_invalidates_usage() {
    let w = Workspace::new();
    let path = w.0.join("transcript.jsonl");
    record(
        &path,
        json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":10}}}}),
    );
    w.start_with_transcript(&path);
    record(
        &path,
        json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":200,"output_tokens":20}}}}),
    );
    w.cli("report");
    let d = w.data();
    assert_eq!(d["runtime_usage"]["input"], 100);
    assert!(d["runtime_usage"]["cached"].is_null());
    assert!(d["runtime_usage"]["reasoning"].is_null());
    usage(&path, 50, 5);
    w.cli("report");
    assert!(w.data()["runtime_usage"].is_null());
    assert_eq!(w.data()["coverage"]["counter_discontinuity"], true);
    usage(&path, 500, 50);
    w.cli("report");
    assert!(w.data()["runtime_usage"].is_null());
    let d = w.data();
    assert_eq!(d["usage_intervals"][0]["usage"]["input"], 100);
    assert!(d["usage_intervals"][1]["usage"].is_null());
    assert_eq!(d["usage_intervals"][2]["usage"]["input"], 450);
}
#[test]
fn partial_transcript_tail_is_retried_and_partial_event_tail_is_recovered() {
    let w = Workspace::new();
    let path = w.0.join("transcript.jsonl");
    usage(&path, 100, 10);
    w.start_with_transcript(&path);
    let next=json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":200,"cached_input_tokens":0,"output_tokens":20,"reasoning_output_tokens":0}}}}).to_string();
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    let (a, b) = next.split_at(next.len() / 2);
    f.write_all(a.as_bytes()).unwrap();
    w.cli("report");
    assert_eq!(w.data()["runtime_usage"]["input"], 0);
    writeln!(f, "{b}").unwrap();
    w.cli("report");
    assert_eq!(w.data()["runtime_usage"]["input"], 100);
    fs::write(w.dir().join("events.jsonl"), "{\"timestamp_ms\":").unwrap();
    w.hook(
        json!({"hook_event_name":"PostToolUse","tool_use_id":"recovered","tool_response":"abcd"}),
    );
    w.cli("report");
    assert_eq!(w.data()["sources_estimated_tokens"]["tool_result"], 1);
    assert!(
        w.data()["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("incomplete event log"))
    );
}
#[test]
fn truncated_transcript_and_session_end_do_not_fabricate_usage() {
    let w = Workspace::new();
    let path = w.0.join("transcript.jsonl");
    usage(&path, 100, 10);
    usage(&path, 200, 20);
    w.start_with_transcript(&path);
    fs::write(&path, "").unwrap();
    usage(&path, 1000, 100);
    w.hook(json!({"hook_event_name":"SessionEnd"}));
    assert!(w.data()["runtime_usage"].is_null());
    assert_eq!(w.data()["coverage"]["counter_discontinuity"], true);
    assert!(w.data()["coverage"]["stopped_ms"].is_number());
    let log = fs::read(w.dir().join("events.jsonl")).unwrap();
    w.hook(json!({"hook_event_name":"PostToolUse","tool_response":"abcd"}));
    assert_eq!(log, fs::read(w.dir().join("events.jsonl")).unwrap());
}
#[test]
fn status_distinguishes_activation_from_hook_delivery() {
    let w = Workspace::new();
    w.cli("start");
    let status = || {
        let o = Command::new(env!("CARGO_BIN_EXE_token-profiler"))
            .args([
                "--cwd",
                w.0.to_str().unwrap(),
                "--session",
                "test",
                "status",
            ])
            .output()
            .unwrap();
        serde_json::from_slice::<Value>(&o.stdout).unwrap()
    };
    assert_eq!(status()["hook_delivery"], "unconfirmed");
    w.hook(json!({"hook_event_name":"PostToolUse","tool_use_id":"1","tool_response":"abcd"}));
    assert_eq!(status()["hook_delivery"], "observed");
}
#[test]
fn explicit_transcript_with_wrong_session_is_rejected() {
    let w = Workspace::new();
    let path = w.0.join("wrong.jsonl");
    record(
        &path,
        json!({"type":"session_meta","payload":{"id":"another-session"}}),
    );
    let o = Command::new(env!("CARGO_BIN_EXE_token-profiler"))
        .args([
            "--cwd",
            w.0.to_str().unwrap(),
            "--session",
            "test",
            "--transcript",
            path.to_str().unwrap(),
            "start",
        ])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(!w.dir().join("state.json").exists());
}
#[test]
fn missing_baseline_is_delayed_and_missing_transcript_is_stale() {
    let w = Workspace::new();
    let path = w.0.join("empty.jsonl");
    fs::write(&path, "").unwrap();
    w.start_with_transcript(&path);
    assert!(w.data()["runtime_usage"].is_null());
    usage(&path, 100, 10);
    w.cli("report");
    assert_eq!(w.data()["runtime_usage"]["input"], 0);
    usage(&path, 150, 15);
    w.cli("report");
    assert_eq!(w.data()["runtime_usage"]["input"], 50);
    fs::remove_file(&path).unwrap();
    w.cli("report");
    assert_eq!(w.data()["coverage"]["usage_stale"], true);
    assert_eq!(w.data()["runtime_usage"]["input"], 50);
}
#[test]
fn legacy_state_is_readable_without_recounting_earlier_usage() {
    let w = Workspace::new();
    fs::create_dir_all(w.dir()).unwrap();
    let path = w.0.join("legacy.jsonl");
    usage(&path, 20, 2);
    usage(&path, 100, 10);
    usage(&path, 200, 20);
    let baseline = json!({"input":100,"cached":0,"output":10,"reasoning":0});
    fs::write(w.dir().join("state.json"),json!({"enabled":true,"session":"test","transcript":path,"baseline":baseline,"latest":baseline,"warnings":[]}).to_string()).unwrap();
    w.cli("report");
    assert_eq!(w.data()["runtime_usage"]["input"], 100);
    assert_eq!(w.data()["coverage"]["counter_discontinuity"], false);
}
#[test]
fn automatic_discovery_uses_only_the_matching_session() {
    let w = Workspace::new();
    let home = w.0.join("codex-home");
    let sessions = home.join("sessions/2026/10/25");
    fs::create_dir_all(&sessions).unwrap();
    let right = sessions.join("rollout-date-test.jsonl");
    record(
        &right,
        json!({"type":"session_meta","payload":{"id":"test"}}),
    );
    usage(&right, 200, 20);
    let wrong = sessions.join("rollout-date-other.jsonl");
    record(
        &wrong,
        json!({"type":"session_meta","payload":{"id":"other"}}),
    );
    usage(&wrong, 99999, 9999);
    let o = Command::new(env!("CARGO_BIN_EXE_token-profiler"))
        .env("CODEX_HOME", &home)
        .args(["--cwd", w.0.to_str().unwrap(), "--session", "test", "start"])
        .output()
        .unwrap();
    assert!(o.status.success());
    let state: Value =
        serde_json::from_slice(&fs::read(w.dir().join("state.json")).unwrap()).unwrap();
    assert_eq!(state["baseline"]["input"], 200);
    usage(&right, 500, 50);
    w.cli("report");
    assert_eq!(w.data()["runtime_usage"]["input"], 300);
    // Repeated activation refreshes output without replacing the original baseline.
    w.cli("start");
    assert_eq!(w.data()["runtime_usage"]["input"], 300);
}

#[test]
fn demo_has_one_tool_chart_and_labeled_synthetic_data() {
    let w = Workspace::new();
    w.cli("demo");
    let dir = w.0.join(".token-profiler/demo");
    let d: Value = serde_json::from_slice(&fs::read(dir.join("data.json")).unwrap()).unwrap();
    assert_eq!(d["runtime_usage"]["cached"], 8000);
    assert!(dir.join("charts/tools.png").exists());
    assert!(!dir.join("charts/cache.png").exists());
    assert!(
        fs::read_to_string(dir.join("report.md"))
            .unwrap()
            .contains("Synthetic demo data")
    );
}

#[test]
fn regression_call_ids_are_scoped_to_turns() {
    let w = Workspace::new();
    w.cli("start");
    for turn in ["one", "one", "two"] {
        w.hook(json!({"hook_event_name":"PostToolUse","turn_id":turn,"tool_use_id":"reused","tool_response":"abcd"}));
    }
    w.cli("report");
    assert_eq!(w.data()["sources_estimated_tokens"]["tool_result"], 2);
}
#[test]
fn runtime_intervals_remain_complete_in_json_without_a_report_table() {
    let w = Workspace::new();
    let path = w.0.join("intervals.jsonl");
    usage(&path, 100, 10);
    w.start_with_transcript(&path);
    for i in 1..=32 {
        usage(&path, 100 + i * 10, 10 + i);
    }
    w.cli("report");
    let d = w.data();
    assert_eq!(d["usage_intervals"][0]["usage"]["input"], 10);
    let md = fs::read_to_string(w.dir().join("report.md")).unwrap();
    assert_eq!(d["usage_intervals"].as_array().unwrap().len(), 32);
    assert!(!md.contains("| End turn |"));
}
#[test]
fn regression_relative_transcripts_survive_a_different_working_directory() {
    let w = Workspace::new();
    let path = w.0.join("relative.jsonl");
    usage(&path, 100, 10);
    let out = Command::new(env!("CARGO_BIN_EXE_token-profiler"))
        .current_dir(&w.0)
        .args([
            "--session",
            "test",
            "--transcript",
            "relative.jsonl",
            "start",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    usage(&path, 200, 20);
    w.cli("report");
    assert_eq!(w.data()["runtime_usage"]["input"], 100);
    assert_eq!(w.data()["coverage"]["usage_stale"], false);
}
#[test]
fn regression_invalid_usage_records_are_not_silently_fresh() {
    let w = Workspace::new();
    let path = w.0.join("invalid.jsonl");
    usage(&path, 100, 10);
    w.start_with_transcript(&path);
    record(
        &path,
        json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":200,"output_tokens":20,"cached_input_tokens":900}}}}),
    );
    w.cli("report");
    assert_eq!(w.data()["coverage"]["usage_stale"], true);
    w.cli("report");
    assert_eq!(w.data()["coverage"]["usage_stale"], true);
    usage(&path, 300, 30);
    w.cli("report");
    assert_eq!(w.data()["coverage"]["usage_stale"], false);
}
#[test]
fn regression_missing_payload_is_not_a_zero_size_observation() {
    let w = Workspace::new();
    w.cli("start");
    w.hook(json!({"hook_event_name":"PostToolUse","tool_use_id":"missing"}));
    w.cli("report");
    assert!(
        w.data()["sources_estimated_tokens"]
            .get("tool_result")
            .is_none()
    );
    assert!(!w.data()["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn empty_truncation_keeps_a_boundary_until_the_next_counter() {
    let w = Workspace::new();
    let path = w.0.join("rollout.jsonl");
    usage(&path, 100, 10);
    w.start_with_transcript(&path);
    usage(&path, 200, 20);
    w.cli("report");
    fs::write(&path, "").unwrap();
    w.cli("report");
    usage(&path, 300, 30);
    usage(&path, 400, 40);
    w.cli("report");
    let d = w.data();
    assert!(d["runtime_usage"].is_null());
    assert_eq!(d["usage_intervals"][0]["usage"]["input"], 100);
    assert!(d["usage_intervals"][1]["usage"].is_null());
    assert_eq!(d["usage_intervals"][2]["usage"]["input"], 100);
}

#[test]
fn event_ids_survive_duplicates_and_unchanged_charts_are_reused() {
    let w = Workspace::new();
    w.cli("start");
    let event = json!({"hook_event_name":"PostToolUse","turn_id":"t","tool_use_id":"a","tool_response":"abcd"});
    w.hook(event.clone());
    w.cli("report");
    let first = w.data()["events"][0].clone();
    let chart = w.dir().join("charts/tools.png");
    let old = UNIX_EPOCH + std::time::Duration::from_secs(123);
    fs::File::options()
        .write(true)
        .open(&chart)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(old))
        .unwrap();
    w.hook(event);
    w.cli("report");
    assert_eq!(w.data()["events"][0], first);
    assert_eq!(fs::metadata(&chart).unwrap().modified().unwrap(), old);
    w.hook(json!({"hook_event_name":"PostToolUse","turn_id":"t","tool_use_id":"b","tool_response":"abcdefgh"}));
    w.cli("report");
    assert_ne!(fs::metadata(&chart).unwrap().modified().unwrap(), old);
    assert_eq!(w.data()["events"][0]["id"], first["id"]);
    let md = fs::read_to_string(w.dir().join("report.md")).unwrap();
    assert!(md.contains("| Event | Tool |"));
    assert!(!md.contains("| system_prompt | Unknown |"));
    let state: Value =
        serde_json::from_slice(&fs::read(w.dir().join("state.json")).unwrap()).unwrap();
    assert!(state.get("latest").is_none());
}

#[test]
fn waiting_for_a_report_does_not_hold_the_capture_lock() {
    use std::time::{Duration, Instant};
    let w = Workspace::new();
    w.cli("start");
    let render = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(w.dir().join("report.lock"))
        .unwrap();
    render.lock().unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| w.hook(json!({"hook_event_name":"Stop"})));
        let start = Instant::now();
        loop {
            let state: Value =
                serde_json::from_slice(&fs::read(w.dir().join("state.json")).unwrap()).unwrap();
            if state["hook_count"] == 1 {
                break;
            }
            if start.elapsed() > Duration::from_secs(5) {
                drop(render);
                panic!("Stop did not persist capture before waiting for rendering");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let workspace = &w;
        scope.spawn(move || {
            workspace.hook(json!({"hook_event_name":"PostToolUse","tool_use_id":"during-render","tool_response":"abcd"}));
            tx.send(()).unwrap();
        });
        let collected = rx.recv_timeout(Duration::from_secs(3));
        drop(render);
        assert!(collected.is_ok(), "Rendering blocked event collection");
    });
    assert_eq!(w.data()["sources_estimated_tokens"]["tool_result"], 1);
}

#[test]
fn frozen_legacy_latest_is_migrated_without_losing_usage() {
    let w = Workspace::new();
    fs::create_dir_all(w.dir()).unwrap();
    fs::write(
        w.dir().join("state.json"),
        json!({
            "enabled": false, "session": "test", "transcript": null,
            "baseline": {"input": 100,"output": 10,"cached": 0,"reasoning": 0},
            "latest": {"input": 300,"output": 30,"cached": 0,"reasoning": 0},
            "warnings": []
        })
        .to_string(),
    )
    .unwrap();
    w.cli("report");
    assert_eq!(w.data()["runtime_usage"]["input"], 200);
    w.cli("report");
    assert_eq!(w.data()["runtime_usage"]["input"], 200);
    assert_eq!(w.data()["usage_intervals"].as_array().unwrap().len(), 1);
}

#[test]
fn rankings_pair_calls_and_explain_tools_commands_and_turns() {
    let w = Workspace::new();
    w.cli("start");
    w.hook(
        json!({"hook_event_name":"UserPromptSubmit","turn_id":"one","prompt":"Inspect the parser"}),
    );
    for (call, command) in [
        ("shared", "rg parser | head"),
        ("second", "rg tokenizer src"),
    ] {
        w.hook(json!({"hook_event_name":"PreToolUse","turn_id":"one","tool_use_id":call,"tool_name":"Search","tool_input":{"command":command}}));
        let event = json!({"hook_event_name":"PostToolUse","turn_id":"one","tool_use_id":call,"tool_name":"Search","tool_response":"x".repeat(8000)});
        w.hook(event.clone());
        w.hook(event);
    }
    w.hook(json!({"hook_event_name":"UserPromptSubmit","turn_id":"two","prompt":"Run the tests"}));
    w.hook(json!({"hook_event_name":"PreToolUse","turn_id":"two","tool_use_id":"shared","tool_name":"Bash","tool_input":{"command":"cargo test --locked"}}));
    w.hook(json!({"hook_event_name":"PostToolUse","turn_id":"two","tool_use_id":"shared","tool_name":"Bash","tool_response":"x".repeat(12000)}));
    w.cli("report");
    let b = w.data()["breakdown"].clone();
    assert_eq!(b["tools"][0]["name"], "Search");
    assert_eq!(b["tools"][0]["calls"], 2);
    assert_eq!(b["tools"][0]["result"], 4000);
    assert_eq!(b["calls"].as_array().unwrap().len(), 3);
    assert_eq!(b["calls"][0]["summary"], "cargo test --locked");
    assert_eq!(b["calls"][0]["result"], 3000);
    let input = json!({"command":"cargo test --locked"})
        .to_string()
        .len()
        .div_ceil(4);
    assert_eq!(b["calls"][0]["input"], input);
    assert_eq!(b["turns"][0]["prompt"], "Inspect the parser");
    assert_eq!(b["turns"][0]["calls"], 2);
    let sum: u64 = b["turns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["tokens"].as_u64().unwrap())
        .sum();
    assert_eq!(b["total"], sum);
    let report = fs::read_to_string(w.dir().join("report.md")).unwrap();
    assert!(report.contains("**Largest tool:** Search"));
    assert!(report.contains("cargo test --locked"));
    assert!(report.contains("rg parser &#124; head"));
    assert!(report.contains("Inspect the parser"));
    assert!(w.dir().join("charts/tools.png").exists());
}

#[test]
fn missing_ids_are_not_paired_and_missing_inputs_stay_unknown() {
    let w = Workspace::new();
    w.cli("start");
    // A prompt without a turn ID belongs to the following observed turn.
    w.hook(json!({"hook_event_name":"UserPromptSubmit","prompt":"Find the relevant files"}));
    w.hook(json!({"hook_event_name":"PostToolUse","turn_id":"one","tool_use_id":"result-only","tool_name":"Bash","tool_response":"abcd"}));
    w.hook(json!({"hook_event_name":"PreToolUse","turn_id":"one","tool_name":"Bash","tool_input":{"command":"pwd"}}));
    w.hook(json!({"hook_event_name":"PostToolUse","turn_id":"one","tool_name":"Bash","tool_response":"abcd"}));
    w.cli("report");
    let b = w.data()["breakdown"].clone();
    assert_eq!(b["calls"].as_array().unwrap().len(), 3);
    assert_eq!(b["turns"].as_array().unwrap().len(), 1);
    assert_eq!(b["turns"][0]["prompt"], "Find the relevant files");
    let result_only = b["calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["call"] == "result-only")
        .unwrap();
    assert!(result_only["input"].is_null());
    assert_eq!(result_only["result"], 1);
    assert_eq!(result_only["summary"], "");
}

#[test]
fn excerpts_are_bounded_unicode_and_do_not_store_tool_outputs() {
    let w = Workspace::new();
    w.cli("start");
    w.hook(json!({"hook_event_name":"UserPromptSubmit","turn_id":"one","prompt":format!("Fix the parser\n{}", "文".repeat(300))}));
    w.hook(json!({"hook_event_name":"PostToolUse","turn_id":"one","tool_use_id":"a","tool_name":"exec","tool_input":{"cmd":format!("rg {} PRIVATE_TAIL", "文".repeat(300))},"tool_response":"OUTPUT_BODY_DO_NOT_SAVE"}));
    w.cli("report");
    let data = w.data();
    for event in data["events"].as_array().unwrap() {
        let text = event["summary"].as_str().unwrap();
        assert!(text.chars().count() <= 160);
        assert!(!text.contains('\n'));
        assert!(!text.contains("PRIVATE_TAIL"));
    }
    for name in ["events.jsonl", "data.json", "report.md"] {
        let text = fs::read_to_string(w.dir().join(name)).unwrap();
        assert!(!text.contains("OUTPUT_BODY_DO_NOT_SAVE"));
        assert!(!text.contains("PRIVATE_TAIL"));
    }
}

#[test]
fn compact_report_has_only_useful_sections_and_retires_old_charts() {
    let w = Workspace::new();
    w.cli("start");
    w.hook(
        json!({"hook_event_name":"UserPromptSubmit","turn_id":"one","prompt":"Inspect the parser"}),
    );
    w.hook(json!({"hook_event_name":"PostToolUse","turn_id":"one","tool_use_id":"a","tool_name":"Bash","tool_input":{"cmd":"rg parser src"},"tool_response":"x".repeat(10000)}));
    let charts = w.dir().join("charts");
    for name in ["sources", "hotspots", "turns", "pre-existing", "cache"] {
        for extension in ["png", "inputs.json"] {
            fs::write(charts.join(format!("{name}.{extension}")), "legacy output").unwrap();
        }
    }
    w.cli("report");
    let report = fs::read_to_string(w.dir().join("report.md")).unwrap();
    let sections: Vec<_> = report
        .lines()
        .filter(|line| line.starts_with("## "))
        .collect();
    assert_eq!(
        sections,
        [
            "## Overview",
            "## Tools",
            "## Commands and tool calls",
            "## Conversation turns"
        ]
    );
    assert_eq!(report.matches("![").count(), 1);
    assert!(report.contains("rg parser src"));
    assert!(report.contains("Inspect the parser"));
    assert!(!report.contains("HOT"));
    assert!(!report.contains("Unix ms"));
    for name in ["sources", "hotspots", "turns", "pre-existing", "cache"] {
        assert!(!charts.join(format!("{name}.png")).exists());
        assert!(!charts.join(format!("{name}.inputs.json")).exists());
    }
    assert!(w.data().get("coverage").is_some());
    assert!(w.data().get("pre_existing_inventory").is_some());
    assert!(w.data().get("usage_intervals").is_some());
}

#[test]
fn empty_capture_is_actionable_without_empty_rankings_or_charts() {
    let w = Workspace::new();
    w.cli("start");
    let report = fs::read_to_string(w.dir().join("report.md")).unwrap();
    assert!(report.contains("No content captured yet"));
    assert!(report.contains("Hook delivery unconfirmed"));
    assert!(!report.contains("## Tools"));
    assert!(!report.contains("!["));
    assert!(!w.dir().join("charts/tools.png").exists());
}

#[cfg(unix)]
#[test]
fn latest_link_follows_report_updates_and_survives_moving_the_project() {
    let mut w = Workspace::new();
    w.cli("start");
    let latest = w.0.join("token-profile/latest");
    assert!(
        fs::symlink_metadata(&latest)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(fs::read_link(&latest).unwrap().is_relative());
    assert_eq!(
        fs::canonicalize(&latest).unwrap(),
        fs::canonicalize(w.dir()).unwrap()
    );
    w.cli("demo");
    assert_eq!(
        fs::read_link(&latest).unwrap(),
        PathBuf::from("../.token-profiler/demo")
    );
    assert!(latest.join("charts/tools.png").is_file());
    let linked: Value =
        serde_json::from_slice(&fs::read(latest.join("data.json")).unwrap()).unwrap();
    assert_eq!(linked["session"], "demo");
    w.cli("report");
    assert_eq!(
        fs::canonicalize(&latest).unwrap(),
        fs::canonicalize(w.dir()).unwrap()
    );
    // An interrupted publication may leave a dangling temporary link.
    std::os::unix::fs::symlink("missing", w.0.join("token-profile/.latest.tmp")).unwrap();
    w.cli("report");
    assert!(!w.0.join("token-profile/.latest.tmp").try_exists().unwrap());
    let moved = w.0.with_extension("moved");
    fs::rename(&w.0, &moved).unwrap();
    w.0 = moved;
    let latest = w.0.join("token-profile/latest");
    assert_eq!(
        fs::canonicalize(latest.join("report.md")).unwrap(),
        fs::canonicalize(w.dir().join("report.md")).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn latest_link_never_overwrites_a_regular_file_or_directory() {
    let w = Workspace::new();
    w.cli("start");
    let latest = w.0.join("token-profile/latest");
    fs::remove_file(&latest).unwrap();
    fs::write(&latest, "user content").unwrap();
    let report = || {
        Command::new(env!("CARGO_BIN_EXE_token-profiler"))
            .args([
                "--cwd",
                w.0.to_str().unwrap(),
                "--session",
                "test",
                "report",
            ])
            .output()
            .unwrap()
    };
    let output = report();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Refusing to replace a non-symlink"));
    assert_eq!(fs::read_to_string(&latest).unwrap(), "user content");
    fs::remove_file(&latest).unwrap();
    fs::create_dir(&latest).unwrap();
    assert!(!report().status.success());
    assert!(latest.is_dir());
}
