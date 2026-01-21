//! Input handling module - Two modes: Normal and Command
//!
//! Normal mode: Quick keybindings for common operations (i/j/k/h/l/v)
//! Command mode: Free text input for complex commands with parameter input

use crate::app::{App, InputMode};
use crate::commands;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fs;
use std::path::Path;

pub fn handle_key(key: KeyEvent, app: &mut App) {
    match app.input_mode {
        InputMode::Normal => handle_normal_input(key, app),
        InputMode::Command => handle_command_input(key, app),
        InputMode::InsertEnterParams => handle_insert_params_input(key, app),
        InputMode::ReplaceSelectModule => handle_replace_module_input(key, app),
        InputMode::Help => handle_help_input(key, app),
    }
}

/// Normal mode: Quick keybindings
fn handle_normal_input(key: KeyEvent, app: &mut App) {
    match key.code {
        // i - insert module (mapped to :insert command)
        KeyCode::Char('i') => {
            app.input_mode = InputMode::Command;
            app.input_buffer = "insert ".to_string();
        }

        // t - translate
        KeyCode::Char('t') => {
            app.input_mode = InputMode::Command;
            app.input_buffer = "translate ".to_string();
        }

        // s - scale
        KeyCode::Char('s') => {
            app.input_mode = InputMode::Command;
            app.input_buffer = "scale ".to_string();
        }

        // Navigation: j (next), k (prev), h (back/collapse), l (forward/expand)
        KeyCode::Char('j') | KeyCode::Down => {
            execute_command(app, "next");
        }
        KeyCode::Char('k') | KeyCode::Up => {
            execute_command(app, "prev");
        }
        KeyCode::Char('h') | KeyCode::Left => {
            execute_command(app, "collapse");
        }
        KeyCode::Char('l') | KeyCode::Right => {
            execute_command(app, "expand");
        }

        // v - select/toggle node
        KeyCode::Char('v') => {
            execute_command(app, "select");
        }

        // u - undo
        KeyCode::Char('u') => {
            execute_command(app, "undo");
        }

        // r - rotate (Ctrl+r for redo)
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            execute_command(app, "redo");
        }
        KeyCode::Char('r') => {
            app.input_mode = InputMode::Command;
            app.input_buffer = "rotate ".to_string();
        }

        // d - delete node
        KeyCode::Char('d') => {
            execute_command(app, "delete");
        }

        // w - write (save to JSON)
        KeyCode::Char('w') => {
            app.input_mode = InputMode::Command;
            app.input_buffer = "write ".to_string();
            app.set_info("Save to JSON file - enter filename");
        }

        // e - edit (load from JSON)
        KeyCode::Char('e') => {
            app.input_mode = InputMode::Command;
            app.input_buffer = "edit ".to_string();
            app.set_info("Load from JSON file - enter filename");
        }

        // L - library (load library JSON)
        KeyCode::Char('L') => {
            app.input_mode = InputMode::Command;
            app.input_buffer = "library ".to_string();
            app.set_info("Load library from JSON file - enter filename");
        }

        // : - enter command mode
        KeyCode::Char(':') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.clear();
        }

        // Enter - toggle expand/collapse node
        KeyCode::Enter => {
            execute_command(app, "toggle");
        }

        // q - quit
        KeyCode::Char('q') => {
            execute_command(app, "quit");
        }

        // Ctrl+C to quit
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            execute_command(app, "quit");
        }

        // ? - show help
        KeyCode::Char('?') => {
            execute_command(app, "help");
        }

        _ => {}
    }
}

/// Handle input in command mode - text input with echo
fn handle_command_input(key: KeyEvent, app: &mut App) {
    match key.code {
        // Esc to return to Normal mode or cancel completion
        KeyCode::Esc => {
            if app.completion_active {
                // Cancel completion but stay in command mode
                app.completion_active = false;
                app.completion_candidates.clear();
            } else {
                // Exit command mode
                app.input_mode = InputMode::Normal;
                app.input_buffer.clear();
                app.clear_error();
            }
        }

        // Regular character input - with echo
        KeyCode::Char(c) => {
            if app.completion_active {
                // User started typing, exit completion mode
                app.completion_active = false;
                app.completion_candidates.clear();
            }
            app.input_buffer.push(c);
        }

        // Backspace to delete character
        KeyCode::Backspace => {
            if app.completion_active {
                // User started editing, exit completion mode
                app.completion_active = false;
                app.completion_candidates.clear();
            }
            app.input_buffer.pop();
        }

        // Enter to execute command
        KeyCode::Enter => {
            if app.completion_active {
                // Unified behavior: only accept completion, do not execute command
                app.completion_active = false;
                app.completion_candidates.clear();

                // Special handling: if file completion and path is directory, add "/"
                if let crate::app::CompletionContext::File { current_path, .. } =
                    &app.completion_context
                {
                    if !current_path.ends_with('/') && Path::new(current_path).is_dir() {
                        app.input_buffer.push('/');
                    }
                }

                // Note: User needs to press Enter again to execute the command
            } else {
                // Not in completion mode: execute command
                let cmd = app.input_buffer.clone();
                execute_command(app, &cmd);
            }
        }

        // Tab for autocomplete
        KeyCode::Tab => {
            handle_tab_completion(app);
        }

        _ => {}
    }
}

/// Handle module name input for insert command
/// Handle parameter input for insert command (multi-stage)
fn handle_insert_params_input(key: KeyEvent, app: &mut App) {
    match key.code {
        KeyCode::Char(c) => {
            if app.completion_active {
                app.completion_active = false;
                app.completion_candidates.clear();
            }
            app.input_buffer.push(c);
        }
        KeyCode::Backspace => {
            if app.completion_active {
                app.completion_active = false;
                app.completion_candidates.clear();
            }
            app.input_buffer.pop();
        }
        KeyCode::Tab => {
            handle_tab_completion(app);
        }
        KeyCode::Enter => {
            // User finished entering parameters
            let params = app.input_buffer.trim().to_string();
            if let Some(ref module_name) = app.insert_module_name.clone() {
                // Check if module accepts children and we have selections
                if let Some(module_def) = app.library.get_module(module_name) {
                    if module_def.accepts_children && app.selected_nodes.is_empty() {
                        app.set_error(&format!(
                            "'{}' requires child modules. Select modules with 'v' first",
                            module_name
                        ));
                        app.input_mode = InputMode::Normal;
                        app.input_buffer.clear();
                        app.insert_module_name = None;
                        return;
                    }
                }

                app.push_undo();
                if let Err(e) = commands::cmd_insert(app, module_name, None, Some(&params)) {
                    app.set_error(&e.to_string());
                } else {
                    app.update_navigation_status();
                    app.set_info(&format!("Inserted: {}", module_name));
                }
            }
            app.input_mode = InputMode::Normal;
            app.input_buffer.clear();
            app.insert_module_name = None;
        }
        KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
            app.input_buffer.clear();
            app.insert_module_name = None;
            app.set_info("Insert cancelled");
        }
        _ => {}
    }
}

/// Handle module selection for replace command
fn handle_replace_module_input(key: KeyEvent, app: &mut App) {
    match key.code {
        KeyCode::Char(c) => {
            app.input_buffer.push(c);
        }
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }
        KeyCode::Enter => {
            let _module_name = app.input_buffer.trim().to_string();
            app.set_error("Replace command not implemented yet");
            app.input_mode = InputMode::Command;
            app.input_buffer.clear();
        }
        KeyCode::Esc => {
            app.input_mode = InputMode::Command;
            app.input_buffer.clear();
            app.set_info("Replace cancelled");
        }
        _ => {}
    }
}

/// Handle help modal input
fn handle_help_input(key: KeyEvent, app: &mut App) {
    match key.code {
        // Any key to close help modal
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
            app.input_mode = InputMode::Normal;
        }
        _ => {}
    }
}

/// Execute a command using the new command registry
/// This is a transitional function that will eventually replace the old execute_command
fn execute_command_registry(app: &mut App, cmd: &str) -> bool {
    app.input_buffer.clear();

    if cmd.is_empty() {
        return true;
    }

    let parts: Vec<&str> = cmd.split_whitespace().collect();
    let cmd_name = parts[0];
    let args = &parts[1..];

    // First, check if this is a command that should be handled by the registry
    if let Some(cmd_def) = app.command_registry.find(cmd_name) {
        // Validate arguments
        if args.len() < cmd_def.min_args {
            app.set_error(&format!(
                "{} requires at least {} arguments",
                cmd_name, cmd_def.min_args
            ));
            return true;
        }

        if let Some(max) = cmd_def.max_args {
            if args.len() > max {
                app.set_error(&format!("{} accepts at most {} arguments", cmd_name, max));
                return true;
            }
        }

        // Execute the command
        match (cmd_def.handler)(app, args) {
            Ok(_) => {
                // Command succeeded
                // Note: The handler may have already set an info message
            }
            Err(e) => {
                app.set_error(&e.to_string());
            }
        }

        return true;
    }

    // Command not found in registry
    app.set_error(&format!(
        "Unknown command: '{}'. Type 'help' for commands.",
        cmd_name
    ));
    true
}

fn execute_command(app: &mut App, cmd: &str) {
    // Try the new command registry first
    if execute_command_registry(app, cmd) {
        // Command was handled by registry
        // Return to Normal mode if we're in Command mode
        if app.input_mode == InputMode::Command {
            app.input_mode = InputMode::Normal;
        }
        return;
    }

    // Fall back to old implementation for commands not yet migrated
    app.input_buffer.clear();

    if cmd.is_empty() {
        return;
    }

    let parts: Vec<&str> = cmd.split_whitespace().collect();

    // Handle shorthand commands first
    match parts.first() {
        // === Shorthand single-character commands ===

        // q or quit
        Some(&"q") | Some(&"quit") => {
            if let Err(e) = commands::cmd_quit(app) {
                app.set_error(&e.to_string());
            }
        }

        // j/k/down/up - navigate down/up
        Some(&"j") | Some(&"next") | Some(&"down") => {
            if let Err(e) = commands::cmd_next(app) {
                app.set_error(&e.to_string());
            }
        }
        Some(&"k") | Some(&"prev") | Some(&"up") => {
            if let Err(e) = commands::cmd_prev(app) {
                app.set_error(&e.to_string());
            }
        }

        // h/l - collapse/expand
        Some(&"h") | Some(&"collapse") | Some(&"left") => {
            if let Err(e) = commands::cmd_collapse(app) {
                app.set_error(&e.to_string());
            }
        }
        Some(&"l") | Some(&"expand") | Some(&"right") => {
            if let Err(e) = commands::cmd_expand(app) {
                app.set_error(&e.to_string());
            }
        }
        Some(&"toggle") => {
            if let Err(e) = commands::cmd_toggle(app) {
                app.set_error(&e.to_string());
            }
        }

        // v - select/toggle node
        Some(&"v") | Some(&"select") => {
            if let Err(e) = commands::cmd_select_toggle(app) {
                app.set_error(&e.to_string());
            }
        }

        // u - undo
        Some(&"u") | Some(&"undo") => {
            if let Err(e) = commands::cmd_undo(app) {
                app.set_error(&e.to_string());
            }
        }

        // r - redo
        Some(&"r") | Some(&"redo") => {
            if let Err(e) = commands::cmd_redo(app) {
                app.set_error(&e.to_string());
            }
        }

        // === Full commands ===

        // insert <module_name> [params]
        // Shorthand: i <module_name> [params]
        Some(&"insert") | Some(&"i") => {
            if parts.len() < 2 {
                app.set_error("Usage: insert <module_name> [params]");
                app.input_mode = InputMode::Normal;
                return;
            }
            let module_name = parts[1];

            // Get module definition to check parameters and children requirements
            let module_def = app.library.get_module(module_name);

            // Check if this module accepts children
            if let Some(ref mdef) = module_def {
                if mdef.accepts_children && app.selected_nodes.is_empty() {
                    // This module requires child nodes but none are selected
                    app.set_error(&format!(
                        "'{}' requires child modules. Select modules with 'v' first",
                        module_name
                    ));
                    app.input_mode = InputMode::Normal;
                    return;
                }
            }

            let params = if parts.len() > 2 {
                Some(parts[2..].join(" "))
            } else {
                None
            };

            // Check if module has parameters
            let module_has_params = module_def
                .as_ref()
                .is_some_and(|mdef| !mdef.parameters.is_empty());

            // If params not provided and module has parameters, ask for them in next stage
            if params.is_none() && module_has_params {
                app.insert_module_name = Some(module_name.to_string());
                app.input_mode = InputMode::InsertEnterParams;
                app.set_info(&format!(
                    "Enter parameters for '{}' (or press Enter to skip):",
                    module_name
                ));
                return;
            }

            // If no params provided and module has no parameters, use empty params
            let final_params = params.or_else(|| Some(String::new()));

            app.push_undo();
            match commands::cmd_insert(app, module_name, None, final_params.as_deref()) {
                Ok(_) => {
                    app.update_navigation_status();
                    app.set_info(&format!("Inserted: {}", module_name));
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        // funcdef <function_name> [params=body]
        Some(&"funcdef") => {
            if parts.len() < 2 {
                app.set_error("Usage: funcdef <function_name> [(param1, param2, ...)=expression]");
                app.input_mode = InputMode::Normal;
                return;
            }
            let func_name = parts[1];
            let params_body = if parts.len() > 2 {
                Some(parts[2..].join(" "))
            } else {
                None
            };
            app.push_undo();
            match commands::cmd_funcdef(app, func_name, params_body.as_deref()) {
                Ok(_) => {
                    app.update_navigation_status();
                    app.set_info(&format!("Function '{}' defined", func_name));
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        // moddef <module_name> [params]
        Some(&"moddef") => {
            if parts.len() < 2 {
                app.set_error("Usage: moddef <module_name> [params]");
                app.input_mode = InputMode::Normal;
                return;
            }
            let module_name = parts[1];
            let params = if parts.len() > 2 {
                Some(parts[2..].join(" "))
            } else {
                None
            };
            app.push_undo();
            match commands::cmd_moddef(app, module_name, params.as_deref()) {
                Ok(_) => {
                    app.update_navigation_status();
                    app.set_info(&format!("Module '{}' defined", module_name));
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        // delete [node_id]
        // Shorthand: d [node_id] or dd or D
        Some(&"delete") | Some(&"d") | Some(&"dd") | Some(&"D") => {
            let node_id = if parts.len() > 1 {
                parts[1].to_string()
            } else {
                // Use current cursor position
                let selected = { app.tree_state.borrow().selected().last().cloned() };
                match selected {
                    Some(id) => id,
                    None => {
                        app.set_error("No node selected");
                        app.input_mode = InputMode::Normal;
                        return;
                    }
                }
            };

            app.push_undo();
            if let Err(e) = commands::cmd_delete(app, &node_id) {
                app.set_error(&e.to_string());
            } else {
                app.set_info(&format!("Deleted: {}", node_id));
                app.update_navigation_status();
            }
        }

        // translate [params] - apply translate to selected nodes
        Some(&"translate") => {
            // Check if we have selected nodes
            if app.selected_nodes.is_empty() {
                app.set_error("No nodes selected. Select nodes with 'v' first");
                app.input_mode = InputMode::Normal;
                return;
            }

            // Get parameters if provided
            let params = if parts.len() > 1 {
                Some(parts[1..].join(" "))
            } else {
                None
            };

            app.push_undo();
            match commands::cmd_insert(app, "translate", None, params.as_deref()) {
                Ok(_) => {
                    app.set_info("Applied translate to selected nodes");
                    app.update_navigation_status();
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        // rotate [params] - apply rotate to selected nodes
        Some(&"rotate") => {
            // Check if we have selected nodes
            if app.selected_nodes.is_empty() {
                app.set_error("No nodes selected. Select nodes with 'v' first");
                app.input_mode = InputMode::Normal;
                return;
            }

            // Get parameters if provided
            let params = if parts.len() > 1 {
                Some(parts[1..].join(" "))
            } else {
                None
            };

            app.push_undo();
            match commands::cmd_insert(app, "rotate", None, params.as_deref()) {
                Ok(_) => {
                    app.set_info("Applied rotate to selected nodes");
                    app.update_navigation_status();
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        // scale [params] - apply scale to selected nodes
        Some(&"scale") => {
            // Check if we have selected nodes
            if app.selected_nodes.is_empty() {
                app.set_error("No nodes selected. Select nodes with 'v' first");
                app.input_mode = InputMode::Normal;
                return;
            }

            // Get parameters if provided
            let params = if parts.len() > 1 {
                Some(parts[1..].join(" "))
            } else {
                None
            };

            app.push_undo();
            match commands::cmd_insert(app, "scale", None, params.as_deref()) {
                Ok(_) => {
                    app.set_info("Applied scale to selected nodes");
                    app.update_navigation_status();
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        // union - apply union to selected nodes
        Some(&"union") => {
            // Check if we have selected nodes
            if app.selected_nodes.is_empty() {
                app.set_error("No nodes selected. Select nodes with 'v' first");
                app.input_mode = InputMode::Normal;
                return;
            }

            app.push_undo();
            match commands::cmd_insert(app, "union", None, None) {
                Ok(_) => {
                    app.set_info("Applied union to selected nodes");
                    app.update_navigation_status();
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        // difference - apply difference to selected nodes
        Some(&"difference") => {
            // Check if we have selected nodes
            if app.selected_nodes.is_empty() {
                app.set_error("No nodes selected. Select nodes with 'v' first");
                app.input_mode = InputMode::Normal;
                return;
            }

            app.push_undo();
            match commands::cmd_insert(app, "difference", None, None) {
                Ok(_) => {
                    app.set_info("Applied difference to selected nodes");
                    app.update_navigation_status();
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        // intersection - apply intersection to selected nodes
        Some(&"intersection") => {
            // Check if we have selected nodes
            if app.selected_nodes.is_empty() {
                app.set_error("No nodes selected. Select nodes with 'v' first");
                app.input_mode = InputMode::Normal;
                return;
            }

            app.push_undo();
            match commands::cmd_insert(app, "intersection", None, None) {
                Ok(_) => {
                    app.set_info("Applied intersection to selected nodes");
                    app.update_navigation_status();
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        // deselect-all or clear-selection
        Some(&"deselect-all") | Some(&"deselect_all") | Some(&"clear-selection") => {
            if let Err(e) = commands::cmd_deselect_all(app) {
                app.set_error(&e.to_string());
            }
        }

        // yank/copy - y [node_id]
        Some(&"yank") | Some(&"y") => {
            app.set_error("Yank command not implemented yet");
        }

        // paste - p
        Some(&"paste") | Some(&"p") => {
            app.set_error("Paste command not implemented yet");
        }

        // remove - x [node_id]
        Some(&"remove") | Some(&"x") => {
            app.set_error("Remove command not implemented yet");
        }

        // replace - r <node_id> <new_module>
        Some(&"replace") => {
            if parts.len() < 2 {
                app.set_error("Usage: replace <node_id> <new_module_name>");
                app.input_mode = InputMode::Normal;
                return;
            }
            app.set_error("Replace command not implemented yet");
        }

        // write/save - w <filename>.json
        Some(&"write") | Some(&"save") | Some(&"w") => {
            if parts.len() < 2 {
                app.set_error("Usage: write <filename>.json");
                app.input_mode = InputMode::Normal;
                return;
            }
            let filename = parts[1];
            match commands::cmd_write(app, filename) {
                Ok(_) => app.set_info(&format!("✓ Saved to {}", filename)),
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        // edit - edit <filename>.json
        Some(&"edit") | Some(&"e") => {
            if parts.len() < 2 {
                app.set_error("Usage: edit <filename>.json");
                app.input_mode = InputMode::Normal;
                return;
            }
            let filename = parts[1];
            match commands::cmd_load(app, filename) {
                Ok(_) => app.set_info(&format!("Loaded from {}", filename)),
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        // export - export <filename>.scad
        Some(&"export") => {
            if parts.len() < 2 {
                app.set_error("Usage: export <filename>.scad");
                app.input_mode = InputMode::Normal;
                return;
            }
            let filename = parts[1];
            match commands::cmd_export(app, filename) {
                Ok(_) => app.set_info(&format!("Exported to {}", filename)),
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        // library - library <filename>.json
        Some(&"library") => {
            if parts.len() < 2 {
                app.set_error("Usage: library <filename>.json");
                app.input_mode = InputMode::Normal;
                return;
            }
            let filename = parts[1];
            match commands::cmd_load_library(app, filename) {
                Ok(_) => app.set_info(&format!("Loaded library from {}", filename)),
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        // help - ?
        Some(&"help") | Some(&"?") => {
            if let Err(e) = commands::cmd_help(app) {
                app.set_error(&e.to_string());
            }
        }

        // global <var_name>=<value> - define a global variable
        Some(&"global") => {
            if parts.len() < 2 {
                app.set_error("Usage: global <name>=<value>");
                app.input_mode = InputMode::Normal;
                return;
            }
            let var_spec = parts[1];
            app.push_undo();
            match commands::cmd_global(app, var_spec) {
                Ok(_) => {
                    app.update_navigation_status();
                    app.set_info(&format!(
                        "Global variable '{}' defined",
                        var_spec.split('=').next().unwrap_or("<invalid>")
                    ));
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        _ => {
            app.set_error(&format!(
                "Unknown command: '{}'. Type 'help' for commands.",
                parts.first().unwrap_or(&"")
            ));
        }
    }

    // Return to Normal mode if we're in Command mode
    if app.input_mode == InputMode::Command {
        app.input_mode = InputMode::Normal;
    }
}

/// Handle Tab key for autocompletion
fn handle_tab_completion(app: &mut App) {
    if !app.completion_active {
        // First Tab press: generate completions
        let (candidates, context) = generate_completions(&app.input_buffer, app);
        if candidates.is_empty() {
            // No completions available
            return;
        }
        app.completion_candidates = candidates;
        app.completion_context = context;
        app.completion_index = 0;
        app.completion_active = true;

        // Apply the first completion
        apply_completion(app);
    } else {
        // Already in completion mode: cycle to next candidate
        app.completion_index = (app.completion_index + 1) % app.completion_candidates.len();
        apply_completion(app);

        // New: if we completed to a directory, automatically add "/"
        if let crate::app::CompletionContext::File { current_path, .. } = &app.completion_context {
            if !current_path.ends_with('/') && Path::new(current_path).is_dir() {
                app.input_buffer.push('/');
                // Clear candidates so next Tab will show directory contents
                app.completion_candidates.clear();
            }
        }
    }
}

/// Parse parameters from a string, returning parameter names that have been entered
/// Parameters are separated by commas, not spaces
fn parse_parameter_names(param_str: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut current = String::new();
    let mut in_brackets = 0;

    for ch in param_str.chars() {
        match ch {
            '[' => in_brackets += 1,
            ']' if in_brackets > 0 => in_brackets -= 1,
            ',' if in_brackets == 0 => {
                // End of a parameter
                if let Some(equals_pos) = current.find('=') {
                    let name = current[..equals_pos].trim().to_string();
                    if !name.is_empty() {
                        names.push(name);
                    }
                }
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }

    // Check the last parameter
    if !current.trim().is_empty() {
        if let Some(equals_pos) = current.find('=') {
            let name = current[..equals_pos].trim().to_string();
            if !name.is_empty() {
                names.push(name);
            }
        }
    }

    names
}

/// Generate completion candidates based on current input buffer
fn generate_completions(input: &str, app: &App) -> (Vec<String>, crate::app::CompletionContext) {
    // Check if we're in InsertEnterParams mode
    if app.input_mode == crate::app::InputMode::InsertEnterParams {
        // Completing parameters for a module
        if let Some(ref module_name) = app.insert_module_name {
            if let Some(module_def) = app.library.get_module(module_name) {
                // Parse already entered parameters
                let entered_names = parse_parameter_names(input);

                // Filter out already entered parameters
                let candidates: Vec<String> = module_def
                    .parameters
                    .iter()
                    .filter(|p| !entered_names.contains(&p.name))
                    .map(|p| p.name.clone())
                    .collect();

                return (
                    candidates,
                    crate::app::CompletionContext::ModuleParam {
                        module_name: module_name.clone(),
                        param_index: entered_names.len(),
                    },
                );
            }
        }
        return (Vec::new(), crate::app::CompletionContext::Command);
    }

    // Empty input: complete commands
    if input.trim().is_empty() {
        let commands = get_command_list(app);
        let mut candidates: Vec<String> = commands;

        candidates.sort();
        return (candidates, crate::app::CompletionContext::Command);
    }

    // Check if input starts with "insert " or "i "
    if input.starts_with("insert ") || input.starts_with("i ") {
        let cmd_len = if input.starts_with("insert ") { 7 } else { 2 };

        let after_cmd = &input[cmd_len..];

        // Check if after_cmd is empty or just whitespace
        if after_cmd.trim().is_empty() {
            // Just "insert " or "i " with nothing after: complete module names
            let modules = get_module_list(app);
            let mut candidates: Vec<String> = modules.iter().map(|s| s.to_string()).collect();

            candidates.sort();
            return (candidates, crate::app::CompletionContext::Module);
        }

        // Find the module name (first word after command)
        let after_cmd_trimmed = after_cmd.trim_start(); // Keep trailing spaces
        let mut module_end = 0;
        let mut in_module_name = true;

        for (i, ch) in after_cmd_trimmed.char_indices() {
            if in_module_name {
                if ch.is_whitespace() {
                    // End of module name
                    in_module_name = false;
                } else {
                    module_end = i + ch.len_utf8();
                }
            }
        }

        let module_name_part = &after_cmd_trimmed[..module_end];
        let after_module = &after_cmd_trimmed[module_end..];

        if module_name_part.is_empty() {
            // No module name typed yet
            let prefix = after_cmd_trimmed;
            let modules = get_module_list(app);
            let candidates: Vec<String> = modules
                .iter()
                .filter(|module| module.starts_with(prefix))
                .map(|s| s.to_string())
                .collect();

            let mut sorted = candidates;
            sorted.sort();
            return (sorted, crate::app::CompletionContext::Module);
        }

        // We have a module name, check if it exists
        if let Some(module_def) = app.library.get_module(module_name_part) {
            // Check if there's already content after module name
            if after_module.trim().is_empty() {
                // Just module name, no parameters yet: complete first parameter
                let candidates: Vec<String> = module_def
                    .parameters
                    .iter()
                    .map(|p| p.name.clone())
                    .collect();

                return (
                    candidates,
                    crate::app::CompletionContext::ModuleParam {
                        module_name: module_name_part.to_string(),
                        param_index: 0,
                    },
                );
            } else {
                // Has some content after module name: could be partial parameters
                // Parse parameter names from after_module
                let entered_names = parse_parameter_names(after_module);

                // Filter out already entered parameters
                let candidates: Vec<String> = module_def
                    .parameters
                    .iter()
                    .filter(|p| !entered_names.contains(&p.name))
                    .map(|p| p.name.clone())
                    .collect();

                return (
                    candidates,
                    crate::app::CompletionContext::ModuleParam {
                        module_name: module_name_part.to_string(),
                        param_index: entered_names.len(),
                    },
                );
            }
        } else {
            // Module not found, try to complete module name
            let prefix = module_name_part;
            let modules = get_module_list(app);
            let candidates: Vec<String> = modules
                .iter()
                .filter(|module| module.starts_with(prefix))
                .map(|s| s.to_string())
                .collect();

            let mut sorted = candidates;
            sorted.sort();
            return (sorted, crate::app::CompletionContext::Module);
        }
    }

    // Check for file commands: write/save/w, edit/e, export, library
    let file_commands = ["write", "save", "w", "edit", "e", "export", "library"];
    for cmd in &file_commands {
        if input.starts_with(&format!("{} ", cmd)) {
            // Extract the path part after the command
            let after_cmd = &input[cmd.len() + 1..]; // +1 for space
            let path_prefix = after_cmd.trim();

            // Determine current directory for file completion
            let path = Path::new(path_prefix);
            let (current_dir, at_path_end) = if path.is_dir() || path_prefix.ends_with('/') {
                // If it's a directory or ends with /, we're listing this directory
                (path_prefix.to_string(), true)
            } else if let Some(parent) = path.parent() {
                // If it has a parent, we're listing the parent directory
                (parent.to_string_lossy().into_owned(), false)
            } else {
                // No parent, use current directory
                (".".to_string(), false)
            };

            let candidates = get_file_completions(&current_dir, path_prefix);
            return (
                candidates,
                crate::app::CompletionContext::File {
                    current_path: path_prefix.to_string(),
                    current_dir,
                    at_path_end,
                },
            );
        }
    }

    // Other cases: command completion for the first word
    let first_word = input.split_whitespace().next().unwrap_or("");
    let commands = get_command_list(app);
    let candidates: Vec<String> = commands
        .iter()
        .filter(|cmd| cmd.starts_with(first_word))
        .map(|s| s.to_string())
        .collect();

    let mut sorted = candidates;
    sorted.sort();
    (sorted, crate::app::CompletionContext::Command)
}

/// Apply the current completion to the input buffer
fn apply_completion(app: &mut App) {
    if app.completion_candidates.is_empty()
        || app.completion_index >= app.completion_candidates.len()
    {
        return;
    }

    let completion = &app.completion_candidates[app.completion_index];

    // Determine what to replace based on context and current input
    match &app.completion_context {
        crate::app::CompletionContext::Command => {
            // Replace the current word (or whole buffer if empty)
            if app.input_buffer.trim().is_empty() {
                app.input_buffer = completion.clone();
            } else {
                // Find the current word being typed
                let current = app.input_buffer.clone();
                if let Some(last_space) = current.rfind(' ') {
                    // Replace from last space to end
                    app.input_buffer = current[..last_space + 1].to_string() + completion;
                } else {
                    // No space, replace entire buffer
                    app.input_buffer = completion.clone();
                }
            }
        }
        crate::app::CompletionContext::Module => {
            // For module completion, we're after "insert " or "i "
            // Replace the module name part
            let current = app.input_buffer.clone();
            if current.starts_with("insert ") {
                // Keep "insert " prefix
                app.input_buffer = "insert ".to_string() + completion;
            } else if current.starts_with("i ") {
                // Keep "i " prefix
                app.input_buffer = "i ".to_string() + completion;
            } else {
                // Fallback: just replace the current word
                if let Some(last_space) = current.rfind(' ') {
                    app.input_buffer = current[..last_space + 1].to_string() + completion;
                } else {
                    app.input_buffer = completion.clone();
                }
            }
        }
        crate::app::CompletionContext::ModuleParam {
            module_name,
            param_index: _,
        } => {
            // For parameter completion, handle comma-separated parameters
            let current = &app.input_buffer;

            // Find the module name in the input
            // The input could be "insert cube" or "i cube" or "insert cube size=[1,2,3],"
            // We need to find where the module name ends
            let (module_pos, after_module) = if let Some(pos) = current.find(module_name.as_str()) {
                (pos, &current[pos + module_name.len()..])
            } else {
                // Module name not found (shouldn't happen), fallback to end
                (current.len(), "")
            };

            // Find the last comma in after_module (if any)
            let last_comma_pos = after_module.rfind(',');

            // Check if user is typing a parameter name (partial word after last comma)
            let is_typing_param_name = if let Some(comma_pos) = last_comma_pos {
                // There is a comma, check text after last comma for '='
                let after_last_comma = &after_module[comma_pos + 1..];
                !after_last_comma.trim().is_empty() && !after_last_comma.contains('=')
            } else {
                // No comma, check entire after_module for '='
                !after_module.trim().is_empty() && !after_module.contains('=')
            };

            if is_typing_param_name {
                // User is typing a parameter name (partial)
                // Replace everything after the last comma (or entire after_module) with new param
                let replace_start = if let Some(comma_pos) = last_comma_pos {
                    module_pos + module_name.len() + comma_pos + 1
                } else {
                    module_pos + module_name.len()
                };
                // Keep everything up to replace_start, add space if needed, then param with =
                let prefix = &current[..replace_start];
                // Ensure there's a space before param if not already present
                let needs_space = !prefix.ends_with(' ') && !prefix.ends_with(',');
                app.input_buffer = prefix.to_string();
                if needs_space {
                    app.input_buffer.push(' ');
                }
                app.input_buffer.push_str(completion);
                app.input_buffer.push('=');
            } else if after_module.trim().ends_with(',') {
                // Ends with comma, ready for next parameter
                // Add space and new parameter
                app.input_buffer.push(' ');
                app.input_buffer.push_str(completion);
                app.input_buffer.push('=');
            } else if after_module.trim().is_empty() {
                // No parameters yet, just module name
                // Add parameter with "="
                app.input_buffer.push_str(completion);
                app.input_buffer.push('=');
            } else {
                // Has parameters but doesn't end with comma
                // Add comma, space, and new parameter
                app.input_buffer.push_str(", ");
                app.input_buffer.push_str(completion);
                app.input_buffer.push('=');
            }
        }

        crate::app::CompletionContext::File {
            current_path,
            current_dir: _,
            at_path_end,
        } => {
            // For file completion, we need to update the path based on the selected completion
            let current = &app.input_buffer;

            // Find the command (first word) and the path part after it
            let first_space = current.find(' ');
            if let Some(space_pos) = first_space {
                let command = &current[..space_pos];

                // Build new path using Path::join to handle slashes correctly
                let new_path = if *at_path_end {
                    // We're at the end of current path, append completion
                    if current_path.is_empty() {
                        completion.to_string()
                    } else {
                        // Use Path::join to properly handle path separators
                        Path::new(current_path)
                            .join(completion)
                            .to_string_lossy()
                            .into_owned()
                    }
                } else {
                    // Replace the last component of the path
                    let path = Path::new(current_path);
                    if let Some(parent) = path.parent() {
                        parent.join(completion).to_string_lossy().into_owned()
                    } else {
                        completion.to_string()
                    }
                };

                // Don't add trailing slash here - let Enter do that
                // Update the input buffer with command and new path
                app.input_buffer = format!("{} {}", command, new_path);
            }
        }
    }
}

/// Get list of available commands from the command registry
fn get_command_list(app: &App) -> Vec<String> {
    app.command_registry.get_all_names()
}

/// Get list of available modules from library
fn get_module_list(app: &App) -> Vec<String> {
    app.library.get_module_names()
}

/// Get file completions for a given directory and path prefix
/// Returns entries in the directory that match the prefix
fn get_file_completions(dir_path: &str, prefix: &str) -> Vec<String> {
    let mut completions = Vec::new();

    // Parse the directory path
    let dir = Path::new(dir_path);

    // Extract the partial filename to match from the prefix
    // If prefix ends with /, we're matching empty string (showing all entries)
    let partial_name = if prefix.ends_with('/') {
        String::new()
    } else {
        // Get the last component of the prefix
        let prefix_path = Path::new(prefix);
        prefix_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };

    // Try to read the directory
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(file_name) = entry.file_name().into_string() {
                // Check if it matches the partial name
                if file_name.starts_with(&partial_name) {
                    // Add the file/directory name (not full path)
                    completions.push(file_name);
                }
            }
        }
    }

    // Sort alphabetically
    completions.sort();
    completions
}
