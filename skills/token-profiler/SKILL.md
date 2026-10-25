---
name: token-profiler
description: Enable, inspect, or stop local Codex token profiling and generate a Markdown report with a tool ranking chart. Use when the user asks to profile this session or investigate its token usage.
---

Handle profiling through this skill; users do not need to run terminal commands. Use the bundled `bin/token-profiler` executable, located two directories above this skill directory. Resolve it from this skill's absolute path; do not assume the user's project contains it.

Choose the internal action from the user's request:

| Request | Action |
|---|---|
| Enable profiling | `start`, then `status` |
| Check collection | `status` |
| View results | `status`, then `report` |
| Stop profiling | `stop` |
| Preview an example | `demo`, only when requested; label it synthetic |

Run these commands yourself and summarize the result. `hook` is reserved for automatic hook delivery. A stopped capture cannot restart within the same session; explain that a new session is needed.

For activation, run `<plugin-root>/bin/token-profiler start` in the user's project. The CLI reads `CODEX_THREAD_ID`; if unavailable, pass `--session` only when the actual session ID is known. Never invent an ID, because automatic hooks must match it. If unavailable, explain the limitation and use `demo` only if requested.

After activation, run `status` once and tell the user whether hook delivery is observed or unconfirmed. Zero deliveries immediately after activation do not prove a failure; later invocations can check again. Do not claim automatic collection is working without observed delivery. The CLI discovers only the exact session's transcript under `CODEX_HOME` or `~/.codex`; `--transcript` is an optional explicit override.

After activation, continue the user's work normally. Installed and trusted hooks record events and update the report at Stop, Interrupt, and SessionEnd. Do not wrap tools, add per-command logging calls, read entire transcripts into model context, or load reports automatically after every tool call.

When asked to inspect results, run `status` to check delivery, then `report`, then read the emitted report path. Use the tool, paired-call, and conversation rankings to identify concrete contributors. Include command/prompt excerpts and observed input/result sizes when explaining hotspots. Older events may lack excerpts; do not invent them. Link the Markdown report and tool ranking chart when useful. The report contains only an overview and tool, command, and conversation rankings; diagnostics and earlier-content inventory are in `data.json`. For explicit stop requests, run `stop`. Commands accept `--cwd <project>` and `--session <id>`.

Reports live under `.token-profiler/<session-id>/`. Each successful report update points `token-profile/latest` at that session directory; `token-profile/latest/report.md` is the project-wide latest report, which may belong to another session. Use the emitted session report path when inspecting this session. Distinguish runtime counters from byte-based content estimates. The pre-existing transcript inventory is archived visible material, not active context size or capture-period usage. Cache is an input subset, never a content source. Sampling boundaries may differ from event collection. Missing counters and discontinuities remain unknown. System prompt, tool schemas, and history replay are unknown; RAG is not separately classified. Code-mode nested results may not all reach the model. Missing counters are not zero. Do not infer dollars or exact per-command billed tokens.

If hooks are missing, collection cannot be made automatic by invoking the skill alone. Explain that the plugin must be installed and its hooks trusted; do not silently edit global Codex configuration. Reports and event files are local; command and prompt excerpts of up to 160 characters are saved to identify hotspots; full tool outputs are not persisted. Excerpts may contain sensitive text.
