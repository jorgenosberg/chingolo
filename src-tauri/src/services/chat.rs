//! Sends a user message and streams the reply into the database and to the UI.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use specta::Type;
use tauri::ipc::Channel;
use tokio_util::sync::CancellationToken;

use crate::domain::{Message, MessageStatus, Role};
use crate::error::{AppError, Result};
use crate::persistence::{Db, chat as repo, settings};
use crate::security;
use crate::services::ai::{GenerationEvent, GenerationRequest, Provider, Turn, TurnRole};
use crate::state::{AppState, Generations};

const SYSTEM_PROMPT: &str = "You are Chingui, a research assistant for students. \
Answer carefully and say when you are unsure. \
You can't see the user's documents in this chat, so don't cite sources, page numbers or references, \
and don't invent any. If asked for citations, explain that you can't verify them here. \
Use Markdown. Write math as LaTeX between $...$ or $$...$$.";

const FLUSH_EVERY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize, Type)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatEvent {
    Delta { text: String },
    /// Sent once at the end, whether the reply completed, failed or was cancelled.
    Finished { message: Message },
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SendResult {
    pub user_message: Message,
    pub assistant_message: Message,
}

async fn provider_and_key(state: &AppState, provider_id: &str) -> Result<(Arc<dyn Provider>, String)> {
    let provider = state.providers.get(provider_id)?;
    let key = security::get_key(provider.id())
        .await?
        .ok_or_else(|| AppError::NoApiKey(provider.id().to_string()))?;
    Ok((provider, key))
}

struct Prepared {
    user_message: Message,
    assistant_message: Message,
    request: GenerationRequest,
}

pub async fn send(
    state: &AppState,
    conversation_id: String,
    content: String,
    on_event: Channel<ChatEvent>,
) -> Result<SendResult> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err(AppError::Invalid("Write a message first.".into()));
    }

    let conv_id = conversation_id.clone();
    let conv = state
        .db
        .call(move |c| repo::get_conversation(c, &conv_id))
        .await?;
    let (provider, api_key) = provider_and_key(state, &conv.provider).await?;

    let provider_id = provider.id().to_string();
    let prepared = state
        .db
        .call(move |c| {
            let model = match conv.model_id.clone() {
                Some(m) => m,
                None => settings::get(c, "default_model")?
                    .ok_or_else(|| AppError::Invalid("Choose a model first.".into()))?,
            };
            let tx = c.transaction()?;
            if conv.model_id.is_none() {
                repo::set_model(&tx, &conv.id, &model)?;
            }
            let user_message = repo::insert_message(
                &tx, &conv.id, Role::User, &content, MessageStatus::Complete, None, None,
            )?;
            repo::title_from_first_message(&tx, &conv.id, &content)?;
            let turns = history(&tx, &conv.id)?;
            let assistant_message = repo::insert_message(
                &tx, &conv.id, Role::Assistant, "", MessageStatus::Streaming,
                Some(&provider_id), Some(&model),
            )?;
            tx.commit()?;
            Ok(Prepared {
                user_message,
                assistant_message,
                request: GenerationRequest {
                    model,
                    system: Some(SYSTEM_PROMPT.to_string()),
                    turns,
                },
            })
        })
        .await?;

    let token = state.generations.register(&prepared.assistant_message.id);
    tauri::async_runtime::spawn(run_generation(
        state.db.clone(),
        state.generations.clone(),
        provider,
        api_key,
        prepared.request,
        prepared.assistant_message.clone(),
        on_event,
        token,
    ));

    Ok(SendResult {
        user_message: prepared.user_message,
        assistant_message: prepared.assistant_message,
    })
}

/// Finished messages only. Failed or cancelled replies are not fed back to the model.
fn history(conn: &rusqlite::Connection, conversation_id: &str) -> Result<Vec<Turn>> {
    Ok(repo::list_messages(conn, conversation_id)?
        .into_iter()
        .filter(|m| m.status == MessageStatus::Complete && !m.content.is_empty())
        .map(|m| Turn {
            role: match m.role {
                Role::User => TurnRole::User,
                Role::Assistant => TurnRole::Model,
            },
            text: m.content,
        })
        .collect())
}

enum Outcome {
    Done,
    Cancelled,
    Failed(String),
}

#[allow(clippy::too_many_arguments)]
async fn run_generation(
    db: Db,
    generations: Generations,
    provider: Arc<dyn Provider>,
    api_key: String,
    request: GenerationRequest,
    assistant: Message,
    on_event: Channel<ChatEvent>,
    token: CancellationToken,
) {
    let mut stream = provider.generate(api_key, request);
    let mut content = String::new();
    let (mut input_tokens, mut output_tokens) = (None, None);
    let mut finish_reason: Option<String> = None;
    let mut last_flush = Instant::now();

    let outcome = loop {
        tokio::select! {
            _ = token.cancelled() => break Outcome::Cancelled,
            item = stream.next() => match item {
                None => break Outcome::Done,
                Some(Ok(GenerationEvent::Text(text))) => {
                    content.push_str(&text);
                    let _ = on_event.send(ChatEvent::Delta { text });
                    if last_flush.elapsed() >= FLUSH_EVERY {
                        last_flush = Instant::now();
                        let (id, snapshot) = (assistant.id.clone(), content.clone());
                        let _ = db.call(move |c| repo::flush_content(c, &id, &snapshot)).await;
                    }
                }
                Some(Ok(GenerationEvent::Usage { input_tokens: i, output_tokens: o })) => {
                    input_tokens = Some(i);
                    output_tokens = Some(o);
                }
                Some(Ok(GenerationEvent::Finished(reason))) => finish_reason = Some(reason),
                Some(Err(e)) => break Outcome::Failed(e.to_string()),
            }
        }
    };
    drop(stream);

    let (status, note) = match outcome {
        Outcome::Cancelled => (MessageStatus::Cancelled, None),
        Outcome::Failed(msg) => (MessageStatus::Error, Some(msg)),
        Outcome::Done => match finish_reason.as_deref() {
            Some("STOP") => (MessageStatus::Complete, None),
            Some(other) => (
                MessageStatus::Complete,
                Some(format!("The model stopped early ({other}).")),
            ),
            None => (
                MessageStatus::Error,
                Some("The connection ended before the reply finished.".to_string()),
            ),
        },
    };

    let id = assistant.id.clone();
    let saved = db
        .call(move |c| {
            repo::finish_message(
                c,
                &id,
                repo::Finish {
                    content: &content,
                    status,
                    input_tokens,
                    output_tokens,
                    error: note.as_deref(),
                },
            )
        })
        .await;
    generations.remove(&assistant.id);

    let message = saved.unwrap_or_else(|e| Message {
        status: MessageStatus::Error,
        error: Some(format!("Couldn't save this reply: {e}")),
        ..assistant
    });
    let _ = on_event.send(ChatEvent::Finished { message });
}

const HANDOFF_SYSTEM: &str = "You write short, faithful handoff notes. \
Use only what is in the conversation. Don't add facts, sources or citations that were not there.";

const HANDOFF_REQUEST: &str = "Write a handoff note so a new conversation can pick up this work \
without the full history. Cover: the goal, the key facts and decisions so far (with any numbers \
exactly as given), what is still open or unclear, and the next steps. Keep it under 400 words. \
Plain Markdown, no preamble.";

/// Asks the conversation's own model for a handoff note. Using the same model means it reads
/// the history from its cache, which is cheaper than any other way of summarizing.
pub async fn handoff_summary(state: &AppState, conversation_id: String) -> Result<String> {
    let id = conversation_id.clone();
    let (conv, mut turns) = state
        .db
        .call(move |c| Ok((repo::get_conversation(c, &id)?, history(c, &id)?)))
        .await?;
    if turns.is_empty() {
        return Err(AppError::Invalid("There is nothing to summarize yet.".into()));
    }
    let model = match conv.model_id.clone() {
        Some(m) => m,
        None => state
            .db
            .call(|c| settings::get(c, "default_model"))
            .await?
            .ok_or_else(|| AppError::Invalid("Choose a model first.".into()))?,
    };
    let (provider, api_key) = provider_and_key(state, &conv.provider).await?;
    turns.push(Turn {
        role: TurnRole::User,
        text: HANDOFF_REQUEST.to_string(),
    });
    let mut stream = provider.generate(
        api_key,
        GenerationRequest {
            model,
            system: Some(HANDOFF_SYSTEM.to_string()),
            turns,
        },
    );
    let collect = async {
        let mut out = String::new();
        while let Some(item) = stream.next().await {
            if let GenerationEvent::Text(t) = item? {
                out.push_str(&t);
            }
        }
        Ok::<_, AppError>(out)
    };
    let text = tokio::time::timeout(Duration::from_secs(120), collect)
        .await
        .map_err(|_| AppError::Provider("Writing the summary took too long.".into()))??;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(AppError::Provider("The model returned an empty summary.".into()));
    }
    Ok(text)
}
