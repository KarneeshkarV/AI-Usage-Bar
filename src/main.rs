use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod cli;
mod config;
mod cost;
mod providers;
mod snapshot;
mod util;
mod waybar_proto;

#[derive(Parser)]
#[command(name = "ai_bar", version, about = "AI coding usage limits in your Waybar")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
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
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        match cli.command {
            Cmd::Waybar => cli::waybar::run().await,
            Cmd::Status { detailed } => cli::status::run(detailed).await,
            Cmd::Cost { provider } => cli::cost::run(provider).await,
            Cmd::Config {
                print,
                print_waybar_snippet,
            } => cli::config_cmd::run(print, print_waybar_snippet).await,
        }
    })
}
