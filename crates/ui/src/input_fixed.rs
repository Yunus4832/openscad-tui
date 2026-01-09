//! Input handling module

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::App;
use crate::commands;

pub fn handle_key(key: KeyEvent, app: &mut App) {
    match key.code {
        KeyCode::Char(c) => {
            handle_char(c, key.modifiers, app);
        }
        KeyCode::Up => {
            app.tree_state.borrow_mut().key_up();
            app.clear_error();
        }
        KeyCode::Down => {
            app.tree_state.borrow_mut().key_down();
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
        KeyCode::Enter => {
            if app.command_mode {
                handle_command(app);
            }
        }
        KeyCode::Esc => {
            if app.command_mode {
                app.toggle_command_mode();
            } else {
                app.selected_nodes.clear();
            }
            app.clear_error();
        }
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }
        _ => {}
    }
}

fn handle_char(c: char, modifiers: KeyModifiers, app: &mut App) {
    if app.command_mode {
        match c {
            '\n' => {
                // Enter already handled above
            }
            _ => {
                app.input_buffer.push(c);
            }
        }
        return;
    }

    match c {
        'j' => {
            app.tree_state.borrow_mut().key_down();
            app.clear_error();
        }
        'k' => {
            app.tree_state.borrow_mut().key_up();
            app.clear_error();
        }
        'h' => {
            app.tree_state.borrow_mut().key_left();
            app.clear_error();
        }
        'l' => {
            app.tree_state.borrow_mut().key_right();
            app.clear_error();
        }

        // Insert
        'i' => {
            app.toggle_command_mode();
            app.input_buffer.push_str("insert ");
        }
        'a' => {
            app.toggle_command_mode();
            app.input_buffer.push_str("insert-after ");
        }

        // Delete
        'd' => {
            if modifiers.contains(KeyModifiers::SHIFT) {
                // Delete command (uppercase D)
                // Get the LEAF node (last element of path)
                let node_id = app.tree_state.borrow().selected().last().cloned();
                if let Some(node_id) = node_id {
                    app.push_undo();
                    if let Err(e) = commands::cmd_delete(app, &node_id) {
                        app.set_error(&e.to_string());
                    }
                }
            }
        }

        // Select
        'v' => {
            // Get the LEAF node (last element of path)
            let node_id = app.tree_state.borrow().selected().last().cloned();
            if let Some(node_id) = node_id {
                if app.selected_nodes.contains(&node_id) {
                    app.selected_nodes.retain(|id| id != &node_id);
                } else {
                    app.selected_nodes.push(node_id);
                }
            }
        }

        // Yank (copy)
        'y' => {
            let selected_exists = !app.tree_state.borrow().selected().is_empty();
            if selected_exists {
                let node_id = app.tree_state.borrow().selected().last().cloned();
                if let Some(node_id) = node_id {
                    if let Some(node) = app.ast.find_node_by_id(&node_id).cloned() {
                        app.clipboard = Some(node);
                        app.set_error(&format!("Copied: {}", node_id));
                    }
                }
            }
        }

        // Paste
        'p' => {
            if let Some(node) = app.clipboard.clone() {
                app.push_undo();
                if let Err(e) = app.ast.add_module(node) {
                    app.set_error(&e.to_string());
                } else {
                    app.clear_error();
                }
            } else {
                app.set_error("Nothing to paste");
            }
        }

        // Command mode
        ':' => {
            app.toggle_command_mode();
            app.clear_error();
        }

        // Undo
        'u' => {
            app.pop_undo();
            app.clear_error();
        }

        // Quit
        'q' => {
            std::process::exit(0);
        }

        _ => {}
    }
}

fn handle_command(app: &mut App) {
    let cmd = app.input_buffer.trim();
    app.input_buffer.clear();
    app.toggle_command_mode();

    if cmd.is_empty() {
        return;
    }

    let parts: Vec<&str> = cmd.split_whitespace().collect();
    match parts.get(0).map(|s| *s) {
        Some("insert") => {
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

        "delete" => {
            // Get the LEAF node (last element of path)
            let node_id = app.tree_state.borrow().selected().last().cloned();
            if let Some(node_id) = node_id {
                app.push_undo();
                if let Err(e) = commands::cmd_delete(app, &node_id) {
                    app.set_error(&e.to_string());
                }
            } else {
                app.set_error("No node selected");
            }
        }

        "union" | "difference" | "intersection" => {
            if app.selected_nodes.is_empty() {
                app.set_error("No nodes selected for boolean operation");
                return;
            }
            app.push_undo();
            let nodes = app.selected_nodes.clone();
            match commands::cmd_boolean_op(app, parts[0], &nodes) {
                Ok(_) => {
                    app.selected_nodes.clear();
                    app.clear_error();
                }
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        "write" => {
            if parts.len() < 2 {
                app.set_error("Usage: write <filename>");
                return;
            }
            let filename = parts[1];
            app.push_undo();
            match app.save_to_yaml(filename) {
                Ok(_) => app.clear_error(),
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        "load" => {
            if parts.len() < 2 {
                app.set_error("Usage: load <filename>");
                return;
            }
            let filename = parts[1];
            match app.load_from_yaml(filename) {
                Ok(_) => app.clear_error(),
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        "export" => {
            if parts.len() < 2 {
                app.set_error("Usage: export <filename>");
                return;
            }
            let filename = parts[1];
            match app.export_scad(filename) {
                Ok(_) => app.clear_error(),
                Err(e) => app.set_error(&e.to_string()),
            }
        }

        "help" => {
            app.set_error("OpenSCAD TUI - Commands: insert, delete, union, difference, write, load, export, help, quit");
        }

        _ => {
            app.set_error(&format!("Unknown command: {}", cmd));
        }
    }
}
