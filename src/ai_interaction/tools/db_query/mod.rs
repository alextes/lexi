// this is the new src/ai_interaction/tools/db_query/mod.rs

// declare the submodules first, standard practice
mod globaldb_query;
mod mevdb_query;

pub use globaldb_query::execute_globaldb_query_tool;
pub use globaldb_query::GLOBALDB_QUERY_TOOL;
pub use globaldb_query::GLOBALDB_TOOL_NAME;
pub use mevdb_query::execute_mevdb_query_tool;
pub use mevdb_query::MEVDB_QUERY_TOOL;
pub use mevdb_query::MEVDB_TOOL_NAME;

// original content from db_utils.rs starts here
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use sqlx::postgres::PgTypeKind;
use sqlx::TypeInfo;
use sqlx::{
    postgres::{PgRow, PgTypeInfo, PgValueRef},
    types::BigDecimal,
    Column, Executor, Row, ValueRef,
};
use tracing::instrument;
use tracing::{error, info};

// helper function for parsing enum values
fn try_parse_enum_value_as_json(
    raw_pg_value_ref: &PgValueRef, // from row.try_get_raw(col_idx)
    column_name: &str,
    column_type_info_name: &str, // from column.type_info().name()
    row_idx: usize,
) -> Result<JsonValue, JsonValue> {
    // ok is the parsed json value, err is the json error to return early
    match raw_pg_value_ref.as_str() {
        Ok(s_ref) => Ok(json!(s_ref)), // s_ref is &str
        Err(e) => {
            let err_msg = format!(
                "failed to convert enum column '{}' (type: {}) to string using as_str(): {}. row: {}. application error.",
                column_name, column_type_info_name, e, row_idx
            );
            error!(
                column_name,
                column_type_name = %column_type_info_name,
                row_idx,
                error = %e,
                err_msg,
                "enum as_str() conversion error"
            );
            Err(json!({
                "status": "error",
                "message": "failed to process database results due to an enum conversion issue (as_str failed).",
                "details": err_msg
            }))
        }
    }
}

// New helper function for converting a non-null SQL value to JsonValue
fn try_convert_non_null_sql_value_to_json<'r>(
    raw_pg_value_ref: &PgValueRef<'r>, // From row.try_get_raw(col_idx), used for ENUMs
    row: &'r PgRow,                    // The current row, for other row.try_get calls
    col_idx: usize,                    // Index for row.try_get
    column_name: &str,
    column_type_info: &PgTypeInfo, // Full type info for kind() and name()
    row_idx: usize,                // For logging
) -> Result<JsonValue, JsonValue> {
    // Ok is the parsed json value, Err is the json error to return early
    if matches!(column_type_info.kind(), PgTypeKind::Enum(_)) {
        // Call the existing helper for ENUMs, passing the necessary parts
        try_parse_enum_value_as_json(
            raw_pg_value_ref,
            column_name,
            column_type_info.name(),
            row_idx,
        )
    } else if let Ok(v_str) = row.try_get::<String, _>(col_idx) {
        Ok(json!(v_str))
    } else if let Ok(v_i64) = row.try_get::<i64, _>(col_idx) {
        Ok(json!(v_i64))
    } else if let Ok(v_i32) = row.try_get::<i32, _>(col_idx) {
        Ok(json!(v_i32))
    } else if let Ok(v_f64) = row.try_get::<f64, _>(col_idx) {
        Ok(json!(v_f64))
    } else if let Ok(v_dec) = row.try_get::<BigDecimal, _>(col_idx) {
        Ok(json!(v_dec.to_string()))
    } else if let Ok(v_bool) = row.try_get::<bool, _>(col_idx) {
        Ok(json!(v_bool))
    } else if let Ok(v_time) = row.try_get::<chrono::DateTime<chrono::Utc>, _>(col_idx) {
        Ok(json!(v_time.to_rfc3339()))
    } else {
        // This case is for non-null values that didn't match any of the above types
        let err_msg = format!(
            "unhandled or failed conversion for non-null sql type for column '{}' (type: {}) in row {}. this is an application error.",
            column_name,
            column_type_info.name(), // Use .name() for a cleaner type representation in the error
            row_idx
        );
        error!(
            column_name,
            column_type_name = %column_type_info.name(),
            row_idx,
            err_msg,
            "data conversion error"
        );
        Err(json!({
            "status": "error",
            "message": "failed to process database results due to an unsupported or unparseable data type.",
            "details": err_msg
        }))
    }
}

// this function will be called by both mevdb_query and globaldb_query
// it's now a public function within the db module.
#[instrument(skip(executor, query), fields(tool_name = %tool_name, query = %query))]
pub async fn execute_db_query_common<'c, E>(
    executor: E,
    query: &str,
    tool_name: &str, // for logging purposes, e.g., "mevdb" or "globaldb"
) -> JsonValue
where
    E: Executor<'c, Database = sqlx::Postgres>,
{
    info!("attempting to execute db query using provided executor");

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
                        Ok(raw_pg_value_ref) => {
                            if raw_pg_value_ref.is_null() {
                                json_row.insert(column_name.to_string(), json!(null));
                            } else {
                                // raw_pg_value_ref is not null here.
                                // Pass row as is (it's &'r PgRow from the outer loop iterator)
                                // Pass column_type_info as is (it's &'PgTypeInfo from column.type_info())
                                match try_convert_non_null_sql_value_to_json(
                                    &raw_pg_value_ref,
                                    row,
                                    col_idx,
                                    column_name,
                                    column_type_info,
                                    row_idx,
                                ) {
                                    Ok(parsed_json_value) => {
                                        json_row.insert(column_name.to_string(), parsed_json_value);
                                    }
                                    Err(error_json_to_return) => {
                                        return error_json_to_return;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            let err_msg = format!(
                                "failed to retrieve raw value for column '{}' (type: {:?}) in row {}: {}. this might indicate a problem with the query or db connection.",
                                column_name,
                                column_type_info,
                                row_idx,
                                e
                            );
                            error!(column_name, column_type_name = %column_type_info, row_idx, error = %e, err_msg, "raw data retrieval error");
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
            error!(error = %e, "failed to execute sql query");
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
    use super::execute_db_query_common;
    use anyhow::Result;
    use chrono::{DateTime, Utc};
    use serde_json::json;
    use serde_json::Value as JsonValue;
    use sqlx::{types::BigDecimal, PgPool};
    use std::str::FromStr;

    #[sqlx::test]
    async fn test_execute_db_query_common_all_types(pool: PgPool) -> Result<()> {
        let create_table_query = "
            CREATE TABLE test_all_types (
                id SERIAL PRIMARY KEY,
                text_val TEXT,
                bigint_val BIGINT,
                int_val INTEGER,
                double_val DOUBLE PRECISION,
                numeric_val NUMERIC(30, 15),
                bool_val BOOLEAN,
                timestamp_val TIMESTAMP WITH TIME ZONE
            );";

        sqlx::query(create_table_query).execute(&pool).await?;

        let insert_data_query = "
            INSERT INTO test_all_types 
                (text_val, bigint_val, int_val, double_val, numeric_val, bool_val, timestamp_val) 
            VALUES 
                ('hello world', 123456789012345, 12345, 123.456, 12345.6789, TRUE, '2023-01-01T12:00:00Z'),
                (NULL, NULL, NULL, NULL, NULL, NULL, NULL),
                ('another string', 98765432109876, 67890, 789.0123456789, 0.123456789012345, FALSE, '2024-02-02T10:30:00.500Z');
        ";
        sqlx::query(insert_data_query).execute(&pool).await?;

        let select_data_query = "SELECT id, text_val, bigint_val, int_val, double_val, numeric_val, bool_val, timestamp_val FROM test_all_types ORDER BY id;";

        let result_json_value =
            execute_db_query_common(&pool, select_data_query, "test_all_types").await;

        type ExpectedRow = (
            Option<String>,
            Option<i64>,
            Option<i32>,
            Option<f64>,
            Option<BigDecimal>,
            Option<bool>,
            Option<DateTime<Utc>>,
        );

        let expected_rows_data: Vec<ExpectedRow> = vec![
            (
                Some("hello world".to_string()),
                Some(123456789012345i64),
                Some(12345i32),
                Some(123.456f64),
                Some(BigDecimal::from_str("12345.6789").unwrap()),
                Some(true),
                Some(
                    DateTime::parse_from_rfc3339("2023-01-01T12:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                ),
            ),
            (None, None, None, None, None, None, None),
            (
                Some("another string".to_string()),
                Some(98765432109876i64),
                Some(67890i32),
                Some(789.0123456789f64),
                Some(BigDecimal::from_str("0.123456789012345").unwrap()),
                Some(false),
                Some(
                    DateTime::parse_from_rfc3339("2024-02-02T10:30:00.500Z")
                        .unwrap()
                        .with_timezone(&Utc),
                ),
            ),
        ];

        let arr = result_json_value.as_array().ok_or_else(|| {
            anyhow::anyhow!("result was not a json array. got: {:?}", result_json_value)
        })?;
        anyhow::ensure!(
            arr.len() == expected_rows_data.len(),
            "array length mismatch. expected {}, got: {}. array: {:?}",
            expected_rows_data.len(),
            arr.len(),
            arr
        );

        let column_names = [
            "text_val",
            "bigint_val",
            "int_val",
            "double_val",
            "numeric_val",
            "bool_val",
            "timestamp_val",
        ];

        for (row_idx, expected_row_tuple) in expected_rows_data.iter().enumerate() {
            let json_obj = arr[row_idx].as_object().ok_or_else(|| {
                anyhow::anyhow!(
                    "element {} is not a json object. got: {:?}. full array: {:?}",
                    row_idx,
                    arr[row_idx],
                    arr
                )
            })?;

            let expected_values_as_options: Vec<Option<JsonValue>> = vec![
                expected_row_tuple
                    .0
                    .as_ref()
                    .map(|x| JsonValue::String(x.clone())),
                expected_row_tuple.1.as_ref().map(|x| JsonValue::from(*x)),
                expected_row_tuple.2.as_ref().map(|x| JsonValue::from(*x)),
                expected_row_tuple.3.as_ref().map(|x| JsonValue::from(*x)),
                expected_row_tuple
                    .4
                    .as_ref()
                    .map(|x| JsonValue::String(x.to_string())),
                expected_row_tuple.5.as_ref().map(|x| JsonValue::from(*x)),
                expected_row_tuple
                    .6
                    .as_ref()
                    .map(|x| JsonValue::String(x.to_rfc3339())),
            ];

            for (col_idx, col_name_str) in column_names.iter().enumerate() {
                let col_name = *col_name_str;
                let json_val_for_col = json_obj.get(col_name);
                let expected_json_val_opt = &expected_values_as_options[col_idx];

                match (json_val_for_col, expected_json_val_opt) {
                    (Some(actual_val), Some(expected_val)) => {
                        if actual_val.is_null() {
                            anyhow::bail!(
                                "row {}, col '{}': actual is null but expected non-null value {:?}. object: {:?}",
                                row_idx, col_name, expected_val, json_obj
                            );
                        }

                        if col_name == "numeric_val" {
                            let actual_str = actual_val.as_str().ok_or_else(|| {
                                anyhow::anyhow!(
                                    "row {}, col '{}': numeric actual value is not a string: {:?}",
                                    row_idx,
                                    col_name,
                                    actual_val
                                )
                            })?;
                            let expected_str = expected_val.as_str().ok_or_else(||
                                anyhow::anyhow!("row {}, col '{}': numeric expected value is not a string: {:?}", row_idx, col_name, expected_val))?;
                            let actual_bd = BigDecimal::from_str(actual_str)?;
                            let expected_bd = BigDecimal::from_str(expected_str)?;
                            anyhow::ensure!(actual_bd == expected_bd,
                                "row {}, col '{}': numeric mismatch. actual: {}, expected: {}. object: {:?}",
                                row_idx, col_name, actual_bd, expected_bd, json_obj);
                        } else if col_name == "timestamp_val" {
                            let actual_str = actual_val.as_str().ok_or_else(||
                                anyhow::anyhow!("row {}, col '{}': timestamp actual value is not a string: {:?}", row_idx, col_name, actual_val))?;
                            let expected_str = expected_val.as_str().ok_or_else(||
                                anyhow::anyhow!("row {}, col '{}': timestamp expected value is not a string: {:?}", row_idx, col_name, expected_val))?;
                            let actual_dt =
                                DateTime::parse_from_rfc3339(actual_str)?.with_timezone(&Utc);
                            let expected_dt =
                                DateTime::parse_from_rfc3339(expected_str)?.with_timezone(&Utc);
                            anyhow::ensure!(actual_dt == expected_dt,
                                "row {}, col '{}': timestamp mismatch. actual: {}, expected: {}. object: {:?}",
                                row_idx, col_name, actual_dt, expected_dt, json_obj);
                        } else {
                            anyhow::ensure!(actual_val == expected_val,
                                "row {}, col '{}': value mismatch. actual: {:?}, expected: {:?}. object: {:?}",
                                row_idx, col_name, actual_val, expected_val, json_obj);
                        }
                    }
                    (Some(actual_val), None) => {
                        anyhow::ensure!(actual_val.is_null(),
                            "row {}, col '{}': actual value {:?} was not null, but expected null. object: {:?}",
                            row_idx, col_name, actual_val, json_obj);
                    }
                    (None, Some(expected_val)) => {
                        anyhow::bail!(
                            "row {}, col '{}': key missing but expected value {:?}. object: {:?}",
                            row_idx,
                            col_name,
                            expected_val,
                            json_obj
                        );
                    }
                    (None, None) => {
                        anyhow::bail!(
                            "row {}, col '{}': key missing and expected null. this should be json!(null) instead of missing. object: {:?}",
                            row_idx, col_name, json_obj
                        );
                    }
                }
            }
        }

        sqlx::query("DROP TABLE test_all_types;")
            .execute(&pool)
            .await?;
        Ok(())
    }

    #[sqlx::test]
    async fn test_execute_db_query_common_with_enum(pool: PgPool) -> Result<()> {
        // 1. create custom enum type
        sqlx::query("CREATE TYPE mood AS ENUM ('sad', 'ok', 'happy');")
            .execute(&pool)
            .await?;

        // 2. create table with enum column
        let create_table_query = "
            CREATE TABLE test_enum_table (
                id SERIAL PRIMARY KEY,
                current_mood mood,
                description TEXT
            );";
        sqlx::query(create_table_query).execute(&pool).await?;

        // 3. insert data
        sqlx::query(
            "INSERT INTO test_enum_table (current_mood, description) VALUES
                ('happy', 'feeling great'),
                ('sad', 'not so good'),
                (NULL, 'unknown mood'),
                ('ok', NULL);",
        )
        .execute(&pool)
        .await?;

        // 4. select data
        let select_data_query =
            "SELECT id, current_mood, description FROM test_enum_table ORDER BY id;";
        let result_json_value =
            execute_db_query_common(&pool, select_data_query, "test_enum").await;

        // 5. verify results
        let arr = result_json_value.as_array().ok_or_else(|| {
            anyhow::anyhow!("result was not a json array. got: {:?}", result_json_value)
        })?;
        assert_eq!(
            arr.len(),
            4,
            "expected 4 rows, got {}. result: \n{:#?}",
            arr.len(),
            result_json_value
        );

        // row 0: happy
        let row0 = arr[0].as_object().unwrap();
        assert_eq!(
            row0.get("current_mood").unwrap(),
            &json!("happy"),
            "row 0 mood mismatch. got: {:?}",
            row0.get("current_mood")
        );

        // row 1: sad
        let row1 = arr[1].as_object().unwrap();
        assert_eq!(
            row1.get("current_mood").unwrap(),
            &json!("sad"),
            "row 1 mood mismatch. got: {:?}",
            row1.get("current_mood")
        );

        // row 2: NULL
        let row2 = arr[2].as_object().unwrap();
        assert!(
            row2.get("current_mood").unwrap().is_null(),
            "row 2 mood expected null. got: {:?}",
            row2.get("current_mood")
        );

        // row 3: ok
        let row3 = arr[3].as_object().unwrap();
        assert_eq!(
            row3.get("current_mood").unwrap(),
            &json!("ok"),
            "row 3 mood mismatch. got: {:?}",
            row3.get("current_mood")
        );

        // 6. cleanup - not needed, sqlx::test handles rollback
        Ok(())
    }
}
