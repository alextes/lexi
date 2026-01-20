pub mod types;

use anyhow::{anyhow, Context, Result};
use reqwest::Client as ReqwestClient;
use serde::Serialize;

use types::{ApiResponse, Message as TelegramMessage, User as TelegramUser};

/// Telegram's maximum message length
const MAX_MESSAGE_LENGTH: usize = 4096;

// Keep TELEGRAM_API_URL in main.rs as it's used there for getUpdates too,
// or move to a shared consts.rs if more URLs are added.
// For now, we'll reconstruct it or pass it.
// Let's assume main.rs TELEGRAM_API_URL is accessible or pass token directly.

#[derive(Serialize, Debug)]
struct SendMessageParams<'a> {
    chat_id: i64,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<&'a str>,
}

// Function to send a message via Telegram API
pub async fn send_message(
    http_client: &ReqwestClient,
    api_base_url: &str, // e.g., "https://api.telegram.org/bot"
    bot_token: &str,
    chat_id: i64,
    text: &str,
    message_thread_id: Option<i64>,
    parse_mode: Option<&str>,
) -> Result<TelegramMessage> {
    let url = format!("{api_base_url}{bot_token}/sendMessage");

    let params = SendMessageParams {
        chat_id,
        text,
        message_thread_id,
        parse_mode,
    };

    tracing::debug!(url = %url, ?params, "sending message to telegram");

    let response = http_client
        .post(&url)
        .json(&params)
        .send()
        .await
        .with_context(|| format!("failed to send POST request to {url}"))?;

    let status = response.status();
    if status.is_success() {
        let api_response = response
            .json::<ApiResponse<TelegramMessage>>()
            .await
            .with_context(|| "failed to parse JSON response from sendMessage API")?;

        if api_response.ok {
            api_response.result.ok_or_else(|| {
                anyhow!("sendMessage API call was ok but no message was returned in result")
            })
        } else {
            let error_description = api_response
                .description
                .unwrap_or_else(|| "unknown telegram api error".to_string());
            let error_code = api_response.error_code.unwrap_or(-1);
            tracing::error!(description = %error_description, code = error_code, "telegram sendMessage API error (ok=false)");
            Err(anyhow!(
                "telegram sendMessage API error ({}): {}",
                error_code,
                error_description
            ))
        }
    } else {
        let error_body = response.text().await.unwrap_or_default();
        tracing::error!(status = %status, body = %error_body, "http error during sendMessage");
        Err(anyhow!(
            "http error {} during sendMessage: {}",
            status,
            error_body
        ))
    }
}

/// Split a long message into chunks that fit Telegram's limit.
/// Tries to split at newlines or spaces when possible.
fn split_message(text: &str) -> Vec<&str> {
    if text.len() <= MAX_MESSAGE_LENGTH {
        return vec![text];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= MAX_MESSAGE_LENGTH {
            chunks.push(remaining);
            break;
        }

        // Find a good split point (prefer newline, then space)
        let chunk = &remaining[..MAX_MESSAGE_LENGTH];
        let split_at = chunk
            .rfind('\n')
            .or_else(|| chunk.rfind(' '))
            .unwrap_or(MAX_MESSAGE_LENGTH);

        chunks.push(&remaining[..split_at]);
        remaining = remaining[split_at..].trim_start();
    }

    chunks
}

/// Send a message, splitting into multiple messages if it exceeds Telegram's limit.
/// If parse_mode is set and sending fails due to entity parsing errors, retries without formatting.
/// Returns the last sent message (for storing message IDs, etc.)
pub async fn send_long_message(
    http_client: &ReqwestClient,
    api_base_url: &str,
    bot_token: &str,
    chat_id: i64,
    text: &str,
    message_thread_id: Option<i64>,
    parse_mode: Option<&str>,
) -> Result<TelegramMessage> {
    let chunks = split_message(text);
    let mut last_message = None;

    for chunk in chunks {
        // Try with parse_mode first
        let result = send_message(
            http_client,
            api_base_url,
            bot_token,
            chat_id,
            chunk,
            message_thread_id,
            parse_mode,
        )
        .await;

        // If it failed due to Markdown parsing and we had a parse_mode, retry without it
        let message = match result {
            Ok(msg) => msg,
            Err(e) if parse_mode.is_some() && e.to_string().contains("can't parse entities") => {
                tracing::warn!(
                    "Markdown parsing failed, retrying chunk without parse_mode: {}",
                    e
                );
                send_message(
                    http_client,
                    api_base_url,
                    bot_token,
                    chat_id,
                    chunk,
                    message_thread_id,
                    None, // No parse_mode
                )
                .await?
            }
            Err(e) => return Err(e),
        };

        last_message = Some(message);
    }

    last_message.ok_or_else(|| anyhow!("no message chunks to send"))
}

// Function to get the bot's own user details
pub async fn get_me(
    http_client: &ReqwestClient,
    api_base_url: &str,
    bot_token: &str,
) -> Result<TelegramUser> {
    let url = format!("{api_base_url}{bot_token}/getMe");
    tracing::debug!(url = %url, "fetching bot details (getMe)");

    let response = http_client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed to send GET request to {url}"))?;

    let status = response.status();
    if status.is_success() {
        let api_response = response
            .json::<ApiResponse<TelegramUser>>()
            .await
            .with_context(|| "failed to parse JSON response from getMe API")?;

        if api_response.ok {
            api_response
                .result
                .ok_or_else(|| anyhow!("getMe API call was ok but no user was returned in result"))
        } else {
            let error_description = api_response
                .description
                .unwrap_or_else(|| "unknown telegram api error".to_string());
            let error_code = api_response.error_code.unwrap_or(-1);
            tracing::error!(description = %error_description, code = error_code, "telegram getMe API error (ok=false)");
            Err(anyhow!(
                "telegram getMe API error ({}): {}",
                error_code,
                error_description
            ))
        }
    } else {
        let error_body = response.text().await.unwrap_or_default();
        tracing::error!(status = %status, body = %error_body, "http error during getMe");
        Err(anyhow!(
            "http error {} during getMe: {}",
            status,
            error_body
        ))
    }
}
