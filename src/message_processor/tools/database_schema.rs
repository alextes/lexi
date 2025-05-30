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
    use super::*; // access to execute_get_database_schema, MEVDB_SCHEMA_FILE_PATH, etc.
    use crate::message_processor::HandlerContext; // for dummy context
    use reqwest::Client;
    use serde_json::Value as JsonValue;
    use sqlx::PgPool;
    use std::fs;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // dummy context helper
    fn dummy_handler_context<'a>(
        pool: &'a PgPool,
        http_client: &'a Client,
        openai_api_key: &'a str,
    ) -> HandlerContext<'a> {
        HandlerContext {
            pool,
            http_client,
            bot_db_id: 0,
            openai_api_key,
        }
    }

    // helper to create a temporary schema file for testing get_schema_from_file
    fn create_temp_schema_file(content: &str) -> NamedTempFile {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "{}", content).unwrap();
        temp_file
    }

    #[sqlx::test]
    async fn test_execute_get_database_schema_mevdb_success(pool: PgPool) {
        let args = json!({ "database_name": "mevdb" }).to_string();

        let http_client = Client::new();
        let openai_key = "dummy_key";
        let ctx = dummy_handler_context(&pool, &http_client, &openai_key);

        let expected_content = fs::read_to_string(MEVDB_SCHEMA_FILE_PATH).unwrap_or_default();
        if expected_content.is_empty() {
            println!(
                "warning: mevdb schema file is empty or not found, test might not be meaningful."
            );
        }

        let result_str = execute_get_database_schema(&ctx, &args).await.unwrap();
        assert_eq!(result_str, expected_content);
    }

    #[sqlx::test]
    async fn test_execute_get_database_schema_globaldb_success(pool: PgPool) {
        let args = json!({ "database_name": "globaldb" }).to_string();
        let http_client = Client::new();
        let openai_key = "dummy_key";
        let ctx = dummy_handler_context(&pool, &http_client, &openai_key);

        let expected_content = fs::read_to_string(GLOBALDB_SCHEMA_FILE_PATH).unwrap_or_default();
        if expected_content.is_empty() {
            println!("warning: globaldb schema file is empty or not found, test might not be meaningful.");
        }

        let result_str = execute_get_database_schema(&ctx, &args).await.unwrap();
        assert_eq!(result_str, expected_content);
    }

    #[sqlx::test]
    async fn test_execute_get_database_schema_invalid_name(pool: PgPool) {
        let args = json!({ "database_name": "nonexistentdb" }).to_string();
        let http_client = Client::new();
        let openai_key = "dummy_key";
        let ctx = dummy_handler_context(&pool, &http_client, &openai_key);

        let result_str = execute_get_database_schema(&ctx, &args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "invalid_database_name");
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains("nonexistentdb"));
    }

    #[sqlx::test]
    async fn test_execute_get_database_schema_malformed_json_args(pool: PgPool) {
        let args = "{\"database_name\": \"mevdb\" சுகாதார"; // malformed json
        let http_client = Client::new();
        let openai_key = "dummy_key";
        let ctx = dummy_handler_context(&pool, &http_client, &openai_key);

        let result_str = execute_get_database_schema(&ctx, args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "failed_to_parse_arguments");
    }

    #[sqlx::test]
    async fn test_execute_get_database_schema_missing_database_name_arg(pool: PgPool) {
        let args = json!({}).to_string(); // empty json, missing database_name
        let http_client = Client::new();
        let openai_key = "dummy_key";
        let ctx = dummy_handler_context(&pool, &http_client, &openai_key);

        let result_str = execute_get_database_schema(&ctx, &args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();
        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "failed_to_parse_arguments");
    }

    // test for the private helper get_schema_from_file
    #[test]
    fn test_get_schema_from_file_success() {
        let content = "test schema content here";
        let temp_file = create_temp_schema_file(content);
        let result = get_schema_from_file(temp_file.path().to_str().unwrap(), "testdb");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), format!("{}\n", content));
    }

    #[test]
    fn test_get_schema_from_file_not_found() {
        let result = get_schema_from_file("path/to/nonexistent/schema.txt", "testdb");
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("failed to read testdb schema file"));
    }
}
