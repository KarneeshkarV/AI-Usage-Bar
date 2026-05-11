use anyhow::Result;

use crate::config::{Config, config_path};

pub async fn run(print: bool, print_waybar_snippet: bool) -> Result<()> {
    if print_waybar_snippet {
        print_snippets();
        return Ok(());
    }
    if print {
        let cfg = Config::load_or_default()?;
        println!("# loaded from: {}", config_path()?.display());
        println!("{}", toml::to_string_pretty(&cfg)?);
        return Ok(());
    }
    // Default: show the resolved path so users know where to edit
    println!("config path: {}", config_path()?.display());
    println!("use --print to dump current values, or --print-waybar-snippet for integration help");
    Ok(())
}

fn print_snippets() {
    let bin = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "ai_bar".into());

    println!("# Add to ~/.config/waybar/config.jsonc:");
    println!(r#""custom/ai_bar": {{"#);
    println!(r#"    "exec": "{bin} waybar","#);
    println!(r#"    "return-type": "json","#);
    println!(r#"    "on-click": "omarchy-launch-floating-terminal-with-presentation {bin} tui","#);
    println!(r#"    "tooltip": true"#);
    println!("}}");
    println!();
    println!("# Add to ~/.config/waybar/style.css:");
    println!("#custom-ai_bar.ok    {{ color: #16d3b4; }}");
    println!("#custom-ai_bar.warn  {{ color: #f4b740; }}");
    println!("#custom-ai_bar.crit  {{ color: #ff5a5f; }}");
    println!("#custom-ai_bar.stale {{ opacity: 0.55; }}");
    println!("#custom-ai_bar.auth  {{ color: #888; }}");
    println!();
    println!("# Add to ~/.config/hypr/hyprland.conf:");
    println!("windowrulev2 = float, class:^(ai_bar-popup)$");
    println!("windowrulev2 = size 800 600, class:^(ai_bar-popup)$");
    println!("windowrulev2 = center, class:^(ai_bar-popup)$");
    println!();
    println!("# Then run: omarchy-restart-waybar");
}
