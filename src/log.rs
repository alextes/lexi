use tracing_subscriber::{fmt::format::FmtSpan, EnvFilter};

use crate::env;

pub fn init() {
    // we avoid reading the lazy initialized ENV_CONFIG here as it depends on log being initialized
    let log_json = env::get_env_bool("LOG_JSON").unwrap_or_default();
    let log_perf = env::get_env_bool("LOG_PERF").unwrap_or_default();

    let builder = tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env());

    let builder = if log_perf {
        builder.with_span_events(FmtSpan::CLOSE)
    } else {
        builder
    };

    if log_json {
        builder.json().init();
    } else {
        builder.init();
    }
}
