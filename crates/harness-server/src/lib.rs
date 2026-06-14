#![allow(
    clippy::field_reassign_with_default,
    clippy::items_after_test_module,
    clippy::manual_is_multiple_of,
    clippy::manual_pattern_char_comparison,
    clippy::new_without_default,
    clippy::too_many_arguments,
    clippy::unnecessary_cast,
    clippy::unnecessary_to_owned
)]

// Modules extracted to `harness-workflow`; re-exported so existing `crate::*`
// paths inside this crate continue to resolve without modification.
pub use harness_workflow::checkpoint;
pub use harness_workflow::circuit_breaker;
pub mod assets;
pub(crate) mod billing_calibration;
pub mod complexity_router;
pub mod contract_validator;
pub mod dashboard;
pub mod db;
pub(crate) mod github_auth;
pub mod handlers;
pub mod hook_enforcer;
pub mod http;
/// Credential detection for the authoring catalog (shared with `harness-cli`).
pub use http::credentials_routes::connected_clis;
pub mod memory_monitor;
pub mod notify;
pub mod overview;
pub mod post_validator;
pub mod project_registry;
pub mod q_value_store;
pub mod redact;
pub mod review_store;
pub mod rule_enforcer;
pub mod server;
pub use harness_workflow::task_queue;
pub mod trusted_proxy;

#[doc(hidden)]
pub mod test_helpers;
