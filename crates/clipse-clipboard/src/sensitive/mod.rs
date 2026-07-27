//! Platform-independent, pure privacy checks. Nothing in this module touches
//! the OS clipboard — every function here takes plain text or an app name and
//! returns a verdict, which is what makes it exhaustively unit-testable.

mod apps;
mod secrets;

pub use apps::{AppBlocklist, DEFAULT_BLOCKED_APPS};
pub use secrets::{SecretKind, detect_secret};
