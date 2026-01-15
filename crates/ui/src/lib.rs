//! Library module exposing public API

pub mod app;
pub mod commands;

pub use app::App;
pub use commands::{
    cmd_boolean_op, cmd_clear_selection, cmd_delete, cmd_deselect, cmd_insert, cmd_select,
    CommandError, CommandResult,
};
