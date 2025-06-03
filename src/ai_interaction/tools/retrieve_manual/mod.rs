//! tools for retrieving manuals from the filesystem
//!
//! manuals are stored in the `src/ai_interaction/tools/retrieve_manual/manuals` directory
//! and are named like `generate_proposer_reimbursement.md`
//!
//! the tool is used to retrieve the content of a manual for the assistant to use
//! when responding to user messages.
use crate::openai_api::{
    ToolDefinition, ToolFunctionParameterPropertyBuilder, ToolFunctionParameters,
};
use eyre::Result;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;
use tracing::{error, info, instrument, warn};

pub const RETRIEVE_MANUAL_TOOL_NAME: &str = "retrieve_manual";
const GENERATE_PROPOSER_REIMBURSEMENT_MANUAL_NAME: &str = "generate_proposer_reimbursement";
const MANUALS_DIR_PATH: &str = "src/ai_interaction/tools/retrieve_manual/manuals/";

#[derive(Debug, serde::Deserialize)]
struct RetrieveManualArgs {
    manual_name: String,
}

pub static RETRIEVE_MANUAL_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();
    params_props.insert(
        "manual_name".to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(
                "the name of the manual to retrieve. available manuals: 'generate_proposer_reimbursement' (explains how to generate a proposer reimbursement).",
            )
            .enum_string(&[GENERATE_PROPOSER_REIMBURSEMENT_MANUAL_NAME])
            .build(),
    );
    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: params_props,
        required: Some(vec!["manual_name".to_string()]),
        additional_properties: false,
    };
    ToolDefinition::new(
        RETRIEVE_MANUAL_TOOL_NAME.to_string(),
        Some(
            "retrieves the content of a specified manual. this tool provides access to instructional documents for various tasks."
                .to_string(),
        ),
        Some(tool_params),
    )
});

#[instrument(fields(manual_name = %manual_name))]
fn get_manual_content_from_file(manual_name: &str) -> Result<String> {
    let file_name = format!("{manual_name}.md");
    let file_path_str = format!("{MANUALS_DIR_PATH}{file_name}");
    let file_path = Path::new(&file_path_str);
    info!(
        "attempting to read manual '{}' from file: {}",
        manual_name,
        file_path.display()
    );
    fs::read_to_string(file_path).map_err(|e| {
        error!(error = %e, path = %file_path.display(), "failed to read manual file");
        eyre::eyre!(
            "failed to read manual file '{}': {}",
            file_path.display(),
            e
        )
    })
}

#[instrument(skip(arguments_json_str), fields(tool_name = RETRIEVE_MANUAL_TOOL_NAME))]
pub async fn execute_retrieve_manual(arguments_json_str: &str) -> Result<String> {
    info!(
        args = %arguments_json_str,
        "executing retrieve_manual tool"
    );

    match serde_json::from_str::<RetrieveManualArgs>(arguments_json_str) {
        Ok(args) => {
            // validate manual name (though enum in definition should prevent this)
            match args.manual_name.as_str() {
                GENERATE_PROPOSER_REIMBURSEMENT_MANUAL_NAME => {
                    match get_manual_content_from_file(&args.manual_name) {
                        Ok(content) => Ok(json!({
                            "manual_name": args.manual_name,
                            "manual_content": content
                        })
                        .to_string()),
                        Err(e) => {
                            warn!(manual_name = %args.manual_name, error = %e, "error getting manual for tool call");
                            Ok(json!({
                                "status": "error",
                                "message": "failed_to_read_manual_file",
                                "details": e.to_string()
                            })
                            .to_string())
                        }
                    }
                }
                _ => {
                    // this case should ideally not be reached if openai respects the enum
                    warn!(manual_name = %args.manual_name, "invalid manual name provided");
                    Ok(json!({
                        "status": "error",
                        "message": "invalid_manual_name",
                        "details": format!("manual_name must be one of the defined enum values, got: {}", args.manual_name)
                    }).to_string())
                }
            }
        }
        Err(e) => {
            let err_msg = format!("failed to parse arguments for retrieve_manual: {e}");
            warn!(args = %arguments_json_str, error = %err_msg);
            Ok(json!({
                "status": "error",
                "message": "argument_parsing_error",
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
    use std::fs;
    use std::io::Write;
    use std::path::Path;

    #[tokio::test]
    async fn test_execute_retrieve_manual_success() {
        let manual_name = GENERATE_PROPOSER_REIMBURSEMENT_MANUAL_NAME;
        let args = json!({ "manual_name": manual_name }).to_string();

        let expected_file_path = format!("{}{}.md", MANUALS_DIR_PATH, manual_name);
        let expected_content = fs::read_to_string(&expected_file_path).unwrap_or_else(|e| {
            panic!(
                "failed to read the actual manual file for test: {}. error: {}",
                expected_file_path, e
            )
        });

        let result_str = execute_retrieve_manual(&args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["manual_name"], manual_name);
        assert_eq!(result_json["manual_content"], expected_content);
    }

    #[tokio::test]
    async fn test_execute_retrieve_manual_invalid_name() {
        let manual_name = "non_existent_manual";
        let args = json!({ "manual_name": manual_name }).to_string();

        let result_str = execute_retrieve_manual(&args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "invalid_manual_name");
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains(manual_name));
    }

    #[tokio::test]
    async fn test_execute_retrieve_manual_file_not_found_for_valid_enum() {
        let manual_name = GENERATE_PROPOSER_REIMBURSEMENT_MANUAL_NAME;
        let actual_file_path = format!("{}{}.md", MANUALS_DIR_PATH, manual_name);
        let temp_renamed_path = format!("{}.temp_test_backup", actual_file_path);
        let mut file_renamed = false;

        if Path::new(&actual_file_path).exists() {
            fs::rename(&actual_file_path, &temp_renamed_path).unwrap();
            file_renamed = true;
        }

        let args = json!({ "manual_name": manual_name }).to_string();

        let result_str = execute_retrieve_manual(&args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "failed_to_read_manual_file");

        if file_renamed {
            fs::rename(&temp_renamed_path, &actual_file_path).unwrap();
        }
    }

    #[tokio::test]
    async fn test_execute_retrieve_manual_malformed_json_args() {
        let args = "{\"manual_name\": \"name\" சுகாதார"; // malformed json

        let result_str = execute_retrieve_manual(args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "argument_parsing_error");
    }

    #[test]
    fn test_get_manual_content_from_file_success() {
        // This test requires a known file to exist at the location specified by MANUALS_DIR_PATH
        // For true unit testing, this function would need to be refactored or the fs module mocked.
        // We will test with GENERATE_PROPOSER_REIMBURSEMENT_MANUAL_NAME assuming it exists.
        let manual_name = GENERATE_PROPOSER_REIMBURSEMENT_MANUAL_NAME;
        let expected_file_path = format!("{}{}.md", MANUALS_DIR_PATH, manual_name);

        if !Path::new(&expected_file_path).exists() {
            // Create a dummy file for the test if it doesn't exist to prevent panic
            // This is a workaround for environments where the file might be missing.
            let dummy_content = "dummy content for test_get_manual_content_from_file_success";
            let parent_dir = Path::new(&expected_file_path).parent().unwrap();
            fs::create_dir_all(parent_dir).unwrap();
            let mut file = fs::File::create(&expected_file_path).unwrap();
            write!(file, "{}", dummy_content).unwrap();

            let result = get_manual_content_from_file(manual_name);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), dummy_content);

            // Clean up dummy file
            fs::remove_file(&expected_file_path).unwrap();
        } else {
            let expected_content = fs::read_to_string(&expected_file_path).unwrap();
            let result = get_manual_content_from_file(manual_name);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), expected_content);
        }
    }

    #[test]
    fn test_get_manual_content_from_file_not_found() {
        let result = get_manual_content_from_file("surely_this_manual_does_not_exist_for_test");
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("failed to read manual file"));
        assert!(err_msg.contains("surely_this_manual_does_not_exist_for_test.md"));
    }
}
