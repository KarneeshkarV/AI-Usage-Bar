# Omarchy integration

Live changes applied to this machine on **2026-05-11**.

## Files modified

| Path | Change |
|---|---|
| `~/.config/waybar/config.jsonc` | Added `custom/ai_bar` to `modules-right` (first slot) + module definition; updated click action to open the Ratatui popup |
| `~/.config/waybar/style.css` | Added `#custom-ai_bar.{ok,warn,crit,stale,auth}` color classes |

Timestamped backups: `~/.config/waybar/config.jsonc.bak.1778473364` and `~/.config/waybar/style.css.bak.1778473364`.
TUI update backup: `~/.config/waybar/config.jsonc.bak.20260511-tui`.

## What was added

`~/.config/waybar/config.jsonc`:

```jsonc
"modules-right": ["custom/ai_bar", "memory", ...],

"custom/ai_bar": {
  "exec": "/home/karneeshkar/Desktop/personal/ai_bar/target/release/ai_bar waybar",
  "return-type": "json",
  "on-click": "omarchy-launch-floating-terminal-with-presentation /home/karneeshkar/Desktop/personal/ai_bar/target/release/ai_bar tui",
  "tooltip": true
}
```

`~/.config/waybar/style.css`:

```css
#custom-ai_bar { min-width: 12px; margin: 0 7.5px; }
#custom-ai_bar.ok    { color: #16d3b4; }
#custom-ai_bar.warn  { color: #f4b740; }
#custom-ai_bar.crit  { color: #ff5a5f; }
#custom-ai_bar.stale { opacity: 0.55; }
#custom-ai_bar.auth  { opacity: 0.55; color: #888888; }
```

No Hyprland window rules needed — `omarchy-launch-floating-terminal-with-presentation` opens with app-id `org.omarchy.terminal`, which Omarchy already floats and centers.

## Apply / restart

```bash
omarchy-restart-waybar
```

Waybar spawns `ai_bar waybar` as a long-lived child; the binary polls every 5 minutes and emits one JSON line per refresh.
The click popup runs `ai_bar tui`, a Ratatui terminal interface inspired by CodexBar. It polls the local cached snapshot every 2 seconds while open, so it refreshes reset countdowns and new Waybar snapshots without triggering extra provider API calls.

## Revert

```bash
cp ~/.config/waybar/config.jsonc.bak.1778473364 ~/.config/waybar/config.jsonc
cp ~/.config/waybar/style.css.bak.1778473364 ~/.config/waybar/style.css
omarchy-restart-waybar
```

## Regenerate snippets

If the binary path changes, `target/release/ai_bar config --print-waybar-snippet` emits paste-ready blocks with the current executable path.
