# AI Usage Bar

AI Usage Bar is a small Rust CLI that reports AI coding usage for Codex, Claude,
Grok, Cursor, and OpenCode. It can run as a Waybar module, show a terminal
dashboard, print status snapshots, and scan local session logs for recent cost.

## Features

- Waybar JSON output with `ok`, `warn`, `crit`, `stale`, `auth`, and `incident` classes.
- Codex usage windows, reset timing, plan metadata, and credits when available.
- Claude session, weekly, Sonnet weekly, and extra spend data when available.
- Grok subscription usage via the `grok` CLI (auto-detected from `~/.grok`).
- Cursor plan usage via a manually configured session cookie.
- OpenCode Zen balance and local 30-day spend (auto-detected from
  `~/.local/share/opencode`).
- Reset times as a countdown (`resets in 2h 14m`) or absolute local time
  (`resets tomorrow, 09:05`), configurable per taste.
- Usage pace per window: expected vs actual burn, `X% in reserve` /
  `X% over budget`, and a projected run-out time.
- Reset-time backfill from the cached snapshot when a fetch omits it.
- Provider status-page polling with incident lines in the tooltip.
- Desktop notifications (`notify-send`) on warn/crit crossings and window resets.
- Local 30-day cost reports for Codex and Claude JSONL logs, with daily
  sparklines in the TUI and detailed status.
- Cached snapshots so popups and status checks avoid extra provider calls.
- Ratatui terminal UI for a quick popup view.
- Confetti when the weekly quota resets. Yes, really.

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

## Lint and Format

Format the codebase:

```bash
cargo fmt --all
```

Check formatting without changing files:

```bash
cargo fmtcheck
```

Run Clippy with warnings treated as errors:

```bash
cargo lint
```

Run the test suite:

```bash
cargo ci
```

Run it from the repository:

```bash
target/release/ai-usage-bar status --detailed
```

Or install/copy the binary somewhere on your `PATH` and run:

```bash
ai-usage-bar status --detailed
```

## Commands

```bash
ai-usage-bar waybar
```

Runs continuously, polls providers, writes a cached snapshot, and emits Waybar
JSON to stdout.

```bash
ai-usage-bar status
ai-usage-bar status --detailed
```

Prints a terminal status snapshot. It uses the cached Waybar snapshot when it is
fresh and falls back to a one-shot provider poll.

```bash
ai-usage-bar tui
ai-usage-bar tui --poll-secs 5
```

Opens the terminal dashboard. It polls the local cached snapshot while open.

```bash
ai-usage-bar cost
ai-usage-bar cost --provider codex
ai-usage-bar cost --provider claude
```

Scans local session logs and prints a 30-day cost report.

```bash
ai-usage-bar config
ai-usage-bar config --print
ai-usage-bar config --print-waybar-snippet
```

Shows the config path, prints resolved config values, or prints Waybar
integration snippets.

## Configuration

The config file lives at:

```bash
~/.config/ai-usage-bar/config.toml
```

If the file does not exist, defaults are used.

Example:

```toml
[refresh]
# Either pick a preset ("fast" = 60s/900s, "normal" = 300s/3600s,
# "slow" = 900s/7200s) or set the intervals explicitly; explicit values
# override the preset field by field.
preset = "normal"
interval_secs = 300
cost_refresh_secs = 3600

[providers.codex]
enabled = true
binary = "codex"

[providers.claude]
enabled = true
binary = "claude"
prefer = ["cookies", "pty"]

# The three providers below use mode = "auto" | "on" | "off".
# "auto" (the default) activates only when credentials are detected, so an
# unconfigured provider is omitted from the bar instead of showing an error.

[providers.grok]
mode = "auto"            # active when ~/.grok/auth.json exists ($GROK_HOME honored)
# binary = "grok"

[providers.cursor]
mode = "auto"            # active when a session cookie is configured
# Raw Cookie header from cursor.com, e.g. "WorkosCursorSessionToken=...".
# Can also be supplied via the AI_USAGE_BAR_CURSOR_COOKIE env var.
# cookie = "WorkosCursorSessionToken=..."

[providers.opencode]
mode = "auto"            # active when ~/.local/share/opencode/auth.json has a key

[display]
merge_text = true
show_cost = true
warn_threshold = 70
crit_threshold = 90
reset_style = "countdown"  # or "absolute": "resets 14:30" / "resets tomorrow, 09:05"
confetti = true            # celebrate weekly quota resets for 10 minutes
# Limit which providers appear in the Waybar text (tooltip always shows all):
# bar_providers = ["codex", "claude"]

[status]
enabled = true             # poll provider status pages for incidents

[notify]
enabled = true             # notify-send on warn/crit threshold crossings
on_reset = true            # and when a window resets
```

The cached snapshot is written under the XDG cache directory:

```bash
~/.cache/ai-usage-bar/snapshot.json
```

## Waybar and Omarchy

For Omarchy-specific setup, see [OMARCHY.md](OMARCHY.md).

Minimal Waybar module:

```jsonc
"custom/ai-usage-bar": {
  "exec": "/absolute/path/to/ai-usage-bar waybar",
  "return-type": "json",
  "on-click": "omarchy-launch-floating-terminal-with-presentation /absolute/path/to/ai-usage-bar tui",
  "tooltip": true
}
```

Add `custom/ai-usage-bar` to the appropriate Waybar module list, then restart Waybar:

```bash
omarchy-restart-waybar
```

## Troubleshooting

Run a detailed status check first:

```bash
ai-usage-bar status --detailed
```

Common causes of missing data:

- The provider CLI is not installed or not on `PATH`.
- The provider CLI is installed but not logged in.
- Claude browser cookies are unavailable or expired.
- Waybar is using a stale binary path after rebuilding or moving the project.

Regenerate the Waybar snippet after moving the binary:

```bash
ai-usage-bar config --print-waybar-snippet
```
