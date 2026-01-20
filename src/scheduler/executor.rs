use crate::ai_interaction::tools::beacon_slot_check::BeaconNode;
use crate::ai_interaction::{
    drive_ai_conversation, get_current_model_id, AiConversationOutcome, HandlerContext,
};
use crate::db::Db;
use crate::scheduler::types::ScheduledJob;
use crate::telegram;
use anyhow::Result;
use tracing::info;

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
        None, // No previous_response_id for scheduled tasks
        &current_model,
        false, // No admin session for scheduled tasks
    )
    .await?;

    // Extract response text from outcome
    let response_text = match outcome {
        AiConversationOutcome::TextMessage(text, _response_id) => text,
        AiConversationOutcome::ResetConversation(text, _) => text,
        AiConversationOutcome::ChangeModel(text, _, _) => text,
        AiConversationOutcome::ChangeVerbosity(text, _, _) => text,
        AiConversationOutcome::ChangeReasoningEffort(text, _, _) => text,
        AiConversationOutcome::EndAdminSession(text, _) => text,
    };

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
