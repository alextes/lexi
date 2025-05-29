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

pub const RETRIEVE_MANUAL_TOOL_NAME: &str = "retrieve_manual";
const GENERATE_PROPOSER_REIMBURSEMENT_MANUAL_NAME: &str = "generate_proposer_reimbursement_manual";
const MANUALS_DIR_PATH: &str = "src/message_processor/tools/manuals/";

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
                "the name of the manual to retrieve. available manuals: 'generate_proposer_reimbursement_manual' (explains how to generate a proposer reimbursement).",
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

pub async fn execute_retrieve_manual(
    _ctx: &HandlerContext<'_>,
    arguments_json_str: &str,
) -> Result<String> {
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
