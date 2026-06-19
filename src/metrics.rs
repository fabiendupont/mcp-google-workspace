use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry, TextEncoder,
};
use std::sync::LazyLock;

static REGISTRY: LazyLock<Registry> = LazyLock::new(Registry::new);

static REQUEST_COUNT: LazyLock<IntCounterVec> = LazyLock::new(|| {
    let opts = Opts::new("mcp_requests_total", "Total MCP requests").namespace("mcp_gws");
    let counter = IntCounterVec::new(opts, &["method", "status"]).unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

static REQUEST_DURATION: LazyLock<HistogramVec> = LazyLock::new(|| {
    let opts = HistogramOpts::new("mcp_request_duration_seconds", "MCP request latency")
        .namespace("mcp_gws")
        .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 30.0]);
    let hist = HistogramVec::new(opts, &["method"]).unwrap();
    REGISTRY.register(Box::new(hist.clone())).unwrap();
    hist
});

static ACTIVE_TASKS: LazyLock<IntGauge> = LazyLock::new(|| {
    let gauge = IntGauge::new(
        "mcp_gws_active_tasks",
        "Currently active upload/download tasks",
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

static ERRORS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    let opts = Opts::new("mcp_errors_total", "Total MCP errors").namespace("mcp_gws");
    let counter = IntCounterVec::new(opts, &["method", "error_type"]).unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

pub(crate) fn record_request(method: &str, is_error: bool, duration_secs: f64) {
    let status = if is_error { "error" } else { "ok" };
    REQUEST_COUNT.with_label_values(&[method, status]).inc();
    REQUEST_DURATION
        .with_label_values(&[method])
        .observe(duration_secs);
    if is_error {
        ERRORS_TOTAL.with_label_values(&[method, "handler"]).inc();
    }
}

pub(crate) fn set_active_tasks(count: i64) {
    ACTIVE_TASKS.set(count);
}

pub(crate) fn encode() -> String {
    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_request_increments_counter() {
        record_request("ping", false, 0.001);
        let text = encode();
        assert!(text.contains("mcp_gws_mcp_requests_total"));
    }

    #[test]
    fn test_record_error() {
        record_request("tools/call", true, 0.5);
        let text = encode();
        assert!(text.contains("mcp_gws_mcp_errors_total"));
    }

    #[test]
    fn test_active_tasks_gauge() {
        set_active_tasks(3);
        let text = encode();
        assert!(text.contains("mcp_gws_active_tasks"));
    }

    #[test]
    fn test_encode_produces_prometheus_text() {
        record_request("test", false, 0.01);
        let text = encode();
        assert!(text.contains("# HELP"));
        assert!(text.contains("# TYPE"));
    }
}
