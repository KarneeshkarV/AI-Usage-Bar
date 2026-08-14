use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod backfill;
mod celebration;
mod cli;
mod config;
mod cost;
mod notify;
mod pace;
mod ping;
mod provider_status;
mod providers;
mod snapshot;
mod util;
mod waybar_proto;

#[derive(Parser)]
#[command(
    name = "ai-usage-bar",
    version,
    about = "AI Usage Bar: AI coding usage limits in your Waybar. Default: print the four limits."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Long-running. Polls providers and emits Waybar JSON to stdout.
    Waybar,
    /// One-shot status snapshot (for terminal popup).
    Status {
        #[arg(long)]
        detailed: bool,
    },
    /// Print Codex, Claude, Grok, and Cursor limits as a colored dashboard.
    Limits,
    /// Interactive terminal popover inspired by CodexBar.
    Tui {
        /// Local snapshot polling interval while the popup is open.
        #[arg(long, default_value_t = 2)]
        poll_secs: u64,
    },
    /// 30-day local cost scan from JSONL session logs.
    Cost {
        #[arg(long, value_enum, default_value_t = cli::cost::Which::Both)]
        provider: cli::cost::Which,
    },
    /// Inspect or print config + integration snippets.
    Config {
        #[arg(long)]
        print: bool,
        #[arg(long)]
        print_waybar_snippet: bool,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        match cli.command {
            None | Some(Cmd::Limits) => cli::limits::run().await,
            Some(Cmd::Waybar) => cli::waybar::run().await,
            Some(Cmd::Status { detailed }) => cli::status::run(detailed).await,
            Some(Cmd::Tui { poll_secs }) => cli::tui::run(poll_secs).await,
            Some(Cmd::Cost { provider }) => cli::cost::run(provider).await,
            Some(Cmd::Config {
                print,
                print_waybar_snippet,
            }) => cli::config_cmd::run(print, print_waybar_snippet).await,
        }
    })
}
