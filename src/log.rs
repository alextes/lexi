use tracing_subscriber::{fmt::format::FmtSpan, EnvFilter};

pub fn init(log_json: bool, log_perf: bool) {
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
    };
}
