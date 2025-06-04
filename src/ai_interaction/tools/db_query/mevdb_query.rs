use crate::env::ENV_CONFIG;
use crate::openai_api::{
    ToolDefinition, ToolFunctionParameterPropertyBuilder, ToolFunctionParameters,
};
use anyhow::Result;
use serde_json::json;
use sqlx::Connection;
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{error, info, instrument, warn};

use super::execute_db_query_common;

pub const MEVDB_TOOL_NAME: &str = "execute_mevdb_query";

pub static MEVDB_QUERY_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();
    params_props.insert(
        "sql_query".to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(
                "the sql query to execute against the mev-specific database. provide the full query."
            )
            .build(),
    );
    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: params_props,
        required: Some(vec!["sql_query".to_string()]),
        additional_properties: false,
    };
    ToolDefinition::new(
        MEVDB_TOOL_NAME.to_string(),
        Some(
            "executes a sql query against a read-only mev (maximal extractable value) database. provide the complete sql query. important tables and columns will be described by the assistant if known."
                .to_string(),
        ),
        Some(tool_params),
    )
});

#[instrument(skip(arguments_json_str), fields(tool_name = MEVDB_TOOL_NAME))]
pub async fn execute_mevdb_query_tool(arguments_json_str: &str) -> Result<String> {
    info!(args = %arguments_json_str, "executing execute_mevdb_query tool");

    match serde_json::from_str::<HashMap<String, String>>(arguments_json_str) {
        Ok(args_map) => {
            if let Some(sql_query_from_ai) = args_map.get("sql_query") {
                info!(query = %sql_query_from_ai, "parsed sql_query from ai arguments");

                let db_url = if let Some(url) = &ENV_CONFIG.mevdb_database_url {
                    url.as_str()
                } else {
                    warn!("MEVDB_DATABASE_URL not configured.");
                    return Ok(json!({
                        "status": "error",
                        "message": "mevdb query tool is not configured (missing MEVDB_DATABASE_URL).",
                        "details": "The MEVDB_DATABASE_URL environment variable must be set to use this tool."
                    }).to_string());
                };

                match sqlx::postgres::PgConnection::connect(db_url).await {
                    Ok(mut conn) => {
                        let result_json_value =
                            execute_db_query_common(&mut conn, sql_query_from_ai, "mevdb").await;
                        if let Err(e) = conn.close().await {
                            warn!(error = %e, "failed to close database connection");
                        }
                        Ok(result_json_value.to_string())
                    }
                    Err(e) => {
                        error!(error = %e, "failed to connect to database");
                        Ok(json!({
                            "status": "error",
                            "message": "mevdb query tool failed to connect to its database.",
                            "details": e.to_string()
                        })
                        .to_string())
                    }
                }
            } else {
                let err_msg = "argument 'sql_query' missing";
                warn!(args = %arguments_json_str, %err_msg);
                Ok(json!({
                    "status": "error",
                    "message": err_msg,
                    "details": format!("expected json with a 'sql_query' key, got: {}", arguments_json_str)
                }).to_string())
            }
        }
        Err(e) => {
            let err_msg = format!("failed to parse arguments json: {e}");
            warn!(args = %arguments_json_str, error = %e, "json parsing error for tool arguments");
            Ok(json!({
                "status": "error",
                "message": "failed to parse tool arguments as json.",
                "details": err_msg
            })
            .to_string())
        }
    }
}
