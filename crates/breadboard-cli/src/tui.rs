//! `breadboard tui`: a full-screen terminal board.
//!
//! Arrows / hjkl move the cursor, `[` `]` move the selected card between
//! statuses, `enter` shows detail, `r` refreshes, `q` quits. The board also
//! refreshes on a short timer.

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use anyhow::Result;
use breadboard_client::Client;
use breadboard_core::{Issue, Status};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

const REFRESH: Duration = Duration::from_secs(5);

struct App {
    client: Client,
    issues: Vec<Issue>,
    col: usize,
    rows: [ListState; 5],
    detail: Option<Issue>,
    status_line: String,
    last_refresh: Instant,
}

impl App {
    fn column_issues(&self, status: Status) -> Vec<&Issue> {
        let mut v: Vec<&Issue> = self.issues.iter().filter(|i| i.status == status).collect();
        v.sort_by(|a, b| a.key.cmp(&b.key));
        v
    }

    fn selected(&self) -> Option<&Issue> {
        let status = Status::ALL[self.col];
        let col = self.column_issues(status);
        self.rows[self.col]
            .selected()
            .and_then(|i| col.get(i).copied())
    }

    async fn refresh(&mut self) {
        match self.client.list_issues(&Default::default()).await {
            Ok(mut issues) => {
                issues.sort_by(|a, b| a.key.cmp(&b.key));
                self.issues = issues;
                self.clamp_selection();
                self.last_refresh = Instant::now();
            }
            Err(e) => self.status_line = format!("refresh failed: {e}"),
        }
    }

    fn clamp_selection(&mut self) {
        for (idx, status) in Status::ALL.into_iter().enumerate() {
            let len = self.column_issues(status).len();
            let sel = self.rows[idx].selected().unwrap_or(0);
            if len == 0 {
                self.rows[idx].select(None);
            } else {
                self.rows[idx].select(Some(sel.min(len - 1)));
            }
        }
    }

    fn move_cursor_row(&mut self, delta: isize) {
        let len = self.column_issues(Status::ALL[self.col]).len();
        if len == 0 {
            return;
        }
        let cur = self.rows[self.col].selected().unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(len as isize) as usize;
        self.rows[self.col].select(Some(next));
    }

    fn move_col(&mut self, delta: isize) {
        self.col = (self.col as isize + delta).rem_euclid(5) as usize;
        self.clamp_selection();
    }

    async fn move_card(&mut self, delta: isize) {
        let Some(issue) = self.selected().cloned() else {
            return;
        };
        let target = (self.col as isize + delta).clamp(0, 4) as usize;
        if target == self.col {
            return;
        }
        let new_status = Status::ALL[target];
        match self
            .client
            .patch_issue(
                &issue.key,
                &breadboard_core::dto::IssuePatch {
                    status: Some(new_status),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(updated) => {
                if let Some(slot) = self.issues.iter_mut().find(|i| i.id == updated.id) {
                    *slot = updated;
                }
                self.col = target;
                self.clamp_selection();
                self.status_line = format!("{} → {}", issue.key, new_status.as_str());
            }
            Err(e) => self.status_line = format!("move failed: {e}"),
        }
    }
}

pub async fn run(client: Client) -> Result<()> {
    let mut terminal = setup()?;
    let mut app = App {
        client,
        issues: Vec::new(),
        col: 0,
        rows: Default::default(),
        detail: None,
        status_line: "loading…".into(),
        last_refresh: Instant::now() - REFRESH,
    };
    app.refresh().await;
    if app.status_line == "loading…" {
        app.status_line = "r refresh · [ ] move · enter detail · q quit".into();
    }

    let res = event_loop(&mut terminal, &mut app).await;
    restore(&mut terminal)?;
    res
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| draw(f, app))?;

        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                if app.detail.is_some() {
                    app.detail = None;
                    continue;
                }
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('r') => app.refresh().await,
                    KeyCode::Left | KeyCode::Char('h') => app.move_col(-1),
                    KeyCode::Right | KeyCode::Char('l') => app.move_col(1),
                    KeyCode::Up | KeyCode::Char('k') => app.move_cursor_row(-1),
                    KeyCode::Down | KeyCode::Char('j') => app.move_cursor_row(1),
                    KeyCode::Char('[') => app.move_card(-1).await,
                    KeyCode::Char(']') => app.move_card(1).await,
                    KeyCode::Enter => app.detail = app.selected().cloned(),
                    _ => {}
                }
            }
        }

        if app.last_refresh.elapsed() >= REFRESH {
            app.refresh().await;
        }
    }
}

fn draw(f: &mut Frame, app: &App) {
    let outer = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(f.area());
    let cols = Layout::horizontal([Constraint::Ratio(1, 5); 5]).split(outer[0]);
    let labels = ["Backlog", "Todo", "Doing", "Review", "Done"];

    for (idx, status) in Status::ALL.into_iter().enumerate() {
        let items: Vec<ListItem> = app
            .column_issues(status)
            .into_iter()
            .map(|i| {
                let mark = if i.priority == breadboard_core::Priority::Urgent {
                    "! "
                } else {
                    ""
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{} ", i.key), Style::new().dim()),
                    Span::raw(format!("{mark}{}", i.title)),
                ]))
            })
            .collect();
        let focused = idx == app.col;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(
                " {} ({}) ",
                labels[idx],
                app.column_issues(status).len()
            ))
            .border_style(if focused {
                Style::new().fg(Color::Rgb(0xd9, 0x8a, 0x3d))
            } else {
                Style::new().dim()
            });
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().reversed());
        let mut state = app.rows[idx].clone();
        f.render_stateful_widget(list, cols[idx], &mut state);
    }

    f.render_widget(
        Paragraph::new(app.status_line.as_str()).style(Style::new().dim()),
        outer[1],
    );

    if let Some(issue) = &app.detail {
        detail_popup(f, issue);
    }
}

fn detail_popup(f: &mut Frame, issue: &Issue) {
    let area = centered(70, 60, f.area());
    let mut lines = vec![
        Line::from(Span::styled(
            format!("{}  {}", issue.key, issue.title),
            Style::new().bold(),
        )),
        Line::raw(""),
        Line::raw(format!("status    {}", issue.status.as_str())),
        Line::raw(format!("priority  {}", issue.priority.as_str())),
    ];
    if let Some(e) = issue.estimate {
        lines.push(Line::raw(format!("estimate  {e}")));
    }
    if !issue.labels.is_empty() {
        lines.push(Line::raw(format!("labels    {}", issue.labels.join(", "))));
    }
    if let Some(fr) = &issue.forgejo_ref {
        lines.push(Line::raw(format!("forgejo   {}#{}", fr.repo, fr.number)));
    }
    if !issue.body.is_empty() {
        lines.push(Line::raw(""));
        for l in issue.body.lines() {
            lines.push(Line::raw(l.to_string()));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "any key to close",
        Style::new().dim(),
    )));

    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" issue ")),
        area,
    );
}

fn centered(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let v = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(v[1])[1]
}

fn setup() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
