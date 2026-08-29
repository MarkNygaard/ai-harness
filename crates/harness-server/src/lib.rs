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
pub mod admin;
pub mod assets;
pub(crate) mod billing_calibration;
pub mod dashboard;
pub mod db;
pub mod handlers;
pub mod http;
pub mod linear_cli;
/// Credential detection for the authoring catalog (shared with `harness-cli`).
pub use http::credentials_routes::connected_clis;
pub mod notify;
pub mod overview;
pub mod project_registry;
pub mod server;
pub use harness_workflow::task_queue;
pub mod trusted_proxy;

#[doc(hidden)]
pub mod test_helpers;
