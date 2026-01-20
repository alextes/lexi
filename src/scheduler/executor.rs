use crate::ai_interaction::tools::beacon_slot_check::BeaconNode;
use crate::ai_interaction::{
    drive_ai_conversation, get_current_model_id, AiConversationOutcome, HandlerContext,
};
use crate::bot::conversation_context::{self, ConversationKey};
use crate::db::Db;
use crate::scheduler::types::ScheduledJob;
use crate::telegram;
use anyhow::Result;
use tracing::{info, warn};

use super::SchedulerContext;

/// Execute a scheduled job by sending its prompt to OpenAI and delivering the response to Telegram.
pub async fn execute_scheduled_job<D, B>(
    job: &ScheduledJob,
    ctx: &SchedulerContext<D, B>,
) -> Result<()>
where
    D: Db + Clone,
    B: BeaconNode + Clone,
{
    info!(job_name = %job.name, job_id = job.id, "executing scheduled job");

    // Look up local_chat_id to access conversation context
    let local_chat_id = ctx
        .db
        .get_local_chat_id_by_telegram_id(job.telegram_chat_id)
        .await?;

    // Get previous response_id for conversation continuity (if chat exists in DB)
    let (previous_response_id, conversation_key) = match local_chat_id {
        Some(id) => {
            let key = ConversationKey::new(id, job.message_thread_id);
            let prev_id = conversation_context::get_response_id(&ctx.db, &key)
                .await
                .ok()
                .flatten();
            (prev_id, Some(key))
        }
        None => {
            info!(
                job_name = %job.name,
                telegram_chat_id = job.telegram_chat_id,
                "chat not found in database, proceeding without conversation context"
            );
            (None, None)
        }
    };

    // Build handler context with full tool access (same as regular messages)
    let handler_ctx = HandlerContext {
        db: ctx.db.clone(),
        http_client: ctx.http_client.clone(),
        bot_db_id: ctx.bot_db_id,
        openai_api_key: ctx.openai_api_key.clone(),
        beacon_node: ctx.beacon_node.clone(),
        relay_circuit_breaker: ctx.relay_circuit_breaker.clone(),
        schema_fetcher: ctx.schema_fetcher.clone(),
        // Pass chat context so scheduled jobs can manage their own jobs if needed
        current_telegram_chat_id: Some(job.telegram_chat_id),
        current_message_thread_id: job.message_thread_id,
    };

    // Use existing AI conversation driver with full tools
    let current_model = get_current_model_id().await;
    let outcome = drive_ai_conversation(
        &handler_ctx,
        &job.prompt,
        previous_response_id.as_deref(),
        &current_model,
        false, // No admin session for scheduled tasks
    )
    .await?;

    // Extract response text and response_id from outcome
    let (response_text, response_id) = match outcome {
        AiConversationOutcome::TextMessage(text, id) => (text, id),
        AiConversationOutcome::ResetConversation(text, id) => (text, id),
        AiConversationOutcome::ChangeModel(text, _, id) => (text, id),
        AiConversationOutcome::ChangeVerbosity(text, _, id) => (text, id),
        AiConversationOutcome::ChangeReasoningEffort(text, _, id) => (text, id),
        AiConversationOutcome::EndAdminSession(text, id) => (text, id),
    };

    // Store the new response_id for conversation continuity
    if let Some(ref key) = conversation_key {
        if let Err(e) = conversation_context::update_response_id(&ctx.db, key, &response_id).await {
            warn!(
                job_name = %job.name,
                error = %e,
                "failed to update response_id for scheduled job"
            );
        }
    }

    // Send to Telegram (split into multiple messages if needed)
    telegram::send_long_message(
        &ctx.http_client,
        &ctx.api_base_url,
        &ctx.bot_token,
        job.telegram_chat_id,
        &response_text,
        job.message_thread_id,
        Some("Markdown"),
    )
    .await?;

    info!(job_name = %job.name, job_id = job.id, "scheduled job executed successfully");
    Ok(())
}
