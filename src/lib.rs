//! Library behind the rc-sort CLI. Not a stable API; use the binary.

pub mod blocks;
pub mod classify;
pub mod diff;
pub mod install;
pub mod prompt;
pub mod shell;
pub mod sort;
pub mod state;
pub mod tidy;
#[cfg(feature = "tui")]
pub mod tidy_tui;
pub mod watch;
