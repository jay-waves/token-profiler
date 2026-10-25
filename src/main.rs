use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::{fs, io::Read, path::PathBuf};

mod breakdown;
mod capture;
mod chart;
mod model;
mod report;
mod storage;
mod transcript;
use capture::State;
use model::{Event, field, now};
use report::{report, report_current};
use storage::{append, lock, session_dir, state, write_json};

#[derive(Parser)]
#[command(
    version,
    about = "Internal runner for the token-profiler skill and hooks"
)]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    cwd: PathBuf,
    #[arg(long, global = true, env = "CODEX_THREAD_ID")]
    session: Option<String>,
    /// Explicit transcript path; normally discovered automatically.
    #[arg(long, global = true)]
    transcript: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Enable profiling for the current Codex session.
    Start,
    /// Show activation, hook delivery, transcript and capture health.
    Status,
    /// Read a Codex hook JSON object from stdin (advisory, no model feedback).
    #[command(hide = true)]
    Hook,
    /// Refresh Markdown and PNG reports.
    Report,
    /// Generate a final report and stop collecting.
    Stop,
    /// Generate an example report using synthetic data.
    #[command(hide = true)]
    Demo,
}
fn main() -> Result<()> {
    run(Cli::parse())
}
fn run(cli: Cli) -> Result<()> {
    if matches!(cli.command, Command::Hook) {
        return run_hook();
    }
    let demo = matches!(cli.command, Command::Demo);
    let session = if demo {
        "demo".into()
    } else {
        cli.session
            .context("Pass --session <Codex session ID> or set CODEX_THREAD_ID")?
    };
    let dir = session_dir(&fs::canonicalize(&cli.cwd)?, &session)?;
    if !matches!(cli.command, Command::Start | Command::Demo) && !dir.join("state.json").exists() {
        bail!("Session not started; invoke the token-profiler skill first");
    }
    let guard = lock(&dir)?;
    match cli.command {
        Command::Start => {
            let mut capture = if dir.join("state.json").exists() {
                let s = state(&dir)?;
                if s.enabled {
                    s
                } else {
                    bail!(
                        "Session already stopped; use a new session to avoid mixing capture windows"
                    );
                }
            } else {
                State::new(session)
            };
            capture.connect(cli.transcript)?;
            capture.refresh();
            write_json(&dir.join("state.json"), &capture)?;
            drop(guard);
            report_current(&dir)?;
            println!("{}", dir.join("report.md").display());
        }
        Command::Status => {
            let s = state(&dir)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "enabled":s.enabled,"session":s.session,
                    "hook_delivery":if s.hook_count==0 {"unconfirmed"}else{"observed"},
                    "hook_count":s.hook_count,"last_hook_ms":s.last_hook_ms,
                    "transcript_attached":s.transcript.is_some(),"baseline_available":s.baseline.is_some(),
                    "counter_discontinuity":s.counter_discontinuity,"usage_stale":s.usage_stale,
                    "report":dir.join("report.md")
                }))?
            );
        }
        Command::Report | Command::Stop => {
            let mut s = state(&dir)?;
            if s.enabled {
                s.connect(cli.transcript)?;
                s.refresh();
            }
            if matches!(cli.command, Command::Stop) && s.enabled {
                s.enabled = false;
                s.stopped_ms = Some(now());
            }
            write_json(&dir.join("state.json"), &s)?;
            drop(guard);
            report_current(&dir)?;
            println!("{}", dir.join("report.md").display());
        }
        Command::Demo => {
            fs::write(dir.join("events.jsonl"), "")?;
            for (kind, turn, call, tool, key, len) in [
                ("UserPromptSubmit", "turn-1", "", "", "prompt", 400),
                (
                    "PostToolUse",
                    "turn-1",
                    "call-1",
                    "Bash",
                    "tool_response",
                    24000,
                ),
                (
                    "PostToolUse",
                    "turn-2",
                    "call-2",
                    "mcp__search",
                    "tool_response",
                    8000,
                ),
            ] {
                let mut input = json!({"hook_event_name":kind,"turn_id":turn,"tool_use_id":call,"tool_name":tool});
                input[key] = Value::String("x".repeat(len));
                if kind == "UserPromptSubmit" {
                    input["prompt"] =
                        json!("Find the files and docs needed to explain token usage.");
                } else {
                    input["tool_input"] = if tool == "Bash" {
                        json!({"command": "rg --files ."})
                    } else {
                        json!({"query": "Codex runtime token usage"})
                    };
                    let mut pre = input.clone();
                    pre["hook_event_name"] = json!("PreToolUse");
                    if let Some(event) = Event::from_hook(&pre) {
                        append(&dir, &event)?;
                    }
                }
                if let Some(event) = Event::from_hook(&input) {
                    append(&dir, &event)?;
                }
            }
            let s = State::demo();
            let es = storage::events(&dir)?;
            drop(guard);
            let _render = storage::named_lock(&dir, "report.lock")?;
            report(&dir, &s, &es)?;
            println!("{}", dir.join("report.md").display());
        }
        Command::Hook => unreachable!(),
    }
    Ok(())
}
fn run_hook() -> Result<()> {
    let mut input = String::new();
    let result = std::io::stdin()
        .read_to_string(&mut input)
        .context("Could not read hook input")
        .and_then(|_| serde_json::from_str(&input).context("Invalid hook JSON"))
        .and_then(handle_hook);
    if let Err(error) = result {
        eprintln!("token-profiler: {error:#}");
    }
    // Codex hooks are advisory: never block or add context.
    println!("{{}}");
    Ok(())
}
fn handle_hook(input: Value) -> Result<()> {
    let dir = session_dir(
        PathBuf::from(field(&input, "cwd")).as_path(),
        field(&input, "session_id"),
    )?;
    if !dir.join("state.json").exists() {
        return Ok(());
    }
    let guard = lock(&dir)?;
    let mut s = state(&dir)?;
    let regenerate = s.on_hook(&dir, &input)?;
    write_json(&dir.join("state.json"), &s)?;
    drop(guard);
    if regenerate {
        report_current(&dir)?;
    }
    Ok(())
}
