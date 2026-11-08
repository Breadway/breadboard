//! Room compaction summaries.
//!
//! The server has no model of its own. It calls an OpenAI-compatible chat
//! completions endpoint (llama.cpp on the host by default) and stores the
//! returned text as a summary message.

use anyhow::{Context, Result};
use breadboard_core::{ChatMessage, ChatRoom};
use serde::Deserialize;
use std::time::Duration;

use crate::config::SummaryConfig;

const MAX_TRANSCRIPT_CHARS: usize = 24_000;
const MAX_SUMMARY_CHARS: usize = 6_000;

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ReplyMessage,
}

#[derive(Deserialize)]
struct ReplyMessage {
    content: Option<String>,
}

pub async fn summarize(
    cfg: &SummaryConfig,
    room: &ChatRoom,
    messages: &[ChatMessage],
) -> Result<String> {
    if cfg.url.trim().is_empty() {
        anyhow::bail!(
            "compaction is not configured: set BREADBOARD_SUMMARY_URL to an OpenAI-compatible endpoint"
        );
    }
    if messages.is_empty() {
        anyhow::bail!("no messages to compact");
    }

    let where_ = match (&room.kind, &room.name) {
        (breadboard_core::ChatRoomKind::Room, Some(name)) => format!("#{name}"),
        _ => "a direct message".to_string(),
    };
    let mut transcript = String::new();
    let mut included = 0usize;
    for message in messages.iter().rev() {
        let line = format!(
            "{} [{}]: {}\n",
            message.author_display,
            message.author_kind.as_str(),
            message.body.trim()
        );
        if transcript.len() + line.len() > MAX_TRANSCRIPT_CHARS {
            break;
        }
        transcript.insert_str(0, &line);
        included += 1;
    }

    let system = "You compact a work chat room for a Kanban tracker. Return at most 12 short \
        bullet points covering: decisions, concrete facts (issue keys, PR numbers, branches, \
        outcomes), open questions, blockers, and who owns what. Keep names, keys, and numbers \
        exact. Do not add a preamble or commentary. If the conversation has no substance, say \
        that in one line.";
    let user = format!(
        "Room: {where_}\nDescription: {}\nMessages: {included}{} \n\n{transcript}",
        room.description.as_deref().unwrap_or("(none)"),
        if included < messages.len() {
            format!(" of {}", messages.len())
        } else {
            String::new()
        }
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .context("building summary client")?;
    let mut request = client.post(cfg.url.trim()).json(&serde_json::json!({
        "model": cfg.model,
        "temperature": 0.2,
        "max_tokens": 700,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ]
    }));
    if !cfg.token.trim().is_empty() {
        request = request.bearer_auth(cfg.token.trim());
    }

    let response = request.send().await.context("summary request failed")?;
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "summary endpoint returned {status}: {}",
            detail.chars().take(300).collect::<String>()
        );
    }
    let body: ChatResponse = response.json().await.context("summary response was not JSON")?;
    let text = body
        .choices
        .into_iter()
        .next()
        .and_then(|choice| choice.message.content)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .context("summary endpoint returned no text")?;

    Ok(text.chars().take(MAX_SUMMARY_CHARS).collect())
}
