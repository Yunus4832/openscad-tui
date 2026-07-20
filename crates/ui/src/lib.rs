//! Library module exposing public API

pub mod app;
pub mod command_registry;
pub mod commands;
pub mod input;
pub mod preview;
pub mod project_import;
pub mod ui;

pub use app::App;
pub use command_registry::{CommandDef, CommandHandler, CommandRegistry};
pub use commands::{
    cmd_boolean_op, cmd_clear_selection, cmd_delete, cmd_deselect, cmd_insert, cmd_select,
    CommandError, CommandResult,
};
