/// Server start time. Initialized once in `serve()` before accepting connections,
/// so uptime reflects true server uptime rather than time since first request.
pub(crate) static SERVER_START: std::sync::OnceLock<std::time::Instant> =
    std::sync::OnceLock::new();
