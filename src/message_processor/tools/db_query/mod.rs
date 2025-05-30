// this is the new src/message_processor/tools/db/mod.rs

// declare the submodules first, standard practice
mod globaldb_query;
mod mevdb_query;

pub use globaldb_query::GLOBALDB_TOOL_NAME;
pub use mevdb_query::MEVDB_TOOL_NAME;
pub use globaldb_query::execute_globaldb_query_tool;
pub use mevdb_query::execute_mevdb_query_tool;
pub use globaldb_query::GLOBALDB_QUERY_TOOL;
pub use mevdb_query::MEVDB_QUERY_TOOL;

// original content from db_utils.rs starts here
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use sqlx::{types::BigDecimal, Column, Row, ValueRef, Executor};
use tracing::{error, info};

// this function will be called by both mevdb_query and globaldb_query
// it's now a public function within the db module.
pub async fn execute_db_query_common<'c, E>(
    executor: E,
    query: &str,
    tool_name: &str, // for logging purposes, e.g., "mevdb" or "globaldb"
) -> JsonValue
where
    E: Executor<'c, Database = sqlx::Postgres>,
{
    info!(query = %query, tool = tool_name, "(db module) attempting to execute db query using provided executor"); // updated log origin

    match sqlx::query(query).fetch_all(executor).await {
        Ok(rows) => {
            if rows.is_empty() {
                return json!({
                    "message": format!("{} query executed successfully. no rows returned.", tool_name)
                });
            }
            let mut results: Vec<JsonMap<String, JsonValue>> = Vec::new();
            for (row_idx, row) in rows.iter().enumerate() {
                let mut json_row = JsonMap::new();
                for (col_idx, column) in row.columns().iter().enumerate() {
                    let column_name = column.name();
                    let column_type_info = column.type_info();

                    match row.try_get_raw(col_idx) {
                        Ok(raw_value) if raw_value.is_null() => {
                            json_row.insert(column_name.to_string(), json!(null));
                        }
                        Ok(_non_null_raw_value) => {
                            let value: JsonValue = if let Ok(v_str) =
                                row.try_get::<String, _>(col_idx)
                            {
                                json!(v_str)
                            } else if let Ok(v_i64) = row.try_get::<i64, _>(col_idx) {
                                json!(v_i64)
                            } else if let Ok(v_i32) = row.try_get::<i32, _>(col_idx) {
                                json!(v_i32)
                            } else if let Ok(v_f64) = row.try_get::<f64, _>(col_idx) {
                                json!(v_f64)
                            } else if let Ok(v_dec) = row.try_get::<BigDecimal, _>(col_idx) {
                                json!(v_dec.to_string())
                            } else if let Ok(v_bool) = row.try_get::<bool, _>(col_idx) {
                                json!(v_bool)
                            } else if let Ok(v_time) =
                                row.try_get::<chrono::DateTime<chrono::Utc>, _>(col_idx)
                            {
                                json!(v_time.to_rfc3339())
                            } else {
                                let err_msg = format!(
                                        "unhandled or failed conversion for non-null sql type for column '{}' (type: {:?}, reported type name: {}) in row {}. this is an application error.",
                                        column_name,
                                        column_type_info,
                                        column_type_info.to_string(),
                                        row_idx
                                    );
                                error!(
                                    tool = tool_name,
                                    column_name,
                                    column_type_name = %column_type_info,
                                    row_idx,
                                    err_msg,
                                    "(db module) data conversion error" // updated log origin
                                );
                                return json!({
                                    "status": "error",
                                    "message": "failed to process database results due to an unsupported or unparseable data type.",
                                    "details": err_msg
                                });
                            };
                            json_row.insert(column_name.to_string(), value);
                        }
                        Err(e) => {
                            let err_msg = format!(
                                "failed to retrieve raw value for column '{}' (type: {:?}, reported type name: {}) in row {}: {}. this might indicate a problem with the query or db connection.",
                                column_name,
                                column_type_info,
                                column_type_info.to_string(),
                                row_idx,
                                e
                            );
                            error!(tool = tool_name, column_name, column_type_name = %column_type_info, row_idx, error = %e, err_msg, "(db module) raw data retrieval error"); // updated log origin
                            return json!({
                                "status": "error",
                                "message": "failed to retrieve data from database for a column.",
                                "details": err_msg
                            });
                        }
                    }
                }
                results.push(json_row);
            }
            json!(results)
        }
        Err(e) => {
            error!(error = %e, query = %query, tool = tool_name, "(db module) failed to execute sql query"); // updated log origin
            json!({
                "status": "error",
                "message": format!("failed to execute {} sql query.", tool_name),
                "details": e.to_string()
            })
        }
    }
}

#[cfg(test)]
mod tests {
    // use crate::message_processor::tools::db_utils::execute_db_query_common; // old path
    use super::execute_db_query_common; // new path as it's in the parent module (db/mod.rs)
    use sqlx::{types::BigDecimal, PgPool};
    use std::str::FromStr;
    use eyre::Result; 

    #[sqlx::test] 
    async fn test_numeric_type_handling(pool: PgPool) -> Result<()> { 
        let create_table_query = "CREATE TABLE test_numeric_types (id SERIAL PRIMARY KEY, numeric_val NUMERIC(20, 10));";
        let insert_data_query = "INSERT INTO test_numeric_types (numeric_val) VALUES (12345.6789), (NULL), (0.123), (9876543210.123456789);";
        let select_data_query = "SELECT numeric_val FROM test_numeric_types ORDER BY id;";
        
        sqlx::query(create_table_query).execute(&pool).await?;
        sqlx::query(insert_data_query).execute(&pool).await?;
        
        let result_json_value = execute_db_query_common(&pool, select_data_query, "test_numeric").await;

        let expected_values: Vec<Option<BigDecimal>> = vec![
            Some(BigDecimal::from_str("12345.6789").unwrap()),
            None,
            Some(BigDecimal::from_str("0.123").unwrap()),
            Some(BigDecimal::from_str("9876543210.123456789").unwrap()),
        ];
        
        let arr = result_json_value.as_array()
            .ok_or_else(|| eyre::eyre!("result was not a json array. got: {:?}", result_json_value))?;
        eyre::ensure!(arr.len() == expected_values.len(), 
            "array length mismatch. expected {}, got: {}. array: {:?}", 
            expected_values.len(), arr.len(), arr);

        for i in 0..arr.len() {
            let json_obj = arr[i].as_object()
                .ok_or_else(|| eyre::eyre!("element {} is not a json object. got: {:?}", i, arr[i]))?;
            let json_val_for_col = json_obj.get("numeric_val")
                .ok_or_else(|| eyre::eyre!("element {} does not have 'numeric_val' key. got: {:?}", i, json_obj))?;

            match &expected_values[i] {
                Some(expected_bd) => {
                    eyre::ensure!(!json_val_for_col.is_null(), 
                        "element {}['numeric_val'] should not be null, expected {}", i, expected_bd);
                    let actual_str = json_val_for_col.as_str()
                        .ok_or_else(|| eyre::eyre!(
                            "element {}['numeric_val'] is not a string. got: {:?}", i, json_val_for_col
                        ))?;
                    let actual_bd = BigDecimal::from_str(actual_str)?;
                    eyre::ensure!(actual_bd == *expected_bd, 
                        "value mismatch for element {}. expected bd value: {}, got bd value: {}. (from string: {})", 
                        i, expected_bd, actual_bd, actual_str);
                }
                None => {
                    eyre::ensure!(json_val_for_col.is_null(), 
                        "element {}['numeric_val'] should be null. got: {:?}", i, json_val_for_col);
                }
            }
        }
        
        Ok(())
    }
}
