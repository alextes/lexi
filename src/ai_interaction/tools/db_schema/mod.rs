//! whenever writing sql queries this tool can help the ai see the full DB schema
//! for database schemas we know about.
use crate::openai_api::{
    ToolDefinition, ToolFunctionParameterPropertyBuilder, ToolFunctionParameters,
};
use anyhow::Result;
use serde_json::json;
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{error, info, instrument, warn};

pub const DATABASE_SCHEMA_TOOL_NAME: &str = "get_database_schema";
const MEVDB_SCHEMA_CONTENT: &str = include_str!("./schemas/mevdb_schema.txt");
const GLOBALDB_SCHEMA_CONTENT: &str = include_str!("./schemas/globaldb_schema.txt");

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

#[instrument(fields(db_name = %db_name))]
fn get_schema_content(db_name: &str) -> Result<String> {
    info!("retrieving {} schema content", db_name);
    // instead of reading from a file, we return the embedded content
    match db_name {
        "mevdb" => Ok(MEVDB_SCHEMA_CONTENT.to_string()),
        "globaldb" => Ok(GLOBALDB_SCHEMA_CONTENT.to_string()),
        _ => {
            // this case should ideally be caught before calling this function,
            // but as a safeguard:
            error!("invalid database name {} for schema retrieval", db_name);
            anyhow::bail!("invalid database name for schema retrieval: {}", db_name)
        }
    }
}

#[instrument(skip(arguments_json_str))]
pub async fn execute_get_database_schema(arguments_json_str: &str) -> Result<String> {
    info!(
        args = %arguments_json_str,
        "executing get_database_schema tool"
    );

    match serde_json::from_str::<GetDatabaseSchemaArgs>(arguments_json_str) {
        Ok(args) => {
            let schema_content_result = match args.database_name.as_str() {
                "mevdb" => get_schema_content("mevdb"),
                "globaldb" => get_schema_content("globaldb"),
                _ => {
                    warn!(db_name = %args.database_name, "invalid database name provided");
                    return Ok(json!({
                        "status": "error",
                        "message": "invalid_database_name",
                        "details": format!("database_name must be 'mevdb' or 'globaldb', got: {}", args.database_name)
                    }).to_string());
                }
            };

            match schema_content_result {
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
            let err_msg = format!("failed to parse arguments json: {e}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as JsonValue;

    #[tokio::test]
    async fn test_execute_get_database_schema_mevdb_success() {
        let args = json!({ "database_name": "mevdb" }).to_string();

        // the expected content is now directly from the embedded string
        let expected_content = MEVDB_SCHEMA_CONTENT;
        if expected_content.is_empty() {
            println!("warning: mevdb schema content is empty, test might not be meaningful.");
        }

        let result_str = execute_get_database_schema(&args).await.unwrap();
        assert_eq!(result_str, expected_content);
    }

    #[tokio::test]
    async fn test_execute_get_database_schema_globaldb_success() {
        let args = json!({ "database_name": "globaldb" }).to_string();

        // the expected content is now directly from the embedded string
        let expected_content = GLOBALDB_SCHEMA_CONTENT;
        if expected_content.is_empty() {
            println!("warning: globaldb schema content is empty, test might not be meaningful.");
        }

        let result_str = execute_get_database_schema(&args).await.unwrap();
        assert_eq!(result_str, expected_content);
    }

    #[tokio::test]
    async fn test_execute_get_database_schema_invalid_name() {
        let args = json!({ "database_name": "nonexistentdb" }).to_string();

        let result_str = execute_get_database_schema(&args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "invalid_database_name");
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains("nonexistentdb"));
    }

    #[tokio::test]
    async fn test_execute_get_database_schema_malformed_json_args() {
        let args = "{\"database_name\": \"mevdb\" சுகாதார"; // malformed json

        let result_str = execute_get_database_schema(args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "failed_to_parse_arguments");
    }

    #[tokio::test]
    async fn test_execute_get_database_schema_missing_database_name_arg() {
        let args = json!({}).to_string(); // empty json, missing database_name

        let result_str = execute_get_database_schema(&args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();
        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "failed_to_parse_arguments");
    }

    // test for the private helper get_schema_from_file
    // these tests need to be adapted or removed as get_schema_from_file has changed to get_schema_content
    // and no longer deals with file paths.
    #[test]
    fn test_get_schema_content_success_mevdb() {
        let result = get_schema_content("mevdb");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), MEVDB_SCHEMA_CONTENT.to_string());
    }

    #[test]
    fn test_get_schema_content_success_globaldb() {
        let result = get_schema_content("globaldb");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), GLOBALDB_SCHEMA_CONTENT.to_string());
    }

    #[test]
    fn test_get_schema_content_invalid_db() {
        let result = get_schema_content("nonexistentdb");
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("invalid database name for schema retrieval: nonexistentdb"));
    }
}
