use serde_json::{json, Map as JsonMap, Value as JsonValue};
use sqlx::{types::BigDecimal, Column, PgPool, Row, ValueRef};
use tracing::{error, info};

// this function will be called by both mevdb_query and globaldb_query
pub async fn execute_db_query_common(
    pool: &PgPool,
    query: &str,
    tool_name: &str, // for logging purposes, e.g., "mevdb" or "globaldb"
) -> JsonValue {
    info!(query = %query, tool = tool_name, "(db_utils) attempting to execute db query using provided pool");

    match sqlx::query(query).fetch_all(pool).await {
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
                            // value is not null, proceed with typed parsing
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
                                // using BigDecimal here
                                json!(v_dec.to_string()) // bigdecimal to_string typically normalizes
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
                                    "(db_utils) data conversion error"
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
                            // error from try_get_raw itself
                            let err_msg = format!(
                                "failed to retrieve raw value for column '{}' (type: {:?}, reported type name: {}) in row {}: {}. this might indicate a problem with the query or db connection.",
                                column_name,
                                column_type_info,
                                column_type_info.to_string(),
                                row_idx,
                                e
                            );
                            error!(tool = tool_name, column_name, column_type_name = %column_type_info, row_idx, error = %e, err_msg, "(db_utils) raw data retrieval error");
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
            error!(error = %e, query = %query, tool = tool_name, "(db_utils) failed to execute sql query");
            json!({
                "status": "error",
                "message": format!("failed to execute {} sql query.", tool_name),
                "details": e.to_string()
            })
        }
    }
}

// an example of how to add tests later
#[cfg(test)]
mod tests {
    // Let's be explicit with imports for clarity within the test module
    use crate::message_processor::tools::db_utils::execute_db_query_common;
    use sqlx::{types::BigDecimal, PgPool};
    use std::str::FromStr;
    use eyre::Result; // For the test function's return type
    // serde_json::json is not directly used here anymore, execute_db_query_common returns it.

    #[sqlx::test] 
    async fn test_numeric_type_handling(pool: PgPool) -> Result<()> { 
        // Reverted to plain CREATE TABLE, relying on sqlx::test for isolation/cleanup
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

    // add more tests here: test_all_types, test_no_rows, test_error_handling etc.
}
