# Omarchy usage guide

`ai_bar` is a Waybar module for Omarchy that shows AI coding usage for Codex
and Claude, plus a local 30-day cost scan from session logs. It is designed to
run as a long-lived Waybar process and open a small terminal UI when clicked.

## Build

From this repository:

```bash
cargo build --release
```

The binary will be available at:

```bash
target/release/ai_bar
```

Use an absolute path in Waybar so the module works after login. For this clone,
that path is:

```bash
/home/karneeshkar/Desktop/personal/ai_bar/target/release/ai_bar
```

## Waybar module

Print the current integration snippet:

```bash
target/release/ai_bar config --print-waybar-snippet
```

Add the module name to `modules-right` in `~/.config/waybar/config.jsonc`:

```jsonc
"modules-right": ["custom/ai_bar", "memory", "cpu", "tray", "clock"]
```

Add the module definition in the same Waybar config file:

```jsonc
"custom/ai_bar": {
  "exec": "/home/karneeshkar/Desktop/personal/ai_bar/target/release/ai_bar waybar",
  "return-type": "json",
  "on-click": "omarchy-launch-floating-terminal-with-presentation /home/karneeshkar/Desktop/personal/ai_bar/target/release/ai_bar tui",
  "tooltip": true
}
```

The `waybar` command keeps running, refreshes provider usage every 5 minutes by
default, writes a cached snapshot, and emits Waybar JSON on every refresh.

The click action opens:

```bash
ai_bar tui
```

The TUI reads the cached snapshot every 2 seconds by default, so opening it does
not trigger extra provider API calls.

## Styling

Add these classes to `~/.config/waybar/style.css`:

```css
#custom-ai_bar {
  min-width: 12px;
  margin: 0 7.5px;
}

#custom-ai_bar.ok {
  color: #16d3b4;
}

#custom-ai_bar.warn {
  color: #f4b740;
}

#custom-ai_bar.crit {
  color: #ff5a5f;
}

#custom-ai_bar.stale {
  opacity: 0.55;
}

#custom-ai_bar.auth {
  opacity: 0.55;
  color: #888888;
}
```

Class meanings:

- `ok`: usage is below the warning threshold.
- `warn`: usage is at or above the warning threshold.
- `crit`: usage is at or above the critical threshold.
- `stale`: the cached snapshot is older than expected.
- `auth`: a provider could not return usable data, usually because login or
  local credentials are unavailable.

## Restart Waybar

After changing Waybar config or styles:

```bash
omarchy-restart-waybar
```

If the module does not appear, run the command manually to see errors:

```bash
/home/karneeshkar/Desktop/personal/ai_bar/target/release/ai_bar waybar
```

It should print one JSON object and continue running.

## Configuration

The config file is:

```bash
~/.config/ai_bar/config.toml
```

Show the path:

```bash
ai_bar config
```

Print the resolved config:

```bash
ai_bar config --print
```

Example config:

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

Notes:

- `interval_secs` controls the provider polling interval. The code enforces a
  minimum of 30 seconds.
- `cost_refresh_secs` controls the slower local cost scan interval. The code
  enforces a minimum of 60 seconds.
- `binary` can be omitted to let `ai_bar` find `codex` or `claude` from `PATH`.
- `providers.claude.prefer` controls Claude usage source order. Supported
  values include `cookies`, `web`, `api`, and `pty`.

## Common commands

```bash
ai_bar status
ai_bar status --detailed
ai_bar tui
ai_bar tui --poll-secs 5
ai_bar cost
ai_bar cost --provider codex
ai_bar cost --provider claude
ai_bar config --print
ai_bar config --print-waybar-snippet
```

## Troubleshooting

If Waybar shows `C --` or `Cl --`, check that the corresponding CLI is installed
and logged in:

```bash
codex --help
claude --help
```

If the module shows the `auth` class, run:

```bash
ai_bar status --detailed
```

The detailed output includes provider errors and the source Claude used.

If the click popup does not open, verify that Omarchy has:

```bash
omarchy-launch-floating-terminal-with-presentation
```

If you are not using that helper, replace the `on-click` command with your
terminal of choice, for example:

```jsonc
"on-click": "ghostty --class=ai_bar-popup -e /home/karneeshkar/Desktop/personal/ai_bar/target/release/ai_bar tui"
```

