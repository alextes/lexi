use crate::db::Db;
use crate::openai_api::{
    ToolDefinition, ToolFunctionParameterPropertyBuilder, ToolFunctionParameters,
};
use anyhow::Result;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{info, instrument, warn};

pub const SCHEDULED_JOBS_TOOL_NAME: &str = "scheduled_jobs";

// Command names
const LIST_COMMAND: &str = "list";
const ADD_COMMAND: &str = "add";
const EDIT_COMMAND: &str = "edit";
const DELETE_COMMAND: &str = "delete";
const ENABLE_COMMAND: &str = "enable";
const DISABLE_COMMAND: &str = "disable";

// Parameter names
const COMMAND_PARAM: &str = "command";
const NAME_PARAM: &str = "name";
const CRON_SCHEDULE_PARAM: &str = "cron_schedule";
const PROMPT_PARAM: &str = "prompt";

pub static SCHEDULED_JOBS_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();

    params_props.insert(
        COMMAND_PARAM.to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(
                "The operation to perform: 'list' to show all scheduled jobs, 'add' to create a new job, 'edit' to modify an existing job's schedule or prompt, 'delete' to remove a job, 'enable' or 'disable' to toggle a job's active status.",
            )
            .enum_string(&[LIST_COMMAND, ADD_COMMAND, EDIT_COMMAND, DELETE_COMMAND, ENABLE_COMMAND, DISABLE_COMMAND])
            .build(),
    );

    params_props.insert(
        NAME_PARAM.to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(
                "The unique name for the scheduled job. Required for add/edit/delete/enable/disable commands.",
            )
            .build(),
    );

    params_props.insert(
        CRON_SCHEDULE_PARAM.to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(
                "Cron schedule expression (e.g., '0 9 * * *' for daily at 9am UTC, '*/5 * * * *' for every 5 minutes). Required for add command, optional for edit command.",
            )
            .build(),
    );

    params_props.insert(
        PROMPT_PARAM.to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(
                "The prompt to send to the AI when the scheduled job runs. Required for add command, optional for edit command.",
            )
            .build(),
    );

    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: params_props,
        required: Some(vec![COMMAND_PARAM.to_string()]),
        additional_properties: false,
    };

    ToolDefinition::new(
        SCHEDULED_JOBS_TOOL_NAME.to_string(),
        Some(
            "Manages scheduled jobs for this chat. Jobs run on a cron schedule and send the specified prompt to the AI, with the response sent back to this chat."
                .to_string(),
        ),
        Some(tool_params),
    )
});

#[instrument(skip(db, arguments_json_str))]
pub async fn execute_scheduled_jobs<D: Db>(
    db: &D,
    telegram_chat_id: Option<i64>,
    message_thread_id: Option<i64>,
    arguments_json_str: &str,
) -> Result<String> {
    info!(args = %arguments_json_str, "executing scheduled_jobs tool");

    // Validate we have chat context
    let chat_id = match telegram_chat_id {
        Some(id) => id,
        None => {
            let err_msg = "cannot manage scheduled jobs: no chat context available";
            warn!(args = %arguments_json_str, error = %err_msg);
            return Ok(json!({
                "status": "error",
                "message": err_msg
            })
            .to_string());
        }
    };

    // Parse arguments
    let args: JsonValue = match serde_json::from_str(arguments_json_str) {
        Ok(v) => v,
        Err(e) => {
            let err_msg = format!("failed to parse arguments JSON: {e}");
            warn!(args = %arguments_json_str, error = %err_msg);
            return Ok(json!({
                "status": "error",
                "message": "malformed_json",
                "details": err_msg
            })
            .to_string());
        }
    };

    let command = args
        .get(COMMAND_PARAM)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match command {
        LIST_COMMAND => execute_list(db, chat_id, message_thread_id).await,
        ADD_COMMAND => execute_add(db, chat_id, message_thread_id, &args).await,
        EDIT_COMMAND => execute_edit(db, chat_id, message_thread_id, &args).await,
        DELETE_COMMAND => execute_delete(db, chat_id, message_thread_id, &args).await,
        ENABLE_COMMAND => execute_set_enabled(db, chat_id, message_thread_id, &args, true).await,
        DISABLE_COMMAND => execute_set_enabled(db, chat_id, message_thread_id, &args, false).await,
        _ => {
            let err_msg = format!("unknown command: '{command}'");
            warn!(args = %arguments_json_str, error = %err_msg);
            Ok(json!({
                "status": "error",
                "message": "unknown_command",
                "details": err_msg
            })
            .to_string())
        }
    }
}

async fn execute_list<D: Db>(
    db: &D,
    telegram_chat_id: i64,
    message_thread_id: Option<i64>,
) -> Result<String> {
    match db
        .get_scheduled_jobs_for_chat(telegram_chat_id, message_thread_id)
        .await
    {
        Ok(jobs) => {
            let jobs_info: Vec<JsonValue> = jobs
                .iter()
                .map(|j| {
                    json!({
                        "name": j.name,
                        "cron_schedule": j.cron_schedule,
                        "prompt": j.prompt,
                        "enabled": j.enabled
                    })
                })
                .collect();

            Ok(json!({
                "status": "success",
                "jobs": jobs_info,
                "count": jobs.len()
            })
            .to_string())
        }
        Err(e) => {
            let err_msg = format!("failed to fetch scheduled jobs: {e}");
            warn!(error = %err_msg);
            Ok(json!({
                "status": "error",
                "message": "database_error",
                "details": err_msg
            })
            .to_string())
        }
    }
}

async fn execute_add<D: Db>(
    db: &D,
    telegram_chat_id: i64,
    message_thread_id: Option<i64>,
    args: &JsonValue,
) -> Result<String> {
    let name = match args.get(NAME_PARAM).and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n,
        _ => {
            return Ok(json!({
                "status": "error",
                "message": "missing_parameter",
                "details": format!("'{}' is required for add command", NAME_PARAM)
            })
            .to_string());
        }
    };

    let cron_schedule = match args.get(CRON_SCHEDULE_PARAM).and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Ok(json!({
                "status": "error",
                "message": "missing_parameter",
                "details": format!("'{}' is required for add command", CRON_SCHEDULE_PARAM)
            })
            .to_string());
        }
    };

    let prompt = match args.get(PROMPT_PARAM).and_then(|v| v.as_str()) {
        Some(p) if !p.is_empty() => p,
        _ => {
            return Ok(json!({
                "status": "error",
                "message": "missing_parameter",
                "details": format!("'{}' is required for add command", PROMPT_PARAM)
            })
            .to_string());
        }
    };

    // Validate cron schedule
    if let Err(e) = croner::Cron::new(cron_schedule).parse() {
        return Ok(json!({
            "status": "error",
            "message": "invalid_cron_schedule",
            "details": format!("Invalid cron expression '{}': {}", cron_schedule, e)
        })
        .to_string());
    }

    match db
        .create_scheduled_job(
            name,
            cron_schedule,
            prompt,
            telegram_chat_id,
            message_thread_id,
        )
        .await
    {
        Ok(job_id) => {
            info!(job_id = job_id, name = %name, cron = %cron_schedule, "created scheduled job");
            Ok(json!({
                "status": "success",
                "message": format!("Created scheduled job '{}' with schedule '{}'", name, cron_schedule),
                "job_id": job_id
            })
            .to_string())
        }
        Err(e) => {
            let err_msg = format!("failed to create scheduled job: {e}");
            warn!(error = %err_msg);
            Ok(json!({
                "status": "error",
                "message": "database_error",
                "details": err_msg
            })
            .to_string())
        }
    }
}

async fn execute_edit<D: Db>(
    db: &D,
    telegram_chat_id: i64,
    message_thread_id: Option<i64>,
    args: &JsonValue,
) -> Result<String> {
    let name = match args.get(NAME_PARAM).and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n,
        _ => {
            return Ok(json!({
                "status": "error",
                "message": "missing_parameter",
                "details": format!("'{}' is required for edit command", NAME_PARAM)
            })
            .to_string());
        }
    };

    let cron_schedule = args
        .get(CRON_SCHEDULE_PARAM)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    let prompt = args
        .get(PROMPT_PARAM)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    // At least one of cron_schedule or prompt must be provided
    if cron_schedule.is_none() && prompt.is_none() {
        return Ok(json!({
            "status": "error",
            "message": "missing_parameter",
            "details": "At least one of 'cron_schedule' or 'prompt' must be provided for edit command"
        })
        .to_string());
    }

    // Validate cron schedule if provided
    if let Some(schedule) = cron_schedule {
        if let Err(e) = croner::Cron::new(schedule).parse() {
            return Ok(json!({
                "status": "error",
                "message": "invalid_cron_schedule",
                "details": format!("Invalid cron expression '{}': {}", schedule, e)
            })
            .to_string());
        }
    }

    match db
        .update_scheduled_job(
            name,
            telegram_chat_id,
            message_thread_id,
            cron_schedule.map(|s| s.to_string()),
            prompt.map(|s| s.to_string()),
        )
        .await
    {
        Ok(updated) => {
            if updated {
                let mut changes = Vec::new();
                if let Some(s) = cron_schedule {
                    changes.push(format!("schedule: '{}'", s));
                }
                if prompt.is_some() {
                    changes.push("prompt".to_string());
                }
                info!(name = %name, changes = ?changes, "updated scheduled job");
                Ok(json!({
                    "status": "success",
                    "message": format!("Updated scheduled job '{}' ({})", name, changes.join(", "))
                })
                .to_string())
            } else {
                Ok(json!({
                    "status": "error",
                    "message": "not_found",
                    "details": format!("No scheduled job named '{}' found in this chat", name)
                })
                .to_string())
            }
        }
        Err(e) => {
            let err_msg = format!("failed to update scheduled job: {e}");
            warn!(error = %err_msg);
            Ok(json!({
                "status": "error",
                "message": "database_error",
                "details": err_msg
            })
            .to_string())
        }
    }
}

async fn execute_delete<D: Db>(
    db: &D,
    telegram_chat_id: i64,
    message_thread_id: Option<i64>,
    args: &JsonValue,
) -> Result<String> {
    let name = match args.get(NAME_PARAM).and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n,
        _ => {
            return Ok(json!({
                "status": "error",
                "message": "missing_parameter",
                "details": format!("'{}' is required for delete command", NAME_PARAM)
            })
            .to_string());
        }
    };

    match db
        .delete_scheduled_job_by_name(name, telegram_chat_id, message_thread_id)
        .await
    {
        Ok(deleted) => {
            if deleted {
                info!(name = %name, "deleted scheduled job");
                Ok(json!({
                    "status": "success",
                    "message": format!("Deleted scheduled job '{}'", name)
                })
                .to_string())
            } else {
                Ok(json!({
                    "status": "error",
                    "message": "not_found",
                    "details": format!("No scheduled job named '{}' found in this chat", name)
                })
                .to_string())
            }
        }
        Err(e) => {
            let err_msg = format!("failed to delete scheduled job: {e}");
            warn!(error = %err_msg);
            Ok(json!({
                "status": "error",
                "message": "database_error",
                "details": err_msg
            })
            .to_string())
        }
    }
}

async fn execute_set_enabled<D: Db>(
    db: &D,
    telegram_chat_id: i64,
    message_thread_id: Option<i64>,
    args: &JsonValue,
    enabled: bool,
) -> Result<String> {
    let name = match args.get(NAME_PARAM).and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n,
        _ => {
            let action = if enabled { "enable" } else { "disable" };
            return Ok(json!({
                "status": "error",
                "message": "missing_parameter",
                "details": format!("'{}' is required for {} command", NAME_PARAM, action)
            })
            .to_string());
        }
    };

    match db
        .set_scheduled_job_enabled(name, telegram_chat_id, message_thread_id, enabled)
        .await
    {
        Ok(updated) => {
            if updated {
                let action = if enabled { "enabled" } else { "disabled" };
                info!(name = %name, enabled = enabled, "updated scheduled job enabled status");
                Ok(json!({
                    "status": "success",
                    "message": format!("Scheduled job '{}' has been {}", name, action)
                })
                .to_string())
            } else {
                Ok(json!({
                    "status": "error",
                    "message": "not_found",
                    "details": format!("No scheduled job named '{}' found in this chat", name)
                })
                .to_string())
            }
        }
        Err(e) => {
            let err_msg = format!("failed to update scheduled job: {e}");
            warn!(error = %err_msg);
            Ok(json!({
                "status": "error",
                "message": "database_error",
                "details": err_msg
            })
            .to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MockDb;
    use crate::scheduler::types::ScheduledJob;
    use chrono::Utc;
    use mockall::predicate::*;

    fn make_test_job(name: &str, enabled: bool) -> ScheduledJob {
        ScheduledJob {
            id: 1,
            name: name.to_string(),
            cron_schedule: "0 9 * * *".to_string(),
            prompt: "Test prompt".to_string(),
            telegram_chat_id: 123,
            message_thread_id: None,
            enabled,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_list_jobs_success() {
        let mut mock_db = MockDb::new();
        mock_db
            .expect_get_scheduled_jobs_for_chat()
            .with(eq(123i64), eq(None::<i64>))
            .returning(|_, _| Ok(vec![make_test_job("daily_check", true)]));

        let result = execute_scheduled_jobs(&mock_db, Some(123), None, r#"{"command": "list"}"#)
            .await
            .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "success");
        assert_eq!(parsed["count"], 1);
    }

    #[tokio::test]
    async fn test_add_job_success() {
        let mut mock_db = MockDb::new();
        mock_db
            .expect_create_scheduled_job()
            .with(
                eq("test_job"),
                eq("0 9 * * *"),
                eq("Check status"),
                eq(123i64),
                eq(None::<i64>),
            )
            .returning(|_, _, _, _, _| Ok(1));

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "add", "name": "test_job", "cron_schedule": "0 9 * * *", "prompt": "Check status"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "success");
    }

    #[tokio::test]
    async fn test_add_job_invalid_cron() {
        let mock_db = MockDb::new();

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "add", "name": "test_job", "cron_schedule": "invalid", "prompt": "Check status"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "error");
        assert_eq!(parsed["message"], "invalid_cron_schedule");
    }

    #[tokio::test]
    async fn test_add_job_missing_name() {
        let mock_db = MockDb::new();

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "add", "cron_schedule": "0 9 * * *", "prompt": "Check status"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "error");
        assert_eq!(parsed["message"], "missing_parameter");
    }

    #[tokio::test]
    async fn test_delete_job_success() {
        let mut mock_db = MockDb::new();
        mock_db
            .expect_delete_scheduled_job_by_name()
            .with(eq("test_job"), eq(123i64), eq(None::<i64>))
            .returning(|_, _, _| Ok(true));

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "delete", "name": "test_job"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "success");
    }

    #[tokio::test]
    async fn test_delete_job_not_found() {
        let mut mock_db = MockDb::new();
        mock_db
            .expect_delete_scheduled_job_by_name()
            .with(eq("nonexistent"), eq(123i64), eq(None::<i64>))
            .returning(|_, _, _| Ok(false));

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "delete", "name": "nonexistent"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "error");
        assert_eq!(parsed["message"], "not_found");
    }

    #[tokio::test]
    async fn test_enable_job_success() {
        let mut mock_db = MockDb::new();
        mock_db
            .expect_set_scheduled_job_enabled()
            .with(eq("test_job"), eq(123i64), eq(None::<i64>), eq(true))
            .returning(|_, _, _, _| Ok(true));

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "enable", "name": "test_job"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "success");
    }

    #[tokio::test]
    async fn test_disable_job_success() {
        let mut mock_db = MockDb::new();
        mock_db
            .expect_set_scheduled_job_enabled()
            .with(eq("test_job"), eq(123i64), eq(None::<i64>), eq(false))
            .returning(|_, _, _, _| Ok(true));

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "disable", "name": "test_job"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "success");
    }

    #[tokio::test]
    async fn test_no_chat_context() {
        let mock_db = MockDb::new();

        let result = execute_scheduled_jobs(
            &mock_db,
            None, // No chat context
            None,
            r#"{"command": "list"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "error");
    }

    #[tokio::test]
    async fn test_malformed_json() {
        let mock_db = MockDb::new();

        let result = execute_scheduled_jobs(&mock_db, Some(123), None, r#"not valid json"#)
            .await
            .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "error");
        assert_eq!(parsed["message"], "malformed_json");
    }

    #[tokio::test]
    async fn test_unknown_command() {
        let mock_db = MockDb::new();

        let result = execute_scheduled_jobs(&mock_db, Some(123), None, r#"{"command": "unknown"}"#)
            .await
            .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "error");
        assert_eq!(parsed["message"], "unknown_command");
    }

    #[tokio::test]
    async fn test_edit_job_schedule_success() {
        let mut mock_db = MockDb::new();
        mock_db
            .expect_update_scheduled_job()
            .with(
                eq("test_job"),
                eq(123i64),
                eq(None::<i64>),
                eq(Some("0 10 * * *".to_string())),
                eq(None::<String>),
            )
            .returning(|_, _, _, _, _| Ok(true));

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "edit", "name": "test_job", "cron_schedule": "0 10 * * *"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "success");
    }

    #[tokio::test]
    async fn test_edit_job_prompt_success() {
        let mut mock_db = MockDb::new();
        mock_db
            .expect_update_scheduled_job()
            .with(
                eq("test_job"),
                eq(123i64),
                eq(None::<i64>),
                eq(None::<String>),
                eq(Some("New prompt".to_string())),
            )
            .returning(|_, _, _, _, _| Ok(true));

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "edit", "name": "test_job", "prompt": "New prompt"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "success");
    }

    #[tokio::test]
    async fn test_edit_job_both_success() {
        let mut mock_db = MockDb::new();
        mock_db
            .expect_update_scheduled_job()
            .with(
                eq("test_job"),
                eq(123i64),
                eq(None::<i64>),
                eq(Some("0 12 * * *".to_string())),
                eq(Some("Updated prompt".to_string())),
            )
            .returning(|_, _, _, _, _| Ok(true));

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "edit", "name": "test_job", "cron_schedule": "0 12 * * *", "prompt": "Updated prompt"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "success");
    }

    #[tokio::test]
    async fn test_edit_job_not_found() {
        let mut mock_db = MockDb::new();
        mock_db
            .expect_update_scheduled_job()
            .with(
                eq("nonexistent"),
                eq(123i64),
                eq(None::<i64>),
                eq(Some("0 10 * * *".to_string())),
                eq(None::<String>),
            )
            .returning(|_, _, _, _, _| Ok(false));

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "edit", "name": "nonexistent", "cron_schedule": "0 10 * * *"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "error");
        assert_eq!(parsed["message"], "not_found");
    }

    #[tokio::test]
    async fn test_edit_job_missing_name() {
        let mock_db = MockDb::new();

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "edit", "cron_schedule": "0 10 * * *"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "error");
        assert_eq!(parsed["message"], "missing_parameter");
    }

    #[tokio::test]
    async fn test_edit_job_missing_changes() {
        let mock_db = MockDb::new();

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "edit", "name": "test_job"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "error");
        assert_eq!(parsed["message"], "missing_parameter");
    }

    #[tokio::test]
    async fn test_edit_job_invalid_cron() {
        let mock_db = MockDb::new();

        let result = execute_scheduled_jobs(
            &mock_db,
            Some(123),
            None,
            r#"{"command": "edit", "name": "test_job", "cron_schedule": "invalid"}"#,
        )
        .await
        .unwrap();

        let parsed: JsonValue = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], "error");
        assert_eq!(parsed["message"], "invalid_cron_schedule");
    }
}
