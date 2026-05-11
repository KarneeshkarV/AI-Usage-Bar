# ai_bar

`ai_bar` is a small Rust CLI that reports AI coding usage for Codex and Claude.
It can run as a Waybar module, show a terminal dashboard, print status snapshots,
and scan local session logs for recent cost.

## Features

- Waybar JSON output with `ok`, `warn`, `crit`, `stale`, and `auth` classes.
- Codex usage windows, reset timing, plan metadata, and credits when available.
- Claude session, weekly, Sonnet weekly, and extra spend data when available.
- Local 30-day cost reports for Codex and Claude JSONL logs.
- Cached snapshots so popups and status checks avoid extra provider calls.
- Ratatui terminal UI for a quick popup view.

## Build

Requirements:

- Rust toolchain with Cargo.
- `codex` installed and logged in if Codex usage is enabled.
- `claude` installed and logged in, or browser cookies available, if Claude
  usage is enabled.

Build the release binary:

```bash
cargo build --release
```

Run it from the repository:

```bash
target/release/ai_bar status --detailed
```

Or install/copy the binary somewhere on your `PATH` and run:

```bash
ai_bar status --detailed
```

## Commands

```bash
ai_bar waybar
```

Runs continuously, polls providers, writes a cached snapshot, and emits Waybar
JSON to stdout.

```bash
ai_bar status
ai_bar status --detailed
```

Prints a terminal status snapshot. It uses the cached Waybar snapshot when it is
fresh and falls back to a one-shot provider poll.

```bash
ai_bar tui
ai_bar tui --poll-secs 5
```

Opens the terminal dashboard. It polls the local cached snapshot while open.

```bash
ai_bar cost
ai_bar cost --provider codex
ai_bar cost --provider claude
```

Scans local session logs and prints a 30-day cost report.

```bash
ai_bar config
ai_bar config --print
ai_bar config --print-waybar-snippet
```

Shows the config path, prints resolved config values, or prints Waybar
integration snippets.

## Configuration

The config file lives at:

```bash
~/.config/ai_bar/config.toml
```

If the file does not exist, defaults are used.

Example:

```toml
[refresh]
interval_secs = 300
cost_refresh_secs = 3600

[providers.codex]
enabled = true
binary = "codex"

[providers.claude]
enabled = true
binary = "claude"
prefer = ["cookies", "pty"]

[display]
merge_text = true
show_cost = true
warn_threshold = 70
crit_threshold = 90
```

The cached snapshot is written under the XDG cache directory:

```bash
~/.cache/ai_bar/snapshot.json
```

## Waybar and Omarchy

For Omarchy-specific setup, see [OMARCHY.md](OMARCHY.md).

Minimal Waybar module:

```jsonc
"custom/ai_bar": {
  "exec": "/absolute/path/to/ai_bar waybar",
  "return-type": "json",
  "on-click": "omarchy-launch-floating-terminal-with-presentation /absolute/path/to/ai_bar tui",
  "tooltip": true
}
```

Add `custom/ai_bar` to the appropriate Waybar module list, then restart Waybar:

```bash
omarchy-restart-waybar
```

## Troubleshooting

Run a detailed status check first:

```bash
ai_bar status --detailed
```

Common causes of missing data:

- The provider CLI is not installed or not on `PATH`.
- The provider CLI is installed but not logged in.
- Claude browser cookies are unavailable or expired.
- Waybar is using a stale binary path after rebuilding or moving the project.

Regenerate the Waybar snippet after moving the binary:

```bash
ai_bar config --print-waybar-snippet
```
