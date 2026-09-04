//! `breadboard` — CLI + TUI for breadboard.

mod config;
mod tui;

use anyhow::{anyhow, bail, Context, Result};
use breadboard_client::{Client, DevicePoll};
use breadboard_core::dto::{
    EpicPatch, IssueFilter, IssuePatch, NewCycle, NewEpic, NewIssue, NewProject, ProjectPatch,
};
use breadboard_core::{Priority, Status};
use clap::{Parser, Subcommand};

use crate::config::CliConfig;

fn parse_status(s: &str) -> std::result::Result<Status, String> {
    Status::parse(s).ok_or_else(|| format!("unknown status {s:?} (backlog|todo|doing|review|done)"))
}

fn parse_priority(s: &str) -> std::result::Result<Priority, String> {
    Priority::parse(s).ok_or_else(|| format!("unknown priority {s:?} (none|low|medium|high|urgent)"))
}

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
    /// Authenticate this machine.
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
        #[arg(long = "label", value_name = "LABEL")]
        labels: Vec<String>,
        #[arg(long, value_parser = parse_priority)]
        priority: Option<Priority>,
    },
    /// Move an issue to a new status.
    Mv {
        key: String,
        #[arg(value_parser = parse_status)]
        status: Status,
    },
    /// Print the board.
    Board {
        #[arg(long)]
        cycle: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        mine: bool,
    },
    /// List issues.
    Ls {
        #[arg(long)]
        mine: bool,
        #[arg(long, value_parser = parse_status)]
        status: Option<Status>,
        #[arg(long)]
        project: Option<String>,
    },
    /// Show one issue.
    Show { key: String },
    /// Add a comment to an issue (also pushed to Forgejo if linked).
    Comment { key: String, body: String },
    /// Show an issue's change history (from sync_log).
    Activity { key: String },
    /// Your notifications.
    Inbox {
        /// Mark everything read.
        #[arg(long)]
        read: bool,
    },
    /// Manage projects.
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
    /// Manage epics (cross-repo).
    Epic {
        #[command(subcommand)]
        action: EpicAction,
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
    /// Run a Forgejo reconcile pass now.
    Sync,
    /// Import a markdown outline (## headings → epics, `- [ ]` items → issues).
    Import {
        file: std::path::PathBuf,
        #[arg(long)]
        project: String,
        #[arg(long)]
        dry_run: bool,
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
enum ProjectAction {
    List,
    /// Create a repo-less project (for planning / roadmap items).
    New {
        key: String,
        name: String,
    },
    Rename {
        key: String,
        name: String,
    },
    Archive {
        key: String,
    },
    Unarchive {
        key: String,
    },
}

#[derive(Subcommand)]
enum EpicAction {
    List,
    New {
        key: String,
        title: String,
        #[arg(long)]
        project: Option<String>,
    },
    Rename {
        key: String,
        title: String,
    },
    Archive {
        key: String,
    },
    Unarchive {
        key: String,
    },
}

#[derive(Subcommand)]
enum CycleAction {
    List,
    New {
        name: String,
        /// YYYY-MM-DD
        starts_on: chrono::NaiveDate,
        /// YYYY-MM-DD
        ends_on: chrono::NaiveDate,
    },
    Start { name: String },
    End { name: String },
}

#[derive(Subcommand)]
enum ReposAction {
    List,
    Add {
        /// owner/name on git.breadway.dev
        repo: String,
        /// project key to file the repo's issues under
        key: String,
        #[arg(long)]
        name: Option<String>,
    },
    Remove {
        repo: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut cfg = CliConfig::load()?;
    if let Some(s) = &cli.server {
        cfg.server = s.clone();
    }
    let client = Client::new(cfg.server.clone(), cfg.token.clone());

    match cli.command {
        Command::Ping => {
            let h = client.health().await?;
            println!("{} — {} (v{}, db={})", cfg.server, h.status, h.version, h.db);
        }
        Command::Auth { action } => auth(&client, &mut cfg, action).await?,
        Command::Add {
            title,
            project,
            epic,
            cycle,
            labels,
            priority,
        } => {
            let epic_id = match epic {
                Some(e) => Some(resolve_epic(&client, &e).await?),
                None => None,
            };
            let cycle_id = match cycle {
                Some(c) => Some(resolve_cycle(&client, &c).await?),
                None => None,
            };
            let issue = client
                .create_issue(&NewIssue {
                    title,
                    project,
                    epic_id,
                    cycle_id,
                    labels,
                    priority,
                    ..Default::default()
                })
                .await?;
            println!("{}  {}", issue.key, issue.title);
        }
        Command::Mv { key, status } => {
            let issue = client
                .patch_issue(
                    &key,
                    &IssuePatch {
                        status: Some(status),
                        ..Default::default()
                    },
                )
                .await?;
            println!("{}  → {}", issue.key, issue.status.as_str());
        }
        Command::Board {
            cycle,
            project,
            mine,
        } => {
            let filter = IssueFilter {
                project,
                cycle_id: match cycle {
                    Some(c) => Some(resolve_cycle(&client, &c).await?),
                    None => None,
                },
                mine,
                ..Default::default()
            };
            print_board(&client.list_issues(&filter).await?);
        }
        Command::Ls {
            mine,
            status,
            project,
        } => {
            let filter = IssueFilter {
                mine,
                status,
                project,
                ..Default::default()
            };
            print_list(&client.list_issues(&filter).await?);
        }
        Command::Show { key } => {
            let d = client.get_issue(&key).await?;
            print_issue(&d);
        }
        Command::Comment { key, body } => {
            client.add_comment(&key, &body).await?;
            println!("comment added to {key}");
        }
        Command::Activity { key } => {
            for a in client.issue_activity(&key).await? {
                let who = a.actor.unwrap_or_else(|| a.source.clone());
                let val = a.new_value.unwrap_or_else(|| "—".into());
                println!("{}  {:<12} {} → {}  ({who})", a.at, a.field, a.old_value.unwrap_or_else(|| "—".into()), val);
            }
        }
        Command::Inbox { read } => {
            if read {
                client.mark_all_read().await?;
                println!("marked all read");
            } else {
                let box_ = client.inbox(false).await?;
                for n in &box_.notifications {
                    println!("{}  {}{}", n.created_at, if n.read { "  " } else { "• " }, n.body);
                }
                println!("\n{} unread", box_.unread);
            }
        }
        Command::Project { action } => projects(&client, action).await?,
        Command::Epic { action } => epics(&client, action).await?,
        Command::Cycle { action } => cycles(&client, action).await?,
        Command::Repos { action } => repos(&client, action).await?,
        Command::Sync => {
            client.reconcile().await?;
            println!("reconcile pass queued");
        }
        Command::Import { file, project, dry_run } => {
            let md = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let r = client.import(&project, &md, dry_run).await?;
            if dry_run {
                println!("(dry run)");
            }
            println!("epics:   {}", r.epics_created.join(", "));
            println!("issues:  {} created", r.issues_created.len());
            if !r.skipped.is_empty() {
                println!("skipped: {} (already present)", r.skipped.len());
            }
        }
        Command::Tui => tui::run(client).await?,
    }

    Ok(())
}

async fn auth(client: &Client, cfg: &mut CliConfig, action: AuthAction) -> Result<()> {
    match action {
        AuthAction::Status => match client.me().await {
            Ok(u) => println!("signed in as {} ({})", u.login, u.display_name),
            Err(breadboard_client::Error::Unauthenticated) => {
                println!("not signed in — run `breadboard auth login`")
            }
            Err(e) => return Err(e.into()),
        },
        AuthAction::Logout => {
            cfg.token = None;
            cfg.save()?;
            println!("signed out (local token cleared)");
        }
        AuthAction::Login => {
            let start = client.device_start().await.context("starting device auth")?;
            println!("\n  Open:  {}", start.verification_uri_complete);
            println!("  Code:  {}\n", start.user_code);
            let _ = std::process::Command::new("xdg-open")
                .arg(&start.verification_uri_complete)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            print!("waiting for approval");
            use std::io::Write;
            let token = loop {
                std::io::stdout().flush().ok();
                tokio::time::sleep(std::time::Duration::from_secs(start.interval)).await;
                match client.device_poll(&start.device_code).await? {
                    DevicePoll::Pending => print!("."),
                    DevicePoll::Approved(t) => break t,
                    DevicePoll::Expired => bail!("\nthe code expired — run `breadboard auth login` again"),
                }
            };
            cfg.token = Some(token);
            cfg.save()?;
            let who = Client::new(cfg.server.clone(), cfg.token.clone()).me().await?;
            println!("\nsigned in as {}", who.login);
        }
    }
    Ok(())
}

async fn projects(client: &Client, action: ProjectAction) -> Result<()> {
    match action {
        ProjectAction::List => {
            for p in client.list_projects().await? {
                println!(
                    "{:>3}  {:<8}  {:<24}  {}{}",
                    p.id,
                    p.key,
                    p.name,
                    p.forgejo_repo.as_deref().unwrap_or("(no repo)"),
                    if p.archived { "  [archived]" } else { "" }
                );
            }
        }
        ProjectAction::New { key, name } => {
            let p = client
                .create_project(&NewProject {
                    key,
                    name,
                    forgejo_repo: None,
                })
                .await?;
            println!("created project {} ({})", p.key, p.name);
        }
        ProjectAction::Rename { key, name } => {
            let id = resolve_project(client, &key).await?;
            let p = client
                .update_project(
                    id,
                    &ProjectPatch {
                        name: Some(name),
                        ..Default::default()
                    },
                )
                .await?;
            println!("renamed {} → {}", p.key, p.name);
        }
        ProjectAction::Archive { key } => set_archived(client, &key, true).await?,
        ProjectAction::Unarchive { key } => set_archived(client, &key, false).await?,
    }
    Ok(())
}

async fn set_archived(client: &Client, key: &str, archived: bool) -> Result<()> {
    let id = resolve_project(client, key).await?;
    let p = client
        .update_project(
            id,
            &ProjectPatch {
                archived: Some(archived),
                ..Default::default()
            },
        )
        .await?;
    println!("{} {}", if p.archived { "archived" } else { "unarchived" }, p.key);
    Ok(())
}

async fn epics(client: &Client, action: EpicAction) -> Result<()> {
    match action {
        EpicAction::List => {
            for e in client.list_epics().await? {
                let done = e.progress.get("done").copied().unwrap_or(0);
                let bar = if e.total > 0 {
                    format!("{done}/{} done", e.total)
                } else {
                    "no issues".into()
                };
                println!(
                    "{:<10} {:<32} {}{}",
                    e.epic.key,
                    e.epic.title,
                    bar,
                    if e.epic.archived { "  [archived]" } else { "" }
                );
            }
        }
        EpicAction::New { key, title, project } => {
            let project_id = match project {
                Some(p) => Some(resolve_project(client, &p).await?),
                None => None,
            };
            let e = client
                .create_epic(&NewEpic {
                    key,
                    title,
                    description: None,
                    project_id,
                })
                .await?;
            println!("created epic {} ({})", e.key, e.title);
        }
        EpicAction::Rename { key, title } => {
            let id = resolve_epic(client, &key).await?;
            let e = client
                .update_epic(id, &EpicPatch { title: Some(title), ..Default::default() })
                .await?;
            println!("renamed {} → {}", e.key, e.title);
        }
        EpicAction::Archive { key } => set_epic_archived(client, &key, true).await?,
        EpicAction::Unarchive { key } => set_epic_archived(client, &key, false).await?,
    }
    Ok(())
}

async fn set_epic_archived(client: &Client, key: &str, archived: bool) -> Result<()> {
    let id = resolve_epic(client, key).await?;
    let e = client
        .update_epic(id, &EpicPatch { archived: Some(archived), ..Default::default() })
        .await?;
    println!("{} {}", if e.archived { "archived" } else { "unarchived" }, e.key);
    Ok(())
}

async fn resolve_project(client: &Client, key_or_id: &str) -> Result<i64> {
    if let Ok(id) = key_or_id.parse::<i64>() {
        return Ok(id);
    }
    client
        .list_projects()
        .await?
        .into_iter()
        .find(|p| p.key.eq_ignore_ascii_case(key_or_id))
        .map(|p| p.id)
        .ok_or_else(|| anyhow!("no project with key {key_or_id:?}"))
}

async fn cycles(client: &Client, action: CycleAction) -> Result<()> {
    match action {
        CycleAction::List => {
            for c in client.list_cycles().await? {
                println!(
                    "{:>3}  {:<20}  {} → {}{}",
                    c.id,
                    c.name,
                    c.starts_on,
                    c.ends_on,
                    if c.closed { "  (closed)" } else { "" }
                );
            }
        }
        CycleAction::New {
            name,
            starts_on,
            ends_on,
        } => {
            let c = client
                .create_cycle(&NewCycle {
                    name,
                    starts_on,
                    ends_on,
                })
                .await?;
            println!("created cycle {} ({})", c.id, c.name);
        }
        CycleAction::Start { name } => {
            let id = resolve_cycle(client, &name).await?;
            let c = client.start_cycle(id).await?;
            println!("started cycle {}", c.name);
        }
        CycleAction::End { name } => {
            let id = resolve_cycle(client, &name).await?;
            let c = client.end_cycle(id).await?;
            println!("ended cycle {}", c.name);
        }
    }
    Ok(())
}

async fn repos(client: &Client, action: ReposAction) -> Result<()> {
    match action {
        ReposAction::List => {
            for r in client.list_repos().await? {
                println!(
                    "{:<32}  {:<8}  {}{}",
                    r.repo,
                    r.project_key.as_deref().unwrap_or("-"),
                    if r.enabled { "enabled" } else { "disabled" },
                    r.last_synced_at
                        .map(|t| format!("  last sync {t}"))
                        .unwrap_or_default()
                );
            }
        }
        ReposAction::Add { repo, key, name } => {
            let r = client.add_repo(&repo, &key, name.as_deref()).await?;
            println!(
                "syncing {} → project {}",
                r.repo,
                r.project_key.as_deref().unwrap_or(&key)
            );
        }
        ReposAction::Remove { repo } => {
            client.remove_repo(&repo).await?;
            println!("stopped syncing {repo}");
        }
    }
    Ok(())
}

async fn resolve_cycle(client: &Client, name_or_id: &str) -> Result<i64> {
    if let Ok(id) = name_or_id.parse::<i64>() {
        return Ok(id);
    }
    client
        .list_cycles()
        .await?
        .into_iter()
        .find(|c| c.name.eq_ignore_ascii_case(name_or_id))
        .map(|c| c.id)
        .ok_or_else(|| anyhow!("no cycle named {name_or_id:?}"))
}

async fn resolve_epic(client: &Client, key_or_id: &str) -> Result<i64> {
    if let Ok(id) = key_or_id.parse::<i64>() {
        return Ok(id);
    }
    client
        .list_epics()
        .await?
        .into_iter()
        .find(|e| e.epic.key.eq_ignore_ascii_case(key_or_id))
        .map(|e| e.epic.id)
        .ok_or_else(|| anyhow!("no epic with key {key_or_id:?}"))
}

fn print_board(issues: &[breadboard_core::Issue]) {
    for status in Status::ALL {
        let col: Vec<_> = issues.iter().filter(|i| i.status == status).collect();
        println!("\n{} ({})", status.as_str().to_uppercase(), col.len());
        for i in col {
            println!("  {:<12} {}", i.key, i.title);
        }
    }
}

fn print_list(issues: &[breadboard_core::Issue]) {
    for i in issues {
        let p = if i.priority == Priority::None {
            String::new()
        } else {
            format!("  !{}", i.priority.as_str())
        };
        println!("{:<12} {:<8} {}{}", i.key, i.status.as_str(), i.title, p);
    }
}

fn print_issue(d: &breadboard_client::IssueDetail) {
    let i = &d.issue;
    println!("{}  {}", i.key, i.title);
    println!("  status    {}", i.status.as_str());
    println!("  priority  {}", i.priority.as_str());
    if let Some(e) = i.estimate {
        println!("  estimate  {e}");
    }
    if !i.labels.is_empty() {
        println!("  labels    {}", i.labels.join(", "));
    }
    if let Some(m) = &i.milestone {
        println!("  milestone {m}");
    }
    if let Some(f) = &i.forgejo_ref {
        println!("  forgejo   {}#{}", f.repo, f.number);
    }
    if !i.body.is_empty() {
        println!("\n{}\n", i.body);
    }
    for c in &d.comments {
        println!("  — {} ({})", c.body, c.created_at);
    }
}
