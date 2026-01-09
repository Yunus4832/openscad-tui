//! Input handling module - Command-line focused interface
//!
//! All operations are commands. Focus is always on the command line at the bottom.
//! The tree is just a visual representation of the current AST state.
//! Navigation and selection are also expressed as commands.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::{App, InputMode};
use crate::commands;

pub fn handle_key(key: KeyEvent, app: &mut App) {
    match app.input_mode {
        InputMode::Command => handle_command_input(key, app),
        InputMode::InsertEnterParams => handle_insert_params_input(key, app),
        InputMode::ReplaceSelectModule => handle_replace_module_input(key, app),
        // Legacy modes - fallback to command input
        _ => handle_command_input(key, app),
    }
}

/// Handle input in command mode - all input goes through the command line
fn handle_command_input(key: KeyEvent, app: &mut App) {
    match key.code {
        // Ctrl+C or Ctrl+D to quit (check modifiers first, before general Char)
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        
        // Regular character input
        KeyCode::Char(c) => {
            app.input_buffer.push(c);
        }
        
        // Backspace to delete character
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }
        
        // Enter to execute command
        KeyCode::Enter => {
            execute_command(app);
        }
        
        // Tab for autocomplete
        KeyCode::Tab => {
            // TODO: Implement command/module autocomplete
            app.input_buffer.push('\t');
        }
        
        // Esc to clear input buffer
        KeyCode::Esc => {
            app.input_buffer.clear();
            app.clear_error();
        }
        
        _ => {}
    }
}

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
                app.push_undo();
                if let Err(e) = commands::cmd_insert(app, module_name, None, Some(&params)) {
                    app.set_error(&e.to_string());
                } else {
                    app.clear_error();
                }
            }
            app.input_mode = InputMode::Command;
            app.input_buffer.clear();
            app.insert_module_name = None;
            app.set_error("Insert complete. Next command:");
        }
        KeyCode::Esc => {
            app.input_mode = InputMode::Command;
            app.input_buffer.clear();
            app.insert_module_name = None;
            app.set_error("Insert cancelled");
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
            app.set_error("Replace cancelled");
        }
        _ => {}
    }
}

fn execute_command(app: &mut App) {
    let cmd = app.input_buffer.trim().to_string();
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
            app.should_quit = true;
        }
        
        // j/k - navigate down/up
        Some(&"j") | Some(&"down") => {
            app.tree_state.borrow_mut().key_down();
            app.clear_error();
        }
        Some(&"k") | Some(&"up") => {
            app.tree_state.borrow_mut().key_up();
            app.clear_error();
        }
        
        // h/l - collapse/expand
        Some(&"h") | Some(&"left") => {
            app.tree_state.borrow_mut().key_left();
            app.clear_error();
        }
        Some(&"l") | Some(&"right") => {
            app.tree_state.borrow_mut().key_right();
            app.clear_error();
        }
        
        // v - select/toggle node
        Some(&"v") | Some(&"select") => {
            let selected = {
                app.tree_state.borrow().selected().last().cloned()
            };
            if let Some(node_id) = selected {
                if app.selected_nodes.contains(&node_id) {
                    app.selected_nodes.retain(|n| n != &node_id);
                    app.set_error(&format!("Deselected: {}", node_id));
                } else {
                    app.selected_nodes.push(node_id.clone());
                    app.set_error(&format!("Selected: {}", node_id));
                }
            } else {
                app.set_error("No node selected");
            }
        }
        
        // u - undo
        Some(&"u") | Some(&"undo") => {
            app.undo();
            app.clear_error();
        }
        
        // r - redo (or replace)
        Some(&"r") | Some(&"redo") => {
            app.redo();
            app.clear_error();
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
            let params = if parts.len() > 2 {
                Some(parts[2..].join(" "))
            } else {
                None
            };
            
            // If no params provided, ask for them in next stage
            if params.is_none() {
                app.insert_module_name = Some(module_name.to_string());
                app.input_mode = InputMode::InsertEnterParams;
                app.set_error(&format!("Enter parameters for '{}' (or press Enter to skip):", module_name));
                return;
            }
            
            app.push_undo();
            match commands::cmd_insert(app, module_name, None, params.as_deref()) {
                Ok(_) => {
                    app.clear_error();
                    app.set_error(&format!("Inserted: {}", module_name));
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }
        
        // delete [node_id]
        // Shorthand: d [node_id]
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
                app.set_error(&format!("Deleted: {}", node_id));
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
                    app.set_error("Union operation completed");
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
                    app.set_error("Difference operation completed");
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
                    app.set_error("Intersection operation completed");
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }
        
        // yank/copy - y <node_id>
        Some(&"yank") | Some(&"y") => {
            app.set_error("Yank command not implemented yet");
        }
        
        // paste - p
        Some(&"paste") | Some(&"p") => {
            app.set_error("Paste command not implemented yet");
        }
        
        // remove - x <node_id>
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
        
        // write/save - w <filename>.yaml
        Some(&"write") | Some(&"save") | Some(&"w") => {
            if parts.len() < 2 {
                app.set_error("Usage: write <filename>.yaml");
                return;
            }
            let filename = parts[1];
            app.set_error(&format!("Write to {} not implemented yet", filename));
        }
        
        // load - load <filename>.scad
        Some(&"load") => {
            if parts.len() < 2 {
                app.set_error("Usage: load <filename>.scad");
                return;
            }
            let filename = parts[1];
            app.set_error(&format!("Load from {} not implemented yet", filename));
        }
        
        // edit - edit <filename>.yaml
        Some(&"edit") | Some(&"e") => {
            if parts.len() < 2 {
                app.set_error("Usage: edit <filename>.yaml");
                return;
            }
            let filename = parts[1];
            app.set_error(&format!("Edit {} not implemented yet", filename));
        }
        
        // export - export <filename>.scad
        Some(&"export") => {
            if parts.len() < 2 {
                app.set_error("Usage: export <filename>.scad");
                return;
            }
            let filename = parts[1];
            app.set_error(&format!("Export to {} not implemented yet", filename));
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
            app.set_error(&format!("Unknown command: '{}'. Type 'help' for commands.", 
                parts.get(0).unwrap_or(&"")));
        }
    }
}
