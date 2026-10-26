# Token Profiler

A lightweight, local token profiler for Codex. Activate it with a skill,
continue working, and get a Markdown report with a PNG chart in your project.

Use it to understand session usage, spot unusually large tool results, and
see where visible content enters the conversation.

- **Usage overview:** input, output, cached input, and cache hit ratio when available.
- **Useful rankings:** tools, commands, and conversation turns ranked by observed content size.
- **Focused report:** usage overview, command groups, and expensive calls; tool charts and turns fold into detailed statistics.
- **Local reports:** readable Markdown, static charts, and JSON for further analysis.

Runs as a native binary. No server, API key, or extra runtime to configure.
Short command and prompt excerpts (up to 160 characters) are saved locally to identify hotspots. Full tool outputs are not saved; excerpts may contain sensitive text, so review reports before sharing.

## Install

You need Codex with plugin and hook support, plus **Rust 1.89+** to build. From this project directory:

```sh
./scripts/package.sh
codex plugin marketplace add "$PWD/dist"
codex plugin add token-profiler@token-profiler-local
```

Restart your Codex session and approve the plugin's hooks if prompted.
No manual configuration edits are needed. The build targets your current platform.

## Use in Codex

In the project you want to profile, invoke:

```text
$token-profiler:token-profiler
```

Continue working normally. Reports refresh when Codex finishes a response, on interruption,
and when the session ends normally. Keep the session in the same project directory.

You can also ask Codex:

```text
Show my token profiler report.
Check whether token profiling is collecting events.
Stop token profiling and generate the final report.
Generate a sample token profiler report.
```

You can start midway through a conversation. New event collection begins at activation;
earlier visible content stays in the JSON export. Runtime accounting starts from the first available baseline,
which may be later than activation.

Stopping freezes the capture. To start another capture after stopping, use a new Codex session.

## Read the report

Open `token-profile/latest/report.md` for the most recently updated report. The `latest` symlink points to its session directory under `.token-profiler/`. Its charts are in the adjacent `charts/` directory,
and `data.json` contains the exported measurements. Keep the charts alongside the Markdown when sharing it.

The overview shows runtime usage and collection health. Command groups rank estimated input and result tokens by executable combination. The ten most expensive calls show command excerpts, conversation turns, and total estimates. Expand Detailed statistics for the tool chart and conversation rankings. Full measurements remain in data.json. Older captures may lack excerpts.

Command groups use sorted, distinct executable names: `rg foo | head` and `head file && rg foo` both belong to `head & rg`; repeated `cat` commands belong to `cat`. Arguments do not affect grouping. Each call contributes its input and result estimates to one group only. Names remain literal (`python3` and `python` are separate). Calls without command excerpts fall back to their tool name. Grouping uses the saved excerpts, so truncated commands and complex shell scripts may be incomplete.

The report separates runtime counters from visible-content estimates.
**Content estimates are approximate and do not add up to billed usage.**
Hidden prompts, history replay, and individual commands cannot be fully attributed.
Cache is part of input usage. Missing measurements appear as `Unknown`.

If a report has no captured events, ask Codex to check token profiler collection. The plugin must be installed and its hooks trusted before automatic collection works.

## Update

From this project directory:

```sh
./scripts/package.sh
codex plugin add token-profiler@token-profiler-local
```

Restart the Codex session to load the update.

## License

[MIT](LICENSE). The bundled font has its own [Apache 2.0 notice](assets/DroidSans-LICENSE.txt).
