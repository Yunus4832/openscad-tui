//! Input handling module

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::App;
use crate::commands;

pub fn handle_key(key: KeyEvent, app: &mut App) {
    if app.command_mode {
        handle_command_input(key, app);
    } else {
        handle_normal_input(key, app);
    }
}

fn handle_normal_input(key: KeyEvent, app: &mut App) {
    match key.code {
        // Tree navigation: jkhl (vim style)
        KeyCode::Char('j') => {
            app.tree_state.borrow_mut().key_down();
            app.clear_error();
        }
        KeyCode::Char('k') => {
            app.tree_state.borrow_mut().key_up();
            app.clear_error();
        }
        KeyCode::Char('h') => {
            app.tree_state.borrow_mut().key_left();
            app.clear_error();
        }
        KeyCode::Char('l') => {
            app.tree_state.borrow_mut().key_right();
            app.clear_error();
        }
        // Also support arrow keys
        KeyCode::Down => {
            app.tree_state.borrow_mut().key_down();
            app.clear_error();
        }
        KeyCode::Up => {
            app.tree_state.borrow_mut().key_up();
            app.clear_error();
        }
        KeyCode::Left => {
            app.tree_state.borrow_mut().key_left();
            app.clear_error();
        }
        KeyCode::Right => {
            app.tree_state.borrow_mut().key_right();
            app.clear_error();
        }
        // Select node with 'v'
        KeyCode::Char('v') => {
            // Get the LEAF node (last element of path) and mark it as selected
            let selected = {
                app.tree_state.borrow().selected().last().cloned()
            };
            if let Some(node_id) = selected {
                if app.selected_nodes.contains(&node_id) {
                    app.selected_nodes.retain(|n| n != &node_id);
                } else {
                    app.selected_nodes.push(node_id);
                }
                app.clear_error();
            } else {
                app.set_error("No node selected");
            }
        }
        // Toggle command mode with ':'
        KeyCode::Char(':') => {
            app.toggle_command_mode();
        }
        // Undo with 'u'
        KeyCode::Char('u') => {
            app.undo();
            app.clear_error();
        }
        // Redo with 'r'
        KeyCode::Char('r') => {
            app.redo();
            app.clear_error();
        }
        // Clear selection with Esc
        KeyCode::Esc => {
            app.selected_nodes.clear();
            app.clear_error();
        }
        // Quit with Ctrl+Q
        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        _ => {}
    }
}

fn handle_command_input(key: KeyEvent, app: &mut App) {
    match key.code {
        KeyCode::Char(c) => {
            app.input_buffer.push(c);
        }
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }
        KeyCode::Enter => {
            execute_command(app);
        }
        KeyCode::Esc => {
            app.toggle_command_mode();
            app.input_buffer.clear();
        }
        KeyCode::Tab => {
            app.input_buffer.push('\t');
        }
        _ => {}
    }
}

fn execute_command(app: &mut App) {
    // Copy the string to avoid long-lived borrows
    let cmd = app.input_buffer.trim().to_string();
    app.input_buffer.clear();
    app.toggle_command_mode();

    if cmd.is_empty() {
        return;
    }

    let parts: Vec<&str> = cmd.split_whitespace().collect();
    match parts.get(0) {
        Some(&"insert") => {
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
            
            app.push_undo();
            match commands::cmd_insert(app, module_name, None, params.as_deref()) {
                Ok(_) => app.clear_error(),
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        Some(&"delete") => {
            // Get the LEAF node (last element of path)
            let node_id = {
                app.tree_state.borrow().selected().last().cloned()
            };
            if let Some(node_id) = node_id {
                app.push_undo();
                if let Err(e) = commands::cmd_delete(app, &node_id) {
                    app.set_error(&e.to_string());
                }
            } else {
                app.set_error("No node selected");
            }
        }

        Some(&"union") | Some(&"difference") | Some(&"intersection") => {
            if app.selected_nodes.is_empty() {
                app.set_error("No nodes selected for boolean operation");
                return;
            }
            let nodes = {
                app.selected_nodes.clone()
            };
            let op_name = parts[0].to_string();
            app.push_undo();
            match commands::cmd_boolean_op(app, &op_name, &nodes) {
                Ok(_) => {
                    app.selected_nodes.clear();
                    app.clear_error();
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        Some(&"write") => {
            app.set_error("Write command not implemented yet");
        }

        Some(&"load") => {
            app.set_error("Load command not implemented yet");
        }

        Some(&"export") => {
            app.set_error("Export command not implemented yet");
        }

        Some(&"help") => {
            let help_text = "OpenSCAD TUI - Commands:\n\
                \n\
                Normal mode:\n\
                  j/k - navigate down/up\n\
                  h/l - collapse/expand node\n\
                  v - select/deselect node\n\
                  u - undo\n\
                  r - redo\n\
                  : - enter command mode\n\
                  Ctrl+Q - quit\n\
                \n\
                Command mode:\n\
                  insert <name> [params] - insert module\n\
                  delete - delete selected node\n\
                  union - union selected nodes\n\
                  difference - difference operation\n\
                  intersection - intersection operation\n\
                  help - show this help";
            app.set_error(help_text);
        }

        _ => {
            app.set_error(&format!("Unknown command: {}", cmd));
        }
    }
}
