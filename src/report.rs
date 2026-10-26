use crate::chart::bars;
use crate::{Result, breakdown::Breakdown, capture::State, chart, model::Event, storage};
use serde_json::json;
use std::{fs, path::Path};

fn md(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('`', "\\`")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace(['\n', '\r'], " ")
}
fn metric(value: Option<u64>) -> String {
    value
        .map(|n| n.to_string())
        .unwrap_or_else(|| "Unknown".into())
}
pub fn report_current(dir: &Path) -> Result<()> {
    // Serialize renders before taking the snapshot, so an older render cannot win.
    let _render = storage::named_lock(dir, "report.lock")?;
    let capture = storage::lock(dir)?;
    let s = storage::state(dir)?;
    let es = storage::events(dir)?;
    drop(capture);
    report(dir, &s, &es)
}
pub fn report(dir: &Path, s: &State, es: &[Event]) -> Result<()> {
    let breakdown = Breakdown::new(es);
    let usage = s.usage();
    let charts = dir.join("charts");
    fs::create_dir_all(&charts)?;
    // Retire charts produced by earlier versions, including rendering caches.
    for name in ["sources", "hotspots", "turns", "pre-existing", "cache"] {
        remove_chart(&charts, name)?;
    }
    let mut text = format!(
        "# Token Profiler\n\n## Overview\n\n{} · {} observed tool calls · {} conversation turns\n\n",
        if s.session == "demo" {
            "Synthetic demo data"
        } else if s.enabled {
            "Collecting"
        } else {
            "Stopped"
        },
        breakdown.calls.len(),
        breakdown.turns.len()
    );
    let hit = usage.as_ref().and_then(|u| {
        u.cached
            .filter(|_| u.input > 0)
            .map(|n| share(n as usize, u.input as usize))
    });
    text += &format!(
        "| Input | Output | Cached input | Cache hit |\n|---:|---:|---:|---:|\n| {} | {} | {} | {} |\n",
        metric(usage.as_ref().map(|u| u.input)),
        metric(usage.as_ref().map(|u| u.output)),
        metric(usage.as_ref().and_then(|u| u.cached)),
        hit.unwrap_or_else(|| "Unknown".into())
    );
    if let Some(reasoning) = usage.as_ref().and_then(|u| u.reasoning) {
        text += &format!("\nReasoning: {reasoning} tokens (included in output).\n");
    }
    if s.hook_count == 0 {
        text += "\n**Hook delivery unconfirmed.** Check that the plugin is installed and its hooks are trusted.\n";
    }
    if s.usage_stale {
        text += "\n**Runtime counters may be stale.**\n";
    }
    if s.counter_discontinuity {
        text += "\n**Runtime total incomplete:** an accounting gap was detected.\n";
    } else if usage.is_none() {
        text += "\nRuntime counters unavailable.\n";
    }
    text += "\nRankings show estimated new content captured since activation; cache is included in runtime input. Full measurements and diagnostics: [data.json](data.json).\n";
    if es.iter().all(|e| e.source.is_none()) {
        text += "\nNo content captured yet. Rankings will appear after hooks record prompts and tool calls.\n";
    } else {
        text += &rankings(&breakdown, &charts)?;
    }
    if breakdown.tools.is_empty() {
        remove_chart(&charts, "tools")?;
    }
    // Keep diagnostic/export detail out of the human report.
    let mut sources = std::collections::BTreeMap::<String, usize>::new();
    for e in es {
        if let Some(source) = &e.source {
            let total = sources.entry(source.clone()).or_default();
            *total = total.saturating_add(e.estimated_tokens());
        }
    }
    let intervals = s.intervals();
    let serialized_events=es.iter().map(|e|json!({
        "id":format!("E{}", e.id),"timestamp_ms":e.timestamp_ms,"kind":e.kind,"turn":e.turn,"call":e.call,"tool":e.tool,
        "source":e.source,"bytes":e.bytes,
        "estimated_tokens":e.source.as_ref().map(|_|e.estimated_tokens()),"summary":e.summary
    })).collect::<Vec<_>>();
    storage::write_json(
        &dir.join("data.json"),
        &json!({
            "schema_version":5,"breakdown":breakdown,"session":s.session,"runtime_usage":usage,
            "sources_estimated_tokens":sources,"events":serialized_events,"warnings":s.warnings,
            "pre_existing_inventory":s.inventory,
            "coverage":{"started_ms":(s.started_ms!=0).then_some(s.started_ms),"baseline_ms":s.baseline_ms,
                "stopped_ms":s.stopped_ms,"hook_count":s.hook_count,"last_hook_ms":s.last_hook_ms,
                "counter_discontinuity":s.counter_discontinuity,"usage_stale":s.usage_stale},
            "usage_intervals":intervals
        }),
    )?;
    storage::publish_report(dir, text.as_bytes())?;
    Ok(())
}

fn share(tokens: usize, total: usize) -> String {
    if total == 0 {
        "—".into()
    } else {
        format!("{:.1}%", tokens as f64 / total as f64 * 100.)
    }
}
fn label(value: &str) -> String {
    if value.is_empty() {
        "Not captured".into()
    } else {
        md(value)
    }
}
fn rankings(b: &Breakdown, charts: &Path) -> Result<String> {
    let mut text = String::from("\n## Command groups\n\n");
    if b.categories.is_empty() {
        text += "No tool calls captured.\n";
    } else {
        text += "| Commands | Calls | Input est. | Result est. |\n|---|---:|---:|---:|\n";
        for group in b.categories.iter().take(10) {
            text += &format!(
                "| {} | {} | {} | {} |\n",
                md(&group.name),
                group.calls,
                group.input,
                group.result
            );
        }
        if b.categories.len() > 10 {
            text += &format!(
                "\nTop 10 of {} groups; all groups are in [data.json](data.json).\n",
                b.categories.len()
            );
        }
    }
    text += "\n## Most expensive calls\n\n";
    if b.calls.is_empty() {
        text += "No tool calls captured.\n";
    } else {
        text += "| Command / input excerpt | Turn | Total est. |\n|---|---|---:|\n";
        for call in b.calls.iter().take(10) {
            let excerpt = if call.summary.is_empty() {
                &call.tool
            } else {
                &call.summary
            };
            text += &format!(
                "| {} | {} | {} |\n",
                label(excerpt),
                call.turn,
                call.total()
            );
        }
        text += "\nTop 10 by estimated input + result tokens. Missing measurements contribute no known tokens; see data.json for individual measurements.\n";
    }
    text += "\n<details>\n<summary>Detailed statistics: tools and conversation turns</summary>\n\n";
    if !b.tools.is_empty() {
        chart::init()?;
        bars(
            &charts.join("tools.png"),
            "Tools by estimated new content",
            &b.tools
                .iter()
                .take(10)
                .map(|t| (t.name.clone(), t.input.saturating_add(t.result)))
                .collect::<Vec<_>>(),
        )?;
        text += "![Tools](charts/tools.png)\n\n";
    }
    text += "### Conversation turns\n\n| Turn | Prompt excerpt | Tool calls | Content est. | Share |\n|---|---|---:|---:|---:|\n";
    for turn in b.turns.iter().take(10) {
        text += &format!(
            "| {} | {} | {} | {} | {} |\n",
            turn.label,
            label(&turn.prompt),
            turn.calls,
            turn.tokens,
            share(turn.tokens, b.total)
        );
    }
    text += "\n</details>\n";
    Ok(text)
}
fn remove_chart(charts: &Path, name: &str) -> Result<()> {
    for extension in ["png", "inputs.json"] {
        match fs::remove_file(charts.join(format!("{name}.{extension}"))) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
