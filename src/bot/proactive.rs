use super::BotContext;
use crate::ai_interaction::tools::beacon_slot_check::BeaconNodeHttp;
use crate::db::Db;
use crate::telegram;
use reqwest::Client as ReqwestClient;
use tracing::{info, warn};

// proactive trigger configuration
const PROACTIVE_CHAT_ID: i64 = -702100346;
const PROACTIVE_PREFIX: &str = "Proposer bid failed simulation in slot";
const PROACTIVE_DELAY_SECS: u64 = 36;

fn extract_slot_number_from_message(message: &str) -> Option<u64> {
    // prefer extracting the first integer after the configured prefix
    if let Some(rest) = message.strip_prefix(PROACTIVE_PREFIX) {
        let digits = rest
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>();

        if !digits.is_empty() {
            return digits.parse::<u64>().ok();
        }
    }

    // fallback: first integer token anywhere in the message
    message
        .split_whitespace()
        .filter_map(|tok| tok.parse::<u64>().ok())
        .next()
}

#[derive(Clone)]
struct ProactiveContext {
    http_client: ReqwestClient,
    api_base_url: String,
    bot_token: String,
    beacon_node: BeaconNodeHttp,
}

impl ProactiveContext {
    fn from_bot_context<D: Db>(ctx: &BotContext<D>) -> Self {
        Self {
            http_client: ctx.http_client.clone(),
            api_base_url: ctx.api_base_url.clone(),
            bot_token: ctx.bot_token.clone(),
            beacon_node: ctx.beacon_node.clone(),
        }
    }
}

async fn run_proactive_slot_check(
    ctx: ProactiveContext,
    telegram_chat_id: i64,
    full_message: String,
) {
    use std::time::Duration;

    tokio::time::sleep(Duration::from_secs(PROACTIVE_DELAY_SECS)).await;

    let slot_opt = extract_slot_number_from_message(&full_message);

    let reply_text = if let Some(slot) = slot_opt {
        let args_json = serde_json::json!({ "slot_number": slot }).to_string();
        match crate::ai_interaction::tools::beacon_slot_check::execute_beacon_slot_check(
            &ctx.beacon_node,
            &args_json,
        )
        .await
        {
            Ok(result_json) => match serde_json::from_str::<serde_json::Value>(&result_json) {
                Ok(val) => {
                    let status = val
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("error");
                    match status {
                        "not_missed" => format!("slot {} not missed", slot),
                        "missed" => format!("slot {} missed", slot),
                        _ => {
                            let details = val.get("details").and_then(|v| v.as_str()).unwrap_or("");
                            if details.is_empty() {
                                format!("error checking slot {}", slot)
                            } else {
                                format!("error checking slot {}: {}", slot, details)
                            }
                        }
                    }
                }
                Err(_) => format!("error checking slot {}", slot),
            },
            Err(e) => format!("error checking slot {}: {}", slot, e),
        }
    } else {
        "could not extract slot number from message".to_string()
    };

    if let Err(e) = telegram::send_message(
        &ctx.http_client,
        ctx.api_base_url.as_str(),
        ctx.bot_token.as_str(),
        telegram_chat_id,
        &reply_text,
        None, // Proactive messages go to main chat, not a topic
        Some("Markdown"),
    )
    .await
    {
        warn!(error = %e, "failed to send proactive slot check reply");
    } else {
        info!(
            chat_id = telegram_chat_id,
            "sent proactive slot check reply"
        );
    }
}

pub fn maybe_schedule_proactive_slot_check<D: Db>(
    ctx: &BotContext<D>,
    telegram_chat_id: i64,
    message_text: &str,
) {
    if telegram_chat_id != PROACTIVE_CHAT_ID {
        return;
    }

    if !message_text.starts_with(PROACTIVE_PREFIX) {
        return;
    }

    info!(
        chat_id = telegram_chat_id,
        delay_secs = PROACTIVE_DELAY_SECS,
        "proactive slot check trigger matched; scheduling follow-up"
    );

    let ctx_clone = ProactiveContext::from_bot_context(ctx);
    let full_message = message_text.to_string();

    // spawn without blocking the update loop
    tokio::spawn(run_proactive_slot_check(
        ctx_clone,
        telegram_chat_id,
        full_message,
    ));
}
