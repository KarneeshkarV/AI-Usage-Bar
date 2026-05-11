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
        .unwrap_or_else(|| "ai-usage-bar".into());
    let bin = shell_quote(&bin);

    println!("# Add to ~/.config/waybar/config.jsonc:");
    println!(r#""custom/ai-usage-bar": {{"#);
    println!(r#"    "exec": "{bin} waybar","#);
    println!(r#"    "return-type": "json","#);
    println!(r#"    "on-click": "omarchy-launch-floating-terminal-with-presentation {bin} tui","#);
    println!(r#"    "tooltip": true"#);
    println!("}}");
    println!();
    println!("# Add to ~/.config/waybar/style.css:");
    println!("#custom-ai-usage-bar.ok    {{ color: #16d3b4; }}");
    println!("#custom-ai-usage-bar.warn  {{ color: #f4b740; }}");
    println!("#custom-ai-usage-bar.crit  {{ color: #ff5a5f; }}");
    println!("#custom-ai-usage-bar.stale {{ opacity: 0.55; }}");
    println!("#custom-ai-usage-bar.auth  {{ color: #888; }}");
    println!();
    println!("# Add to ~/.config/hypr/hyprland.conf:");
    println!("windowrulev2 = float, class:^(ai-usage-bar-popup)$");
    println!("windowrulev2 = size 800 600, class:^(ai-usage-bar-popup)$");
    println!("windowrulev2 = center, class:^(ai-usage-bar-popup)$");
    println!();
    println!("# Then run: omarchy-restart-waybar");
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
    {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}
