//! whenever writing sql queries this tool can help the ai see the full DB schema
//! for database schemas we know about.
use crate::message_processor::HandlerContext;
use crate::openai_api::{
    ToolDefinition, ToolFunctionParameterPropertyBuilder, ToolFunctionParameters,
};
use eyre::Result;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;
use tracing::{error, info, warn};

pub const DATABASE_SCHEMA_TOOL_NAME: &str = "get_database_schema";
const MEVDB_SCHEMA_FILE_PATH: &str = "src/message_processor/tools/schemas/mevdb_schema.txt";
const GLOBALDB_SCHEMA_FILE_PATH: &str = "src/message_processor/tools/schemas/globaldb_schema.txt";

#[derive(Debug, serde::Deserialize)]
struct GetDatabaseSchemaArgs {
    database_name: String,
}

pub static DATABASE_SCHEMA_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();
    params_props.insert(
        "database_name".to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(
                "the name of the database to get the schema for. must be one of 'mevdb' or 'globaldb'.",
            )
            .enum_string(&["mevdb", "globaldb"])
            .build(),
    );
    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: params_props,
        required: Some(vec!["database_name".to_string()]),
        additional_properties: false,
    };
    ToolDefinition::new(
        DATABASE_SCHEMA_TOOL_NAME.to_string(),
        Some(
            "retrieves the schema definition for a specified database. this can be used to understand table structures before forming a query for the corresponding 'execute_<db_name>_query' tool."
                .to_string(),
        ),
        Some(tool_params),
    )
});

fn get_schema_from_file(file_path: &str, db_name: &str) -> Result<String> {
    info!(
        "attempting to read {} schema from file: {}",
        db_name, file_path
    );
    fs::read_to_string(Path::new(file_path)).map_err(|e| {
        error!(error = %e, path = file_path, "failed to read {} schema file", db_name);
        eyre::eyre!("failed to read {} schema file: {}", db_name, e)
    })
}

pub async fn execute_get_database_schema(
    _ctx: &HandlerContext<'_>,
    arguments_json_str: &str,
) -> Result<String> {
    info!(
        args = %arguments_json_str,
        "executing get_database_schema tool"
    );

    match serde_json::from_str::<GetDatabaseSchemaArgs>(arguments_json_str) {
        Ok(args) => {
            let schema_file_path = match args.database_name.as_str() {
                "mevdb" => MEVDB_SCHEMA_FILE_PATH,
                "globaldb" => GLOBALDB_SCHEMA_FILE_PATH,
                _ => {
                    warn!(db_name = %args.database_name, "invalid database name provided");
                    return Ok(json!({
                        "status": "error",
                        "message": "invalid_database_name",
                        "details": format!("database_name must be 'mevdb' or 'globaldb', got: {}", args.database_name)
                    }).to_string());
                }
            };

            match get_schema_from_file(schema_file_path, &args.database_name) {
                Ok(s) => Ok(s),
                Err(e) => {
                    warn!(db_name = %args.database_name, error = %e, "error getting schema for tool call");
                    Ok(json!({
                        "status": "error",
                        "message": "failed_to_read_schema",
                        "details": e.to_string()
                    })
                    .to_string())
                }
            }
        }
        Err(e) => {
            let err_msg = format!("failed to parse arguments json: {}", e);
            warn!(args = %arguments_json_str, error = %e, "json parsing error for tool arguments");
            Ok(json!({
                "status": "error",
                "message": "failed_to_parse_arguments",
                "details": err_msg
            })
            .to_string())
        }
    }
}
