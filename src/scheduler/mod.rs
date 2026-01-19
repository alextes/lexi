pub mod executor;
pub mod service;
pub mod types;

use crate::ai_interaction::tools::beacon_slot_check::BeaconNode;
use crate::ai_interaction::tools::db_schema::LiveSchemaFetcher;
use crate::ai_interaction::tools::relay_circuit_breaker::RelayCircuitBreaker;
use crate::bot::BotContext;
use crate::db::Db;
use reqwest::Client as ReqwestClient;

/// Context required by the scheduler to execute jobs.
/// Contains all dependencies needed to call AI and send Telegram messages.
#[derive(Clone)]
pub struct SchedulerContext<D: Db, B: BeaconNode> {
    pub db: D,
    pub http_client: ReqwestClient,
    pub api_base_url: String,
    pub bot_token: String,
    pub bot_db_id: i32,
    pub openai_api_key: String,
    pub schema_fetcher: LiveSchemaFetcher,
    pub beacon_node: B,
    pub relay_circuit_breaker: RelayCircuitBreaker,
}

impl<D: Db + Clone>
    SchedulerContext<D, crate::ai_interaction::tools::beacon_slot_check::BeaconNodeHttp>
{
    /// Create a SchedulerContext from an existing BotContext
    pub fn from_bot_context(ctx: &BotContext<D>) -> Self {
        Self {
            db: ctx.db.clone(),
            http_client: ctx.http_client.clone(),
            api_base_url: ctx.api_base_url.clone(),
            bot_token: ctx.bot_token.clone(),
            bot_db_id: ctx.bot_db_id,
            openai_api_key: ctx.openai_api_key.clone(),
            schema_fetcher: ctx.schema_fetcher.clone(),
            beacon_node: ctx.beacon_node.clone(),
            relay_circuit_breaker: ctx.relay_circuit_breaker.clone(),
        }
    }
}
