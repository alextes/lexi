//! Fns to read variables from the environment more conveniently and help other functions figure
//! out what environment they're running in.

use std::{env, sync::LazyLock};

use tracing::{debug, warn};

const LOG_BLACKLIST: [&str; 1] = ["DATABASE_URL"];

pub static ENV_CONFIG: LazyLock<EnvConfig> = LazyLock::new(get_env_config);

fn obfuscate_if_secret(blacklist: &[&str], key: &str, value: &str) -> String {
    if blacklist.contains(&key) {
        let mut last_four = value.to_string();
        last_four.drain(0..value.len().saturating_sub(4));
        format!("****{last_four}")
    } else {
        value.to_string()
    }
}

/// Get an environment variable, encoding found or missing as Option, and panic otherwise.
pub fn get_env_var(key: &str) -> Option<String> {
    let var = match env::var(key) {
        Err(env::VarError::NotPresent) => None,
        Err(e) => panic!("{e}"),
        Ok(var) => Some(var),
    };

    if let Some(ref existing_var) = var {
        let output = obfuscate_if_secret(&LOG_BLACKLIST, key, existing_var);
        debug!("env var {key}: {output}");
    } else {
        debug!("env var {key} requested but not found")
    };

    var
}

pub fn get_env_bool(key: &str) -> Option<bool> {
    get_env_var(key).and_then(|var| match var.to_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        "t" => Some(true),
        "f" => Some(false),
        "1" => Some(true),
        "0" => Some(false),
        str => {
            warn!("invalid bool value {str} for {key}");
            None
        }
    })
}

pub struct EnvConfig {
    /// url of the beacon node
    pub beacon_url: Option<String>,
    /// the admin code for the bot, used in conversation admin tool
    pub bot_admin_code: Option<String>,
    /// the url of the database to run against
    /// url of the database to run against
    pub database_url: Option<String>,
    /// url of the globaldb database (optional)
    pub globaldb_database_url: Option<String>,
    /// output logs in json format
    pub log_json: bool,
    /// add how long each span took to the logs
    pub log_perf: bool,
    /// url of the mevdb database (optional)
    pub mevdb_database_url: Option<String>,
}

pub fn get_env_config() -> EnvConfig {
    EnvConfig {
        beacon_url: get_env_var("BEACON_URL"),
        bot_admin_code: get_env_var("BOT_ADMIN_CODE"),
        database_url: get_env_var("DATABASE_URL"),
        globaldb_database_url: get_env_var("GLOBALDB_DATABASE_URL"),
        log_json: get_env_bool("LOG_JSON").unwrap_or(false),
        log_perf: get_env_bool("LOG_PERF").unwrap_or(false),
        mevdb_database_url: get_env_var("MEVDB_DATABASE_URL"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_env_var_safe_some() {
        let test_key = "TEST_KEY_SAFE_SOME";
        let test_value = "my-env-value";
        std::env::set_var(test_key, test_value);
        assert_eq!(get_env_var(test_key), Some(test_value.to_string()));
    }

    #[test]
    fn test_get_env_var_safe_none() {
        let key = get_env_var("DOESNT_EXIST");
        assert!(key.is_none());
    }

    #[test]
    fn test_get_env_bool_not_there() {
        let flag = get_env_bool("DOESNT_EXIST");
        assert_eq!(flag, None);
    }

    #[test]
    fn test_get_env_bool_true() {
        let test_key = "TEST_KEY_BOOL_TRUE";
        let test_value = "true";
        std::env::set_var(test_key, test_value);
        assert_eq!(get_env_bool(test_key), Some(true));
    }

    #[test]
    fn test_get_env_bool_true_upper() {
        let test_key = "TEST_KEY_BOOL_TRUE2";
        let test_value = "TRUE";
        std::env::set_var(test_key, test_value);
        assert_eq!(get_env_bool(test_key), Some(true));
    }

    #[test]
    fn test_get_env_bool_false() {
        let test_key = "TEST_KEY_BOOL_FALSE";
        let test_value = "false";
        std::env::set_var(test_key, test_value);
        assert_eq!(get_env_bool(test_key), Some(false));
    }

    #[test]
    fn test_get_env_bool_t() {
        let test_key = "TEST_KEY_BOOL_T";
        std::env::set_var(test_key, "t");
        assert_eq!(get_env_bool(test_key), Some(true));
    }

    #[test]
    fn test_get_env_bool_f() {
        let test_key = "TEST_KEY_BOOL_F";
        std::env::set_var(test_key, "f");
        assert_eq!(get_env_bool(test_key), Some(false));
    }

    #[test]
    fn test_get_env_bool_one() {
        let test_key = "TEST_KEY_BOOL_ONE";
        std::env::set_var(test_key, "1");
        assert_eq!(get_env_bool(test_key), Some(true));
    }

    #[test]
    fn test_get_env_bool_zero() {
        let test_key = "TEST_KEY_BOOL_ZERO";
        std::env::set_var(test_key, "0");
        assert_eq!(get_env_bool(test_key), Some(false));
    }

    #[test]
    fn test_get_env_bool_invalid() {
        let test_key = "TEST_KEY_BOOL_INVALID";
        std::env::set_var(test_key, "not_a_bool");
        assert_eq!(get_env_bool(test_key), None);
    }

    #[test]
    fn test_obfuscate_if_secret() {
        let secret_key = "SECRET_KEY";
        let blacklist = vec![secret_key];
        assert_eq!(
            obfuscate_if_secret(&blacklist, secret_key, "my_secret_value"),
            "****alue"
        );

        let normal_key = "NORMAL_KEY";
        assert_eq!(
            obfuscate_if_secret(&blacklist, normal_key, "my_normal_value"),
            "my_normal_value"
        );
    }
}
