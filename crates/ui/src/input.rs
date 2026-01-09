//! Input handling module - Vim-like keybindings with multi-stage interactions

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::{App, InputMode};
use crate::commands;

pub fn handle_key(key: KeyEvent, app: &mut App) {
    match app.input_mode {
        InputMode::Normal => handle_normal_input(key, app),
        InputMode::Command => handle_command_input(key, app),
        InputMode::InsertSelectModule => handle_insert_select_module_input(key, app),
        InputMode::InsertEnterParams => handle_insert_params_input(key, app),
        InputMode::ReplaceSelectModule => handle_replace_select_module_input(key, app),
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
        
        // Insert command: i (after), a (before)
        // These enter InsertSelectModule mode where user can search/select modules
        KeyCode::Char('i') => {
            app.enter_insert_mode(true);  // true = insert after
            app.set_error("Insert after - Type module name to search:");
        }
        KeyCode::Char('a') => {
            app.enter_insert_mode(false);  // false = insert before
            app.set_error("Insert before - Type module name to search:");
        }
        
        // Select mode with 'v' - toggle visual selection
        KeyCode::Char('v') => {
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
        
        // Delete commands: dd or D
        KeyCode::Char('d') | KeyCode::Char('D') => {
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
        
        // Remove command: x (delete node, move children to parent)
        KeyCode::Char('x') => {
            app.set_error("Remove command not implemented yet");
        }
        
        // Yank (copy): y
        KeyCode::Char('y') => {
            let _selected = {
                app.tree_state.borrow().selected().last().cloned()
            };
            app.set_error("Yank command not implemented yet");
        }
        
        // Paste: p (paste below)
        KeyCode::Char('p') => {
            app.set_error("Paste command not implemented yet");
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
        
        // Replace command: R
        KeyCode::Char('R') => {
            app.set_error("Replace command not implemented yet");
        }
        
        // Toggle command mode with ':'
        KeyCode::Char(':') => {
            app.toggle_command_mode();
        }
        
        // Clear selection with Esc
        KeyCode::Esc => {
            app.selected_nodes.clear();
            app.clear_error();
        }
        
        // Write (save as YAML): w
        KeyCode::Char('w') => {
            app.set_error("Usage: :write <filename>.yaml");
        }
        
        // Edit (load YAML): e
        KeyCode::Char('e') => {
            app.set_error("Usage: :edit <filename>.yaml");
        }
        
        // Quit with 'q'
        KeyCode::Char('q') => {
            app.should_quit = true;
        }
        
        // Quit with Ctrl+C
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
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

/// Handle input while selecting a module in insert mode
fn handle_insert_select_module_input(key: KeyEvent, app: &mut App) {
    match key.code {
        KeyCode::Char(c) => {
            app.input_buffer.push(c);
            // Display filtered modules as user types
        }
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }
        KeyCode::Enter => {
            // User confirmed module selection
            let module_name = app.input_buffer.trim().to_string();
            if module_name.is_empty() {
                app.set_error("Module name cannot be empty");
                return;
            }
            
            app.insert_module_name = Some(module_name.clone());
            // For now, insert directly (would check for params if needed)
            app.push_undo();
            if let Err(e) = commands::cmd_insert(app, &module_name, None, None) {
                app.set_error(&e.to_string());
                app.exit_insert_mode();
            } else {
                app.clear_error();
                app.exit_insert_mode();
            }
        }
        KeyCode::Esc => {
            app.exit_insert_mode();
            app.set_error("Insert cancelled");
        }
        KeyCode::Tab => {
            // Tab for autocomplete
            app.input_buffer.push('\t');
        }
        _ => {}
    }
}

/// Handle input while entering parameters for insert mode
fn handle_insert_params_input(key: KeyEvent, app: &mut App) {
    match key.code {
        KeyCode::Char(c) => {
            app.input_buffer.push(c);
        }
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }
        KeyCode::Enter => {
            // Parse and insert with parameters
            let params = app.input_buffer.trim().to_string();
            let module_name = app.insert_module_name.clone();
            if let Some(ref name) = module_name {
                app.push_undo();
                if let Err(e) = commands::cmd_insert(app, name, None, Some(&params)) {
                    app.set_error(&e.to_string());
                } else {
                    app.clear_error();
                    app.exit_insert_mode();
                }
            }
        }
        KeyCode::Esc => {
            app.exit_insert_mode();
            app.set_error("Parameter input cancelled");
        }
        _ => {}
    }
}

/// Handle input while selecting a module for replace mode
fn handle_replace_select_module_input(key: KeyEvent, app: &mut App) {
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
            app.input_mode = InputMode::Normal;
            app.input_buffer.clear();
        }
        KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
            app.input_buffer.clear();
            app.set_error("Replace cancelled");
        }
        _ => {}
    }
}

fn execute_command(app: &mut App) {
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
            let nodes = app.selected_nodes.clone();
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
            if parts.len() < 2 {
                app.set_error("Usage: write <filename>.yaml");
                return;
            }
            let filename = parts[1];
            app.set_error(&format!("Write to {} not implemented yet", filename));
        }

        Some(&"load") => {
            if parts.len() < 2 {
                app.set_error("Usage: load <filename>.scad");
                return;
            }
            let filename = parts[1];
            app.set_error(&format!("Load from {} not implemented yet", filename));
        }

        Some(&"export") => {
            if parts.len() < 2 {
                app.set_error("Usage: export <filename>.scad");
                return;
            }
            let filename = parts[1];
            app.set_error(&format!("Export to {} not implemented yet", filename));
        }

        Some(&"edit") => {
            if parts.len() < 2 {
                app.set_error("Usage: edit <filename>.yaml");
                return;
            }
            let filename = parts[1];
            app.set_error(&format!("Edit {} not implemented yet", filename));
        }

        Some(&"yank") => {
            app.set_error("Yank command not implemented yet");
        }

        Some(&"paste") => {
            app.set_error("Paste command not implemented yet");
        }

        Some(&"remove") => {
            app.set_error("Remove command not implemented yet");
        }

        Some(&"replace") => {
            if parts.len() < 2 {
                app.set_error("Usage: replace <module_name>");
                return;
            }
            app.set_error("Replace command not implemented yet");
        }

        Some(&"help") => {
            let help_text = "OpenSCAD TUI - Keybindings\n\
                \n\
                Normal mode:\n\
                  j/k - navigate down/up\n\
                  h/l - collapse/expand\n\
                  v - select node\n\
                  i - insert after\n\
                  a - insert before\n\
                  dd/D - delete\n\
                  u - undo\n\
                  r - redo\n\
                  : - command mode\n\
                  q - quit\n\
                \n\
                Commands: insert, delete, union, export";
            app.set_error(help_text);
        }

        _ => {
            app.set_error(&format!("Unknown command: {}", cmd));
        }
    }
}
