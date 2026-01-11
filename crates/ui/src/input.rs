//! Input handling module - Two modes: Normal and Command
//!
//! Normal mode: Quick keybindings for common operations (i/j/k/h/l/v)
//! Command mode: Free text input for complex commands with parameter input

use crate::app::{App, InputMode};
use crate::commands;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn handle_key(key: KeyEvent, app: &mut App) {
    match app.input_mode {
        InputMode::Normal => handle_normal_input(key, app),
        InputMode::Command => handle_command_input(key, app),
        InputMode::InsertEnterParams => handle_insert_params_input(key, app),
        InputMode::ReplaceSelectModule => handle_replace_module_input(key, app),
    }
}

/// Normal mode: Quick keybindings
fn handle_normal_input(key: KeyEvent, app: &mut App) {
    match key.code {
        // i - insert module (mapped to :insert command)
        KeyCode::Char('i') => {
            app.input_mode = InputMode::Command;
            app.input_buffer = "insert ".to_string();
            app.set_info("Insert mode - enter module name (type 'help' for available modules)");
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

        // r - redo
        KeyCode::Char('r') => {
            execute_command(app, "redo");
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
            app.set_info("Command mode - type 'help' for available commands");
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

        _ => {}
    }
}

/// Handle input in command mode - text input with echo
fn handle_command_input(key: KeyEvent, app: &mut App) {
    match key.code {
        // Esc to return to Normal mode
        KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
            app.input_buffer.clear();
            app.clear_error();
        }

        // Regular character input - with echo
        KeyCode::Char(c) => {
            app.input_buffer.push(c);
        }

        // Backspace to delete character
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }

        // Enter to execute command
        KeyCode::Enter => {
            let cmd = app.input_buffer.clone();
            execute_command(app, &cmd);
        }

        // Tab for autocomplete
        KeyCode::Tab => {
            // TODO: Implement command/module autocomplete
            app.input_buffer.push('\t');
        }

        _ => {}
    }
}

/// Handle module name input for insert command
/// Handle parameter input for insert command (multi-stage)
fn handle_insert_params_input(key: KeyEvent, app: &mut App) {
    match key.code {
        KeyCode::Char(c) => {
            app.input_buffer.push(c);
        }
        KeyCode::Backspace => {
            app.input_buffer.pop();
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

fn execute_command(app: &mut App, cmd: &str) {
    app.input_buffer.clear();

    if cmd.is_empty() {
        return;
    }

    let parts: Vec<&str> = cmd.split_whitespace().collect();

    // Handle shorthand commands first
    match parts.get(0) {
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
                .map_or(false, |mdef| !mdef.parameters.is_empty());

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

        // union/difference/intersection <node1> <node2> ...
        Some(&"union") => {
            if app.selected_nodes.is_empty() {
                app.set_error("No nodes selected. Usage: select node1, select node2, union");
                return;
            }
            let nodes = app.selected_nodes.clone();
            app.push_undo();
            match commands::cmd_boolean_op(app, "union", &nodes) {
                Ok(_) => {
                    app.selected_nodes.clear();
                    app.set_info("Union operation completed");
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        Some(&"difference") => {
            if app.selected_nodes.is_empty() {
                app.set_error("No nodes selected. Usage: select node1, select node2, difference");
                return;
            }
            let nodes = app.selected_nodes.clone();
            app.push_undo();
            match commands::cmd_boolean_op(app, "difference", &nodes) {
                Ok(_) => {
                    app.selected_nodes.clear();
                    app.set_info("Difference operation completed");
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        Some(&"intersection") => {
            if app.selected_nodes.is_empty() {
                app.set_error("No nodes selected. Usage: select node1, select node2, intersection");
                return;
            }
            let nodes = app.selected_nodes.clone();
            app.push_undo();
            match commands::cmd_boolean_op(app, "intersection", &nodes) {
                Ok(_) => {
                    app.selected_nodes.clear();
                    app.set_info("Intersection operation completed");
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
                return;
            }
            app.set_error("Replace command not implemented yet");
        }

        // write/save - w <filename>.json
        Some(&"write") | Some(&"save") | Some(&"w") => {
            if parts.len() < 2 {
                app.set_error("Usage: write <filename>.json");
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
            let help_text = "OpenSCAD TUI - Command Reference\n\
                \n\
                Navigation (tree is visual only, use commands):\n\
                  j/up, k/down - move cursor up/down\n\
                  h/left, l/right - collapse/expand nodes\n\
                \n\
                Selection & Operations:\n\
                  v/select - toggle select node at cursor\n\
                  i/insert <name> [params] - insert module\n\
                  d/delete [id] - delete node (uses cursor if no id)\n\
                  union/difference/intersection - boolean ops on selected\n\
                \n\
                Editing:\n\
                  u/undo - undo last operation\n\
                  r/redo - redo last operation\n\
                  y/yank, p/paste - copy/paste (not implemented)\n\
                  x/remove - remove node (not implemented)\n\
                \n\
                File Operations:\n\
                  w/write <file> - save to YAML\n\
                  edit <file> - load from YAML\n\
                  export <file> - export OpenSCAD code\n\
                \n\
                Other:\n\
                  q/quit - exit application\n\
                  help/? - show this help";
            app.set_error(help_text);
        }

        _ => {
            app.set_error(&format!(
                "Unknown command: '{}'. Type 'help' for commands.",
                parts.get(0).unwrap_or(&"")
            ));
        }
    }

    // Return to Normal mode if we're in Command mode
    if app.input_mode == InputMode::Command {
        app.input_mode = InputMode::Normal;
    }
}
