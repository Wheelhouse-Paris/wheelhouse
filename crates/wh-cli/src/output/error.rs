//! CLI error hierarchy and exit code mapping.
//!
//! Exit codes (ADR-014):
//! - 0: success
//! - 1: error
//! - 2: plan change detected

use wh_broker::deploy::DeployError;

/// CLI exit codes per ADR-014.
pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_PLAN_CHANGE: i32 = 2;

/// Map a DeployError to its error code string.
pub fn error_code(err: &DeployError) -> &'static str {
    err.code()
}

/// Map a DeployError to its exit code.
pub fn exit_code_for_error(_err: &DeployError) -> i32 {
    EXIT_ERROR
}
