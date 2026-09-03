//! `breadboard` — CLI + TUI for breadboard.
//!
//! Milestone 1: argument surface is defined and `ping` works end-to-end against
//! a running server. The mutating subcommands are stubbed until the server's
//! REST routes land (milestone 5 fills them in, plus `breadboard tui`).

mod config;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::config::CliConfig;

#[derive(Parser)]
#[command(name = "breadboard", version, about = "Issue tracker for the bread ecosystem")]
struct Cli {
    /// Server base URL (overrides config / $BREADBOARD_SERVER).
    #[arg(long, global = true, env = "BREADBOARD_SERVER")]
    server: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check connectivity to the server.
    Ping,
    /// Authenticate this machine (device flow via Forgejo SSO).
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Create an issue.
    Add {
        title: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        epic: Option<String>,
        #[arg(long)]
        cycle: Option<String>,
    },
    /// Move an issue to a new status.
    Mv { key: String, status: String },
    /// Print the board.
    Board {
        #[arg(long)]
        cycle: Option<String>,
        #[arg(long)]
        project: Option<String>,
    },
    /// List issues.
    Ls {
        #[arg(long)]
        mine: bool,
        #[arg(long)]
        status: Option<String>,
    },
    /// Manage cycles.
    Cycle {
        #[command(subcommand)]
        action: CycleAction,
    },
    /// Manage which Forgejo repos are synced.
    Repos {
        #[command(subcommand)]
        action: ReposAction,
    },
    /// Force a full Forgejo reconcile.
    Sync {
        #[arg(long)]
        full: bool,
    },
    /// Open the full-screen terminal board.
    Tui,
}

#[derive(Subcommand)]
enum AuthAction {
    Login,
    Logout,
    Status,
}

#[derive(Subcommand)]
enum CycleAction {
    List,
    Start { name: String },
    End { name: String },
}

#[derive(Subcommand)]
enum ReposAction {
    List,
    Add { repos: Vec<String> },
    Remove { repos: Vec<String> },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = CliConfig::load()?;
    let base_url = cli.server.clone().unwrap_or(cfg.server.clone());
    let client = breadboard_client::Client::new(base_url.clone(), cfg.token.clone());

    match cli.command {
        Command::Ping => {
            let health = client.health().await?;
            println!("{base_url} — {} (v{}, db={})", health.status, health.version, health.db);
        }
        Command::Tui => {
            anyhow::bail!("`breadboard tui` is not implemented yet (milestone 5)");
        }
        other => {
            let _ = other;
            anyhow::bail!("this subcommand is not implemented yet — see the milestone plan");
        }
    }

    Ok(())
}
