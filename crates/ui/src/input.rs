//! Input handling module - Two modes: Normal and Command
//!
//! Normal mode: Quick keybindings for common operations (i/j/k/h/l/v)
//! Command mode: Free text input for complex commands with parameter input

use crate::app::{
    App, CandidateType, CompletionCandidate, CompletionContext, DefinitionCompletionKind,
    ExpressionCompletionKind, InputMode, PendingModuleAction,
};
use crate::command_registry::{CommandLine, CommandType, CompletionSource};
use crate::commands;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use openscad_core::{Argument, ModuleNode};
use ratatui::layout::Position;
use std::fs;
use std::path::{Component, Path, PathBuf};

pub fn handle_key(key: KeyEvent, app: &mut App) {
    match app.input_mode {
        InputMode::Command => handle_command_input(key, app),
        InputMode::ModuleEnterParams => handle_module_params_input(key, app),
        InputMode::Help => handle_help_input(key, app),
        InputMode::Normal => match app.screen {
            crate::app::Screen::Editor => handle_normal_input(key, app),
            crate::app::Screen::ModelPreview => handle_model_key(key, app),
            crate::app::Screen::Assembly => handle_assembly_key(key, app),
        },
    }
}

pub fn handle_mouse(event: MouseEvent, app: &mut App) {
    if app.input_mode == InputMode::Help {
        match event.kind {
            MouseEventKind::ScrollUp => {
                app.help_scroll_offset = app.help_scroll_offset.saturating_sub(3)
            }
            MouseEventKind::ScrollDown => {
                app.help_scroll_offset =
                    (app.help_scroll_offset + 3).min(app.help_scroll_offset_max)
            }
            _ => {}
        }
        return;
    }
    match app.screen {
        crate::app::Screen::Editor => handle_editor_mouse(event, app),
        crate::app::Screen::ModelPreview => handle_model_mouse(event, app),
        crate::app::Screen::Assembly => handle_assembly_mouse(event, app),
    }
}

fn handle_editor_mouse(event: MouseEvent, app: &mut App) {
    let position = Position::new(event.column, event.row);
    if app.ui_regions.tree.contains(position) {
        handle_tree_mouse(event, app, position);
        return;
    }

    if app.ui_regions.preview.contains(position) {
        handle_preview_mouse(event, app);
    }
}

fn handle_model_mouse(event: MouseEvent, app: &mut App) {
    let position = Position::new(event.column, event.row);
    if matches!(event.kind, MouseEventKind::Up(_)) {
        if app.mouse_drag.is_some() {
            update_model_drag(app, event.column, event.row);
        }
        app.mouse_drag = None;
        return;
    }
    if app.mouse_drag.is_some() && matches!(event.kind, MouseEventKind::Drag(_)) {
        handle_preview_mouse(event, app);
        return;
    }
    if event.kind == MouseEventKind::Down(MouseButton::Left) {
        if let Some(command) = app
            .ui_regions
            .camera_buttons
            .iter()
            .find(|button| button.area.contains(position))
            .map(|button| button.command.clone())
        {
            execute_shortcut(app, &command);
            return;
        }
    }
    if app.ui_regions.preview.contains(position) {
        handle_preview_mouse(event, app);
    }
}

fn handle_assembly_mouse(event: MouseEvent, app: &mut App) {
    let position = Position::new(event.column, event.row);
    if app.ui_regions.tree.contains(position) {
        if event.kind == MouseEventKind::ScrollUp {
            execute_shortcut(app, "assembly select prev");
            return;
        }
        if event.kind == MouseEventKind::ScrollDown {
            execute_shortcut(app, "assembly select next");
            return;
        }
        if event.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        let row = event
            .row
            .saturating_sub(app.ui_regions.tree.y.saturating_add(1)) as usize
            + app.assembly_scroll_offset;
        let part_id = app.active_assembly.as_deref().and_then(|active| {
            app.assemblies
                .iter()
                .find(|assembly| assembly.id == active || assembly.name == active)
                .and_then(|assembly| {
                    assembly
                        .hierarchy_rows()
                        .get(row)
                        .map(|(index, _)| &assembly.parts[*index])
                })
                .map(|part| part.id.clone())
        });
        if let Some(part_id) = part_id {
            execute_shortcut(app, &format!("assembly select {part_id}"));
        }
        return;
    }
    handle_model_mouse(event, app);
}

fn handle_tree_mouse(event: MouseEvent, app: &mut App, position: Position) {
    match event.kind {
        MouseEventKind::ScrollUp => {
            app.tree_state.borrow_mut().scroll_up(3);
        }
        MouseEventKind::ScrollDown => {
            app.tree_state.borrow_mut().scroll_down(3);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if event.modifiers.contains(KeyModifiers::CONTROL) {
                let path = app
                    .tree_state
                    .borrow()
                    .rendered_at(position)
                    .map(<[String]>::to_vec);
                if let Some(path) = path {
                    app.tree_state.borrow_mut().select(path.clone());
                    if let Some(node_id) = path.last().filter(|id| !id.starts_with("__")) {
                        if app.selected_nodes.contains(node_id) {
                            app.selected_nodes.retain(|selected| selected != node_id);
                        } else {
                            app.selected_nodes.push(node_id.clone());
                        }
                    }
                }
            } else {
                app.tree_state.borrow_mut().click_at(position);
            }
            app.update_navigation_status();
        }
        _ => {}
    }
}

fn handle_preview_mouse(event: MouseEvent, app: &mut App) {
    use crate::app::MouseDrag;
    if app.screen == crate::app::Screen::Editor {
        let line_count = app.ast.to_scad().lines().count();
        match event.kind {
            MouseEventKind::ScrollUp => app.preview_offset = app.preview_offset.saturating_sub(3),
            MouseEventKind::ScrollDown => {
                app.preview_offset = (app.preview_offset + 3).min(line_count.saturating_sub(1))
            }
            _ => {}
        }
        return;
    }

    match event.kind {
        MouseEventKind::Down(MouseButton::Left | MouseButton::Right) => {
            app.mouse_drag = Some(MouseDrag {
                last_column: event.column,
                last_row: event.row,
                pan: event.kind == MouseEventKind::Down(MouseButton::Right)
                    || event.modifiers.contains(KeyModifiers::SHIFT),
            });
        }
        MouseEventKind::Drag(MouseButton::Left | MouseButton::Right) => {
            update_model_drag(app, event.column, event.row);
        }
        MouseEventKind::ScrollUp => {
            execute_shortcut(app, "camera zoom 0.85");
        }
        MouseEventKind::ScrollDown => {
            execute_shortcut(app, "camera zoom 1.15");
        }
        _ => {}
    }
}

fn update_model_drag(app: &mut App, column: u16, row: u16) {
    let Some(mut drag) = app.mouse_drag else {
        return;
    };
    let delta_x = i32::from(column) - i32::from(drag.last_column);
    let delta_y = i32::from(row) - i32::from(drag.last_row);
    if delta_x == 0 && delta_y == 0 {
        return;
    }

    drag.last_column = column;
    drag.last_row = row;
    app.mouse_drag = Some(drag);
    let command = if drag.pan {
        let width = f32::from(app.ui_regions.preview.width.max(1));
        let height = f32::from(app.ui_regions.preview.height.max(1));
        format!(
            "camera pan {} {}",
            -(delta_x as f32) / width,
            (delta_y as f32) / height
        )
    } else {
        let (yaw, pitch) = mouse_orbit_delta(delta_x, delta_y);
        format!("camera orbit {yaw} {pitch}")
    };
    execute_shortcut(app, &command);
}

fn mouse_orbit_delta(delta_x: i32, delta_y: i32) -> (f32, f32) {
    const DEGREES_PER_CELL: f32 = 3.2;
    // Orbit moves the camera, while dragging conventionally moves the object
    // under the pointer, so mouse deltas use the opposite camera direction.
    (
        -(delta_x as f32) * DEGREES_PER_CELL,
        delta_y as f32 * DEGREES_PER_CELL,
    )
}

/// Normal mode: Quick keybindings
fn handle_normal_input(key: KeyEvent, app: &mut App) {
    match key.code {
        // i - insert module (mapped to :insert command)
        KeyCode::Char('i') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("insert ");
        }

        // I - insert a module before the current node
        KeyCode::Char('I') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("insert-before ");
        }

        // a - edit arguments on selected nodes or the current node
        KeyCode::Char('a') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("set ");
        }

        // A - remove an explicitly set argument
        KeyCode::Char('A') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("unset ");
        }

        // t - translate
        KeyCode::Char('t') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("translate ");
        }

        // s - scale
        KeyCode::Char('s') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("scale ");
        }

        // n - create a project-owned SCAD source
        KeyCode::Char('n') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("source new ");
        }

        // Navigation: j (next), k (prev), h (back/collapse), l (forward/expand)
        KeyCode::Char('j') | KeyCode::Down => {
            execute_shortcut(app, "next");
        }
        KeyCode::Char('k') | KeyCode::Up => {
            execute_shortcut(app, "prev");
        }
        KeyCode::Char('h') | KeyCode::Left => {
            execute_shortcut(app, "collapse");
        }
        KeyCode::Char('l') | KeyCode::Right => {
            execute_shortcut(app, "expand");
        }

        // v - select/toggle node
        KeyCode::Char('v') => {
            execute_shortcut(app, "select");
        }

        KeyCode::Char(' ') => {
            execute_shortcut(app, "visibility toggle");
        }

        // Vim-style structural editing
        KeyCode::Char('y') => {
            execute_shortcut(app, "yank");
        }
        KeyCode::Char('p') => {
            execute_shortcut(app, "paste");
        }
        KeyCode::Char('x') => {
            execute_shortcut(app, "remove");
        }
        KeyCode::Char('c') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("replace ");
        }

        // u - undo
        KeyCode::Char('u') => {
            execute_shortcut(app, "undo");
        }

        // r - rotate (Ctrl+r for redo)
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            execute_shortcut(app, "redo");
        }
        KeyCode::Char('r') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("rotate ");
        }

        // d - delete node
        KeyCode::Char('d') => {
            execute_shortcut(app, "delete");
        }

        // w - save the current project package
        KeyCode::Char('w') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("project save");
        }

        // e - import an OpenSCAD source into the current project
        KeyCode::Char('e') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("source import ");
        }

        // o - open a .scadtui project
        KeyCode::Char('o') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("project open ");
        }

        // L - attach a SCAD source library
        KeyCode::Char('L') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("library load ");
        }

        // : - enter command mode
        KeyCode::Char(':') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.clear();
        }

        // Enter - toggle expand/collapse node
        KeyCode::Enter => {
            let selected = app.tree_state.borrow().selected().last().cloned();
            let project_source = selected
                .as_deref()
                .and_then(|id| id.strip_prefix("__project_source_"))
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| app.ast.embedded_sources.get(index))
                .map(|source| (source.virtual_path.clone(), source.editable));
            match project_source {
                Some((path, true)) => {
                    execute_shortcut(
                        app,
                        &format!("source switch {}", quote_command_argument(&path)),
                    );
                }
                Some((path, false)) => app.set_error(&format!("Source '{path}' is read-only")),
                None => execute_shortcut(app, "toggle"),
            }
        }

        // q - quit
        KeyCode::Char('q') => {
            execute_shortcut(app, "quit");
        }

        // Ctrl+C to quit
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            execute_shortcut(app, "quit");
        }

        // ? - show help
        KeyCode::Char('?') => {
            execute_shortcut(app, "help");
        }

        // P - switch between source and model preview
        KeyCode::Char('P') => execute_shortcut(app, "model toggle"),

        // R - always render the active buffer and show the new model preview
        KeyCode::Char('R') => execute_shortcut(app, "model render"),

        _ => {}
    }
}

fn handle_model_key(key: KeyEvent, app: &mut App) {
    if key.code == KeyCode::Char(':') {
        app.input_mode = InputMode::Command;
        app.input_buffer.clear();
        app.clear_error();
        return;
    }
    if let Some(command) = model_key_command(key) {
        execute_shortcut(app, command);
    }
}

fn handle_assembly_key(key: KeyEvent, app: &mut App) {
    match key.code {
        KeyCode::Char(':') => begin_assembly_command(app, ""),
        KeyCode::Char('n') => begin_assembly_command(app, "assembly new "),
        KeyCode::Char('a') => begin_assembly_command(app, "assembly add "),
        KeyCode::Char('t') => prefill_assembly_transform(app, "translate"),
        KeyCode::Char('r') => prefill_assembly_transform(app, "rotate"),
        KeyCode::Char('s') => prefill_assembly_transform(app, "scale"),
        KeyCode::Char('o') => prefill_assembly_transform(app, "pivot"),
        KeyCode::Char('g') => prefill_assembly_parent(app),
        KeyCode::Char('e') => begin_assembly_command(app, "assembly export "),
        _ => {}
    }
    if app.input_mode == InputMode::Command {
        return;
    }
    let command: Option<String> = match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('P') => Some("assembly close".into()),
        KeyCode::Char('j') => Some("assembly select next".into()),
        KeyCode::Char('k') => Some("assembly select prev".into()),
        KeyCode::Char('R') => Some("assembly render".into()),
        KeyCode::Char('v') => Some("assembly visibility toggle".into()),
        KeyCode::Char('y') => Some("assembly copy".into()),
        KeyCode::Char('p') => Some("assembly paste".into()),
        KeyCode::Char('d') => Some("assembly remove".into()),
        _ => model_key_command(key).map(str::to_string),
    };
    if let Some(command) = command {
        execute_shortcut(app, &command);
    }
}

fn begin_assembly_command(app: &mut App, command: &str) {
    app.input_mode = InputMode::Command;
    app.input_buffer.set_content(command);
    app.completion_active = false;
    app.completion_candidates.clear();
    app.clear_error();
}

fn selected_assembly_part(app: &App) -> Option<&openscad_assembly::PartInstance> {
    let active = app.active_assembly.as_deref()?;
    let selected = app.selected_assembly_part.as_deref()?;
    app.assemblies
        .iter()
        .find(|assembly| assembly.id == active || assembly.name == active)?
        .part(selected)
}

fn format_assembly_values(values: [f32; 3]) -> String {
    format!("{} {} {}", values[0], values[1], values[2])
}

fn prefill_assembly_transform(app: &mut App, operation: &str) {
    let Some(part) = selected_assembly_part(app) else {
        app.set_error("No assembly part is selected");
        return;
    };
    let values = match operation {
        "translate" => part.transform.translation,
        "rotate" => part.transform.rotation_degrees,
        "scale" => part.transform.scale,
        "pivot" => part.transform.pivot,
        _ => unreachable!(),
    };
    let command = format!(
        "assembly {operation} {} {}",
        part.id,
        format_assembly_values(values)
    );
    begin_assembly_command(app, &command);
}

fn prefill_assembly_parent(app: &mut App) {
    let Some(part) = selected_assembly_part(app) else {
        app.set_error("No assembly part is selected");
        return;
    };
    let parent = part.parent.as_deref().unwrap_or("root");
    let command = format!("assembly parent {} {parent}", part.id);
    begin_assembly_command(app, &command);
}

fn model_key_command(key: KeyEvent) -> Option<&'static str> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('P') => Some("model close"),
        KeyCode::Char('R') => Some("model render"),
        KeyCode::Char('h') => Some("camera orbit -5 0"),
        KeyCode::Char('l') => Some("camera orbit 5 0"),
        KeyCode::Char('j') => Some("camera orbit 0 -5"),
        KeyCode::Char('k') => Some("camera orbit 0 5"),
        KeyCode::Left => Some("camera pan -0.05 0"),
        KeyCode::Right => Some("camera pan 0.05 0"),
        KeyCode::Up => Some("camera pan 0 0.05"),
        KeyCode::Down => Some("camera pan 0 -0.05"),
        KeyCode::Char('+') | KeyCode::Char('=') => Some("camera zoom 0.85"),
        KeyCode::Char('-') => Some("camera zoom 1.15"),
        KeyCode::Char('f') => Some("camera fit"),
        KeyCode::Char('p') => Some("camera projection toggle"),
        KeyCode::Char('x') => Some("display axes toggle"),
        KeyCode::Char(' ') => Some("camera auto-rotate toggle"),
        KeyCode::Char('1') => Some("camera view front"),
        KeyCode::Char('2') => Some("camera view back"),
        KeyCode::Char('3') => Some("camera view left"),
        KeyCode::Char('4') => Some("camera view right"),
        KeyCode::Char('5') => Some("camera view top"),
        KeyCode::Char('6') => Some("camera view bottom"),
        KeyCode::Char('7') => Some("camera view iso"),
        KeyCode::Char('?') => Some("help"),
        _ => None,
    }
}

/// Handle input in command mode - text input with echo
fn handle_command_input(key: KeyEvent, app: &mut App) {
    app.clamp_cursor();

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

        // Regular character input - insert at cursor position
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.completion_active {
                // User started typing, exit completion mode
                app.completion_active = false;
                app.completion_candidates.clear();
            }
            app.input_buffer.insert_char(c);
        }

        // Ctrl+P to get previous command from history (vim-style)
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let draft = app.input_buffer.content().to_string();
            if let Some(prev_cmd) = app.get_previous_command(&draft) {
                app.input_buffer.set_content(&prev_cmd);
            }
        }

        // Ctrl+N to get next command from history (vim-style)
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(next_cmd) = app.get_next_command() {
                app.input_buffer.set_content(&next_cmd);
            } else {
                // Clear input buffer, back to blank input state
                app.input_buffer.clear();
            }
        }

        // Backspace to delete character before cursor
        KeyCode::Backspace => {
            if app.completion_active {
                // User started editing, exit completion mode
                app.completion_active = false;
                app.completion_candidates.clear();
            }
            app.input_buffer.delete_before_cursor();
        }

        // Delete to delete character at cursor
        KeyCode::Delete => {
            if app.completion_active {
                app.completion_active = false;
                app.completion_candidates.clear();
            }
            app.input_buffer.delete_at_cursor();
        }

        // Cursor movement
        KeyCode::Left => {
            app.input_buffer.move_left();
        }
        KeyCode::Right => {
            app.input_buffer.move_right();
        }
        KeyCode::Home => {
            app.input_buffer.move_to_start();
        }
        KeyCode::End => {
            app.input_buffer.move_to_end();
        }

        // Up arrow to get previous command from history
        KeyCode::Up => {
            let draft = app.input_buffer.content().to_string();
            if let Some(prev_cmd) = app.get_previous_command(&draft) {
                app.input_buffer.set_content(&prev_cmd);
            }
        }

        // Down arrow to get next command from history
        KeyCode::Down => {
            if let Some(next_cmd) = app.get_next_command() {
                app.input_buffer.set_content(&next_cmd);
            } else {
                // Clear input buffer, back to blank input state
                app.input_buffer.clear();
            }
        }

        // Ctrl+P to get previous command from history (vim-style)
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let draft = app.input_buffer.content().to_string();
            if let Some(prev_cmd) = app.get_previous_command(&draft) {
                app.input_buffer.set_content(&prev_cmd);
            }
        }

        // Ctrl+N to get next command from history (vim-style)
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(next_cmd) = app.get_next_command() {
                app.input_buffer.set_content(&next_cmd);
            } else {
                // Clear input buffer, back to blank input state
                app.input_buffer.clear();
            }
        }

        KeyCode::Enter => {
            if app.completion_active {
                apply_completion(app);
            } else {
                let cmd = app.input_buffer.content().to_string();
                execute_user_command(app, &cmd);
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
fn handle_module_params_input(key: KeyEvent, app: &mut App) {
    app.clamp_cursor();

    match key.code {
        KeyCode::Char(c) => {
            if app.completion_active {
                app.completion_active = false;
                app.completion_candidates.clear();
            }
            app.input_buffer.insert_char(c);
        }
        KeyCode::Backspace => {
            if app.completion_active {
                app.completion_active = false;
                app.completion_candidates.clear();
            }
            app.input_buffer.delete_before_cursor();
        }
        KeyCode::Delete => {
            if app.completion_active {
                app.completion_active = false;
                app.completion_candidates.clear();
            }
            app.input_buffer.delete_at_cursor();
        }
        KeyCode::Left => {
            app.input_buffer.move_left();
        }
        KeyCode::Right => {
            app.input_buffer.move_right();
        }
        KeyCode::Home => {
            app.input_buffer.move_to_start();
        }
        KeyCode::End => {
            app.input_buffer.move_to_end();
        }
        KeyCode::Tab => {
            handle_tab_completion(app);
        }
        KeyCode::Enter => {
            if app.completion_active {
                apply_completion(app);
            } else {
                let params = app.input_buffer.content().trim().to_string();
                if let Err(error) = commands::commit_pending_module_action(app, &params) {
                    app.set_error(&error.to_string());
                }
                app.input_mode = InputMode::Normal;
                app.input_buffer.clear();
                app.pending_module_action = None;
                app.pending_module_name = None;
            }
        }
        KeyCode::Esc => {
            let action = match app.pending_module_action {
                Some(PendingModuleAction::Insert) => "Insert",
                Some(PendingModuleAction::InsertBefore) => "Insert before",
                Some(PendingModuleAction::Replace { .. }) => "Replace",
                None => "Module action",
            };
            app.input_mode = InputMode::Normal;
            app.input_buffer.clear();
            app.pending_module_action = None;
            app.pending_module_name = None;
            app.set_info(&format!("{} cancelled", action));
        }
        _ => {}
    }
}

/// Handle help modal input
fn handle_help_input(key: KeyEvent, app: &mut App) {
    match key.code {
        // Close help modal
        KeyCode::Esc | KeyCode::Char('q') => {
            app.input_mode = InputMode::Normal;
        }
        // Scroll up
        KeyCode::Up | KeyCode::Char('k') => {
            app.help_scroll_offset = app.help_scroll_offset.saturating_sub(1);
        }
        // Scroll down
        KeyCode::Down | KeyCode::Char('j') => {
            app.help_scroll_offset = app
                .help_scroll_offset
                .saturating_add(1)
                .min(app.help_scroll_offset_max);
        }
        // Page up
        KeyCode::PageUp | KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.help_scroll_offset = app.help_scroll_offset.saturating_sub(10);
        }
        // Page down
        KeyCode::PageDown | KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.help_scroll_offset = app
                .help_scroll_offset
                .saturating_add(10)
                .min(app.help_scroll_offset_max);
        }
        // Home key - go to top
        KeyCode::Home => {
            app.help_scroll_offset = 0;
        }
        // End key - go to bottom
        KeyCode::End => {
            app.help_scroll_offset = app.help_scroll_offset_max;
        }
        _ => {}
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommandOrigin {
    UserInput,
    Shortcut,
}

/// Execute user-entered commands and shortcuts through the same command registry.
fn execute_command_registry(app: &mut App, cmd: &str, origin: CommandOrigin) -> bool {
    if origin == CommandOrigin::UserInput {
        app.input_buffer.clear();
    }

    if cmd.is_empty() {
        return true;
    }

    let command_line = CommandLine::parse(cmd);
    if command_line.unterminated_quote {
        app.set_error("Unterminated quote in command");
        return true;
    }
    let parts = command_line.values();
    if parts.is_empty() {
        return true;
    }

    // Resolve the longest registered path, such as `model view` before `model`.
    if let Some(resolved) = app.command_registry.resolve(&parts) {
        let cmd_def = resolved.definition;
        let args = &parts[resolved.consumed_tokens..];
        let cmd_name = cmd_def.name.clone();
        // Validate arguments
        let handler = cmd_def.handler;
        let (min_args, max_args) = cmd_def.argument_bounds();
        let change_ast = cmd_def.change_ast;
        let write_to_history = cmd_def.write_to_history;

        if write_to_history && origin == CommandOrigin::UserInput {
            app.add_to_history(cmd);
        }

        if args.len() < min_args {
            app.set_error(&format!(
                "{} requires at least {} arguments",
                cmd_name, min_args
            ));
            return true;
        }

        if let Some(max) = max_args {
            if args.len() > max {
                app.set_error(&format!("{} accepts at most {} arguments", cmd_name, max));
                return true;
            }
        }

        // Clear stale feedback before execution; handlers may replace it with a useful success
        // message that must remain visible after the command returns.
        app.message = None;
        // Execute the command
        match handler(app, args) {
            Ok(_) => {
                if change_ast {
                    app.mark_dirty();
                }
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
        parts.join(" ")
    ));
    // Add unknown commands typed by the user to history so they can recall and edit them.
    if origin == CommandOrigin::UserInput {
        app.add_to_history(cmd);
    }
    true
}

fn execute_user_command(app: &mut App, cmd: &str) {
    execute_command_registry(app, cmd, CommandOrigin::UserInput);

    if app.input_mode == InputMode::Command {
        app.input_mode = InputMode::Normal;
    }
}

fn execute_shortcut(app: &mut App, command: &str) {
    execute_command_registry(app, command, CommandOrigin::Shortcut);
}

fn quote_command_argument(value: &str) -> String {
    if value
        .chars()
        .any(|character| character.is_whitespace() || matches!(character, '"' | '\\' | '\''))
    {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

/// Handle Tab key for autocompletion
fn handle_tab_completion(app: &mut App) {
    if !app.completion_active {
        let (candidates, analysis) = generate_completions(app.input_buffer.content(), app);
        if candidates.is_empty() {
            return;
        }

        app.completion_context = analysis.context;
        app.completion_replacement_range = analysis.replacement_range;
        app.completion_index = 0;
        app.completion_active = true;
        app.completion_candidates = candidates;
        preview_completion(app);

        // Check for single match
        if app.completion_candidates.len() == 1 {
            apply_completion(app);
        }
    } else {
        // Already in completion mode: cycle to next candidate
        app.completion_index = (app.completion_index + 1) % app.completion_candidates.len();
        preview_completion(app);
    }
}

/// Parse parameters from a string, returning parameter names that have been entered
/// Parameters are separated by commas, not spaces
fn parse_parameter_names(param_str: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut current = String::new();
    let mut in_list = 0;
    let mut in_function = 0;

    for ch in param_str.chars() {
        match ch {
            '[' => in_list += 1,
            ']' if in_list > 0 => in_list -= 1,
            '(' => in_function += 1,
            ')' if in_function > 0 => in_function -= 1,
            ',' if in_list == 0 && in_function == 0 => {
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

/// 分析输入字符串，确定当前补全上下文
fn analyze_input_context(input: &str, app: &App) -> CompletionContext {
    let trimmed = input.trim();

    if app.input_mode == InputMode::ModuleEnterParams {
        let cmd_type = match app.pending_module_action {
            Some(PendingModuleAction::Replace { .. }) => CommandType::Replace,
            _ => CommandType::Module,
        };
        if let Some(ref module_name) = app.pending_module_name {
            return analyze_param_context(trimmed, module_name, cmd_type);
        }
        return CompletionContext::ModuleParam {
            cmd_type,
            module_name: String::new(),
        };
    }

    // 空输入或只有空白字符：命令补全
    if trimmed.is_empty() {
        return CompletionContext::Command;
    }

    let command_line = CommandLine::parse(input);
    let parts = command_line.values();

    if parts.is_empty() {
        return CompletionContext::Command;
    }

    let (path_prefix, partial_path) = if command_line.trailing_separator {
        (parts.as_slice(), "")
    } else {
        (&parts[..parts.len() - 1], parts[parts.len() - 1])
    };
    let path_candidates = app.command_registry.child_names(path_prefix);
    if !path_candidates.is_empty() {
        return CompletionContext::CommandPath {
            candidates: path_candidates
                .into_iter()
                .filter(|candidate| candidate.starts_with(partial_path))
                .collect(),
        };
    }

    let Some(resolved) = app.command_registry.resolve(&parts) else {
        return CompletionContext::Command;
    };
    let cmd_def = resolved.definition;
    if !cmd_def.arguments.is_empty() {
        return declarative_argument_context(
            input,
            &command_line,
            resolved.consumed_tokens,
            &cmd_def.arguments,
            app,
        );
    }

    // Legacy OpenSCAD-aware completion commands remain single-token leaves while their
    // specialized providers are migrated behind declarative argument specifications.
    let command = parts[0];

    // 使用命令注册表查找命令类型
    if resolved.consumed_tokens == 1 {
        match &cmd_def.cmd_type {
            CommandType::Module => {
                // insert 命令的处理逻辑 (insert <module> [params])
                if parts.len() == 1 {
                    if input.ends_with(' ') {
                        CompletionContext::Module
                    } else {
                        CompletionContext::Command
                    }
                } else {
                    // 第二个参数应为模块名
                    let module_part = parts[1];

                    if parts.len() == 2 {
                        // 检查输入是否以空格结尾：如果是，则进入模块参数补全上下文
                        if input.ends_with(' ') {
                            CompletionContext::ModuleParam {
                                cmd_type: CommandType::Module,
                                module_name: module_part.to_string(),
                            }
                        } else {
                            CompletionContext::Module
                        }
                    } else {
                        // 有参数部分
                        let param_str = parts[2..].join(" ");
                        analyze_param_context(&param_str, module_part, CommandType::Module)
                    }
                }
            }
            CommandType::Param => {
                // 参数命令的处理逻辑 (<transform_cmd> [params])
                if parts.len() == 1 {
                    // 只有命令名
                    if input.ends_with(' ') {
                        // 命令后有空格，进入此命令的参数补全
                        CompletionContext::ModuleParam {
                            cmd_type: CommandType::Param,
                            module_name: command.to_string(),
                        }
                    } else {
                        // 只输入了命令，还在命令补全阶段
                        CompletionContext::Command
                    }
                } else {
                    // 命令后有参数，将所有参数作为一个整体处理
                    let param_str = parts[1..].join(" ");
                    analyze_param_context(&param_str, command, CommandType::Param)
                }
            }
            CommandType::NoArg => {
                // 无参数命令：无需补全
                CompletionContext::Command
            }
            CommandType::FunctionDefinition => {
                if input.contains('=') {
                    CompletionContext::ExpressionValue {
                        kind: ExpressionCompletionKind::FunctionBody,
                        local_identifiers: function_definition_parameters(input),
                    }
                } else if input.find(char::is_whitespace).is_some()
                    && !input
                        .split_once(char::is_whitespace)
                        .is_some_and(|(_, definition)| definition.contains('('))
                {
                    CompletionContext::DefinitionName {
                        kind: DefinitionCompletionKind::Function,
                    }
                } else {
                    CompletionContext::Command
                }
            }
            CommandType::ModuleDefinition => CompletionContext::Command,
            CommandType::GlobalDefinition => {
                if input.contains('=') {
                    CompletionContext::ExpressionValue {
                        kind: ExpressionCompletionKind::GlobalValue,
                        local_identifiers: Vec::new(),
                    }
                } else if input.find(char::is_whitespace).is_some() {
                    CompletionContext::DefinitionName {
                        kind: DefinitionCompletionKind::Global,
                    }
                } else {
                    CompletionContext::Command
                }
            }
            CommandType::Replace => {
                if parts.len() == 1 {
                    if input.ends_with(' ') {
                        CompletionContext::Module
                    } else {
                        CompletionContext::Command
                    }
                } else if parts.len() == 2 {
                    if input.ends_with(' ') {
                        CompletionContext::ModuleParam {
                            cmd_type: CommandType::Replace,
                            module_name: parts[1].to_string(),
                        }
                    } else {
                        CompletionContext::Module
                    }
                } else {
                    analyze_param_context(&parts[2..].join(" "), parts[1], CommandType::Replace)
                }
            }
            CommandType::NodeParam => {
                if parts.len() == 1 {
                    if input.ends_with(' ') {
                        CompletionContext::NodeParam
                    } else {
                        CompletionContext::Command
                    }
                } else {
                    let argument_source = input
                        .find(char::is_whitespace)
                        .map(|index| input[index..].trim_start())
                        .unwrap_or("");
                    if let Some((parameter_name, _)) = argument_source.split_once('=') {
                        CompletionContext::NodeParamValue {
                            parameter_name: parameter_name.trim().to_string(),
                        }
                    } else {
                        CompletionContext::NodeParam
                    }
                }
            }
            CommandType::NodeParamUnset => {
                if parts.len() == 1 && !input.ends_with(' ') {
                    CompletionContext::Command
                } else {
                    CompletionContext::NodeParamUnset
                }
            }
            CommandType::Visibility => {
                literal_command_context(input, &parts, &["show", "hide", "toggle"], &[])
            }
        }
    } else {
        CompletionContext::Command
    }
}

fn declarative_argument_context(
    _input: &str,
    command_line: &CommandLine,
    consumed_tokens: usize,
    arguments: &[crate::command_registry::ArgumentSpec],
    app: &App,
) -> CompletionContext {
    let values = command_line.values();
    let entered = &values[consumed_tokens..];
    if entered.is_empty() && !command_line.trailing_separator {
        return CompletionContext::Command;
    }
    let argument_index = if command_line.trailing_separator {
        entered.len()
    } else {
        entered.len().saturating_sub(1)
    };
    let Some(argument) = arguments.get(argument_index).or_else(|| {
        arguments
            .last()
            .filter(|argument| argument.variadic && argument_index >= arguments.len())
    }) else {
        return CompletionContext::Command;
    };
    match &argument.completion {
        CompletionSource::None => CompletionContext::Command,
        CompletionSource::Literal(candidates) => CompletionContext::Literal {
            candidates: candidates.clone(),
        },
        CompletionSource::Path { extensions } => {
            let current_path = entered.get(argument_index).copied().unwrap_or("");
            file_completion_context(current_path, extensions)
        }
        CompletionSource::ProjectSource { editable_only } => CompletionContext::Literal {
            candidates: app
                .ast
                .embedded_sources
                .iter()
                .filter(|source| !editable_only || source.editable)
                .map(|source| source.virtual_path.clone())
                .collect(),
        },
        CompletionSource::LoadedLibrary => CompletionContext::Literal {
            candidates: loaded_library_names(app),
        },
        CompletionSource::LibraryRoot => CompletionContext::Literal {
            candidates: loaded_library_roots(app),
        },
        CompletionSource::Assembly => CompletionContext::Literal {
            candidates: app
                .assemblies
                .iter()
                .map(|assembly| assembly.name.clone())
                .collect(),
        },
        CompletionSource::AssemblyPart { literals } => CompletionContext::Literal {
            candidates: literals
                .iter()
                .cloned()
                .chain(
                    app.active_assembly
                        .as_deref()
                        .and_then(|active| {
                            app.assemblies
                                .iter()
                                .find(|assembly| assembly.id == active || assembly.name == active)
                        })
                        .into_iter()
                        .flat_map(|assembly| assembly.parts.iter().map(|part| part.id.clone())),
                )
                .collect(),
        },
        CompletionSource::CommandPath => {
            let path_arguments = if command_line.trailing_separator {
                entered
            } else {
                &entered[..entered.len().saturating_sub(1)]
            };
            CompletionContext::CommandPath {
                candidates: app.command_registry.child_names(path_arguments),
            }
        }
    }
}

fn file_completion_context(current_path: &str, extensions: &[String]) -> CompletionContext {
    let ends_with_separator = current_path.ends_with('/');
    let (base_dir, partial_name) = if current_path.contains('/') {
        let last_slash = current_path.rfind('/').unwrap();
        let base = &current_path[..last_slash + 1];
        let partial = &current_path[last_slash + 1..];
        let normalized_base = if base.starts_with('/') {
            base.to_string()
        } else if let Some(stripped) = base.strip_prefix("~/") {
            std::env::var("HOME")
                .map(|home_dir| format!("{home_dir}/{stripped}"))
                .unwrap_or_else(|_| base.to_string())
        } else if base == "./" || base.is_empty() {
            ".".to_string()
        } else {
            normalize_path(base)
        };
        (normalized_base, partial.to_string())
    } else if current_path == "~" {
        (
            std::env::var("HOME").unwrap_or_else(|_| ".".to_string()),
            String::new(),
        )
    } else {
        (".".to_string(), current_path.to_string())
    };
    CompletionContext::File {
        current_path: current_path.to_string(),
        base_dir,
        partial_name,
        ends_with_separator,
        extensions: extensions.to_vec(),
    }
}

fn loaded_library_names(app: &App) -> Vec<String> {
    let active = app.ast.active_source.as_deref();
    let sources = app
        .ast
        .embedded_sources
        .iter()
        .filter(|source| active != Some(source.virtual_path.as_str()))
        .collect::<Vec<_>>();
    sources
        .iter()
        .map(|source| {
            let filename = Path::new(&source.virtual_path)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or(&source.virtual_path);
            let duplicate_filename = sources.iter().any(|other| {
                other.virtual_path != source.virtual_path
                    && Path::new(&other.virtual_path)
                        .file_name()
                        .and_then(|value| value.to_str())
                        == Some(filename)
            });
            if duplicate_filename {
                source.virtual_path.clone()
            } else {
                filename.to_string()
            }
        })
        .collect()
}

fn loaded_library_roots(app: &App) -> Vec<String> {
    app.ast
        .embedded_sources
        .iter()
        .filter(|source| source.role == openscad_core::EmbeddedSourceRole::Library)
        .map(|source| source.virtual_path.clone())
        .collect()
}

fn literal_command_context(
    input: &str,
    parts: &[&str],
    first_level: &[&str],
    second_level: &[&str],
) -> CompletionContext {
    let candidates = if (parts.len() == 1 && input.ends_with(' '))
        || (parts.len() == 2 && !input.ends_with(' '))
    {
        first_level
    } else if (parts.len() == 2 && input.ends_with(' '))
        || (parts.len() == 3 && !input.ends_with(' '))
    {
        second_level
    } else {
        return CompletionContext::Command;
    };
    CompletionContext::Literal {
        candidates: candidates
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    }
}

fn function_definition_parameters(input: &str) -> Vec<String> {
    let Some(open_parenthesis) = input.find('(') else {
        return Vec::new();
    };
    let Some(close_offset) = input[open_parenthesis + 1..].find(')') else {
        return Vec::new();
    };
    input[open_parenthesis + 1..open_parenthesis + 1 + close_offset]
        .split(',')
        .map(str::trim)
        .filter(|parameter| !parameter.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
struct CompletionAnalysis {
    context: CompletionContext,
    replacement_range: (usize, usize),
}

fn analyze_completion(input: &str, app: &App) -> CompletionAnalysis {
    let context = analyze_input_context(input, app);
    let replacement_range = get_replacement_range(input, &context, app);
    CompletionAnalysis {
        context,
        replacement_range,
    }
}

/// 分析参数字符串上下文（用于正常模式和 InsertEnterParams 模式）
fn analyze_param_context(
    param_str: &str,
    module_name: &str,
    cmd_type: CommandType,
) -> CompletionContext {
    // 解析参数字符串以确定当前上下文
    // 只把最外层的逗号和等号当作模块参数语法；函数调用和列表内部的
    // 分隔符属于参数值表达式。
    let last_comma = find_last_top_level_char(param_str, ',');
    let last_equal = find_last_top_level_char(param_str, '=');

    // 确定当前是在参数名、等号后，还是值之后
    match (last_comma, last_equal) {
        (None, None) => {
            // 没有逗号也没有等号：正在输入第一个参数名
            CompletionContext::ModuleParam {
                cmd_type,
                module_name: module_name.to_string(),
            }
        }
        (Some(_comma_pos), None) => {
            // 有逗号但没有等号（在逗号之后）：正在输入下一个参数名
            // 计算已经输入了多少个参数（逗号数量）
            CompletionContext::ModuleParam {
                cmd_type,
                module_name: module_name.to_string(),
            }
        }
        (None, Some(equal_pos)) => {
            // 有等号但没有逗号：正在输入第一个参数的值
            // 提取参数名
            let param_name = param_str[..equal_pos].trim().to_string();
            CompletionContext::ModuleParamValue {
                cmd_type,
                module_name: module_name.to_string(),
                module_param_name: param_name,
            }
        }
        (Some(comma_pos), Some(equal_pos)) => {
            if comma_pos > equal_pos {
                CompletionContext::ModuleParam {
                    cmd_type,
                    module_name: module_name.to_string(),
                }
            } else {
                // 最后一个等号在逗号之后：正在输入当前参数的值
                // 提取最后一个等号之后的参数名
                let after_last_comma = param_str[comma_pos + 1..].trim();
                if let Some(param_equal_pos) = after_last_comma.find('=') {
                    let param_name = after_last_comma[..param_equal_pos].trim().to_string();
                    CompletionContext::ModuleParamValue {
                        cmd_type,
                        module_name: module_name.to_string(),
                        module_param_name: param_name,
                    }
                } else {
                    // 应该不会发生这种情况
                    CompletionContext::ModuleParam {
                        cmd_type,
                        module_name: module_name.to_string(),
                    }
                }
            }
        }
    }
}

fn find_last_top_level_char(input: &str, needle: char) -> Option<usize> {
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    let mut last_match = None;

    for (index, ch) in input.char_indices() {
        match ch {
            '(' => parentheses += 1,
            ')' => parentheses = parentheses.saturating_sub(1),
            '[' => brackets += 1,
            ']' => brackets = brackets.saturating_sub(1),
            _ if ch == needle && parentheses == 0 && brackets == 0 => {
                last_match = Some(index);
            }
            _ => {}
        }
    }

    last_match
}

/// 规范化路径，处理相对路径符号如 ./ 和 ../
fn normalize_path(path: &str) -> String {
    let path_buf = PathBuf::from(path)
        .components()
        .fold(PathBuf::new(), |mut acc, component| {
            match component {
                Component::ParentDir => {
                    acc.pop();
                }
                Component::CurDir => {
                    // 当前目录，不做任何操作
                }
                _ => {
                    acc.push(component);
                }
            }
            acc
        });

    path_buf.to_string_lossy().to_string()
}

/// 根据前缀过滤字符串列表
fn filter_by_prefix(items: &[CompletionCandidate], prefix: &str) -> Vec<CompletionCandidate> {
    if prefix.is_empty() {
        return items.to_vec();
    }
    items
        .iter()
        .filter(|item| item.content.starts_with(prefix))
        .cloned()
        .collect()
}

/// 从参数字符串中提取当前正在输入的参数名部分
fn get_current_param_name_part(param_str: &str) -> String {
    // 查找最后一个参数分隔符逗号之后的部分，如果没有逗号则从头开始
    // 忽略括号内的逗号
    let after_last_comma = if let Some(pos) = find_last_param_separator(param_str) {
        &param_str[pos + 1..]
    } else {
        param_str
    };

    // 如果包含等号，则等号之前的部分是参数名
    if let Some(equal_pos) = after_last_comma.find('=') {
        after_last_comma[..equal_pos].trim().to_string()
    } else {
        after_last_comma.trim().to_string()
    }
}

/// 查找最后一个参数分隔符逗号的位置（忽略括号内的逗号）
fn find_last_param_separator(param_str: &str) -> Option<usize> {
    find_last_top_level_char(param_str, ',')
}

/// 从指定位置开始查找下一个参数分隔符逗号的位置（忽略括号内的逗号）
fn find_next_param_separator_from(param_str: &str, start: usize) -> Option<usize> {
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    for (index, ch) in param_str
        .char_indices()
        .filter(|(index, _)| *index >= start)
    {
        match ch {
            '(' => parentheses += 1,
            ')' => parentheses = parentheses.saturating_sub(1),
            '[' => brackets += 1,
            ']' => brackets = brackets.saturating_sub(1),
            ',' if parentheses == 0 && brackets == 0 => {
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

/// 从参数字符串中提取当前正在输入的参数值部分
fn get_current_param_value_part(param_str: &str, param_name: &str) -> String {
    // 查找指定参数名的等号之后的部分
    // 参数格式可能是 "size=10, height=20" 或者 "size=10"
    // 我们需要找到 param_name= 之后的部分，直到下一个逗号或字符串结束

    let pattern = format!("{}=", param_name);
    if let Some(start) = param_str.find(&pattern) {
        let value_start = start + pattern.len();
        // 查找下一个逗号或字符串结束，忽略括号内的逗号
        let end = find_next_param_separator_from(param_str, value_start).unwrap_or(param_str.len());
        let (fragment_start, fragment_end) = value_fragment_range(param_str, value_start, end);
        param_str[fragment_start..fragment_end].to_string()
    } else {
        String::new()
    }
}

/// 返回光标所在值表达式中最后一个标识符片段的字节范围。
/// 从表达式末尾向前扫描，因此同时支持函数、列表、索引和运算符之后的补全。
fn value_fragment_range(input: &str, value_start: usize, value_end: usize) -> (usize, usize) {
    let value = &input[value_start..value_end];
    let fragment_end = value_start + value.trim_end_matches(char::is_whitespace).len();
    let fragment = &input[value_start..fragment_end];
    let fragment_start = fragment
        .char_indices()
        .rev()
        .find(|(_, character)| !character.is_alphanumeric() && !matches!(character, '_' | '$'))
        .map(|(index, character)| value_start + index + character.len_utf8())
        .unwrap_or(value_start);
    (fragment_start.min(fragment_end), fragment_end)
}

fn value_has_open_container(param_str: &str, param_name: &str) -> bool {
    let pattern = format!("{}=", param_name);
    let Some(start) = param_str.find(&pattern) else {
        return false;
    };
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    for ch in param_str[start + pattern.len()..].chars() {
        match ch {
            '(' => parentheses += 1,
            ')' => parentheses = parentheses.saturating_sub(1),
            '[' => brackets += 1,
            ']' => brackets = brackets.saturating_sub(1),
            _ => {}
        }
    }
    parentheses > 0 || brackets > 0
}

/// 获取模块中还未输入的参数列表（包括有默认值的可选参数）
fn get_remaining_parameters(app: &App, module_name: &str, input: &str) -> Vec<String> {
    let mut remaining = Vec::new();

    if let Some(module_def) = app.library.get_module(module_name) {
        // 获取已输入的参数名
        let entered_params = parse_parameter_names(input);

        // 添加所有未输入的参数（包括有默认值的）
        for param in &module_def.parameters {
            if !entered_params.contains(&param.name) {
                remaining.push(param.name.clone());
            }
        }
    }

    remaining
}

/// 判断当前参数是否是最后一个未输入的参数
fn is_last_parameter(app: &App, module_name: &str, input: &str) -> bool {
    let remaining = get_remaining_parameters(app, module_name, input);
    // 当前参数在剩余参数列表中，且是最后一个
    remaining.is_empty()
}

/// 根据输入模式和上下文提取模块名和参数字符串
fn extract_module_and_param_str(
    app: &App,
    input: &str,
    cmd_type: &CommandType,
) -> (Option<String>, String) {
    if app.input_mode == InputMode::ModuleEnterParams {
        (app.pending_module_name.clone(), input.trim().to_string())
    } else {
        // 正常命令模式：从输入中提取模块名和参数字符串
        let parts: Vec<&str> = input.split_whitespace().collect();
        if matches!(cmd_type, CommandType::Module | CommandType::Replace) {
            if parts.len() >= 2 {
                let module_name = Some(parts[1].to_string());
                let param_str = if parts.len() > 2 {
                    parts[2..].join(" ")
                } else {
                    String::new()
                };
                (module_name, param_str)
            } else {
                (None, String::new())
            }
        } else if cmd_type == &CommandType::Param {
            if !parts.is_empty() {
                let module_name = Some(parts[0].to_string());
                let param_str = if parts.len() > 1 {
                    parts[1..].join(" ")
                } else {
                    String::new()
                };
                (module_name, param_str)
            } else {
                (None, String::new())
            }
        } else {
            (None, String::new())
        }
    }
}

fn find_node_in_slice<'a>(nodes: &'a [ModuleNode], node_id: &str) -> Option<&'a ModuleNode> {
    nodes.iter().find_map(|node| {
        if node.id == node_id {
            Some(node)
        } else {
            find_node_in_slice(&node.children, node_id)
        }
    })
}

fn completion_target_nodes(app: &App) -> Vec<&ModuleNode> {
    let target_ids = if app.selected_nodes.is_empty() {
        app.tree_state
            .borrow()
            .selected()
            .last()
            .cloned()
            .into_iter()
            .collect()
    } else {
        app.selected_nodes.clone()
    };
    target_ids
        .iter()
        .filter_map(|node_id| {
            app.ast.find_node_by_id(node_id).or_else(|| {
                app.ast
                    .module_defines
                    .iter()
                    .find_map(|definition| find_node_in_slice(&definition.body, node_id))
            })
        })
        .collect()
}

fn node_parameter_names(app: &App) -> Vec<String> {
    let targets = completion_target_nodes(app);
    let Some(first) = targets.first() else {
        return Vec::new();
    };
    let Some(first_definition) = app.library.get_module(&first.name) else {
        return Vec::new();
    };
    first_definition
        .parameters
        .iter()
        .map(|parameter| parameter.name.clone())
        .filter(|name| {
            targets.iter().skip(1).all(|node| {
                app.library
                    .get_module(&node.name)
                    .is_some_and(|definition| {
                        definition
                            .parameters
                            .iter()
                            .any(|parameter| parameter.name == *name)
                    })
            })
        })
        .collect()
}

fn explicit_node_parameter_value(app: &App, node: &ModuleNode, name: &str) -> Option<String> {
    if let Some(value) = node.args.iter().find_map(|argument| match argument {
        Argument::Named {
            name: argument_name,
            value,
        } if argument_name == name => Some(value),
        _ => None,
    }) {
        return Some(value.to_scad());
    }
    let position = app
        .library
        .get_module(&node.name)?
        .parameters
        .iter()
        .position(|parameter| parameter.name == name)?;
    node.args
        .iter()
        .filter_map(|argument| match argument {
            Argument::Positional(value) => Some(value),
            Argument::Named { .. } => None,
        })
        .nth(position)
        .map(|value| value.to_scad())
}

fn node_parameter_candidates(app: &App) -> Vec<CompletionCandidate> {
    let targets = completion_target_nodes(app);
    node_parameter_names(app)
        .into_iter()
        .map(|name| {
            let values = targets
                .iter()
                .map(|node| explicit_node_parameter_value(app, node, &name))
                .collect::<Vec<_>>();
            let common_value = values.first().cloned().flatten().filter(|value| {
                values
                    .iter()
                    .all(|candidate| candidate.as_ref() == Some(value))
            });
            let content =
                common_value.map_or_else(|| name.clone(), |value| format!("{name}={value}"));
            CompletionCandidate::new(content, CandidateType::ModuleParam)
        })
        .collect()
}

fn module_scope_parameter_names(app: &App) -> Vec<String> {
    let Some(target) = completion_target_nodes(app).first().copied() else {
        return Vec::new();
    };
    let Some(module_name) = app.find_module_definition_for_node(&target.id) else {
        return Vec::new();
    };
    app.ast
        .module_defines
        .iter()
        .find(|definition| definition.name == module_name)
        .map(|definition| {
            definition
                .parameters
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn expression_candidates(
    app: &App,
    local_identifiers: &[String],
    default_value: Option<String>,
    include_functions: bool,
) -> Vec<CompletionCandidate> {
    let mut candidates = Vec::new();
    if let Some(default_value) = default_value {
        candidates.push(CompletionCandidate::new(
            default_value,
            CandidateType::Value,
        ));
    }
    for identifier in local_identifiers {
        candidates.push(CompletionCandidate::new(
            identifier.clone(),
            CandidateType::GlobalVar,
        ));
    }
    for literal in ["true", "false", "undef"] {
        if !candidates
            .iter()
            .any(|candidate| candidate.content == literal)
        {
            candidates.push(CompletionCandidate::new(
                literal.to_string(),
                CandidateType::Value,
            ));
        }
    }
    for variable in &app.ast.global_variables {
        let completion_name = variable.name.clone();
        if !candidates
            .iter()
            .any(|candidate| candidate.content == completion_name)
        {
            candidates.push(CompletionCandidate::new(
                completion_name,
                CandidateType::GlobalVar,
            ));
        }
    }
    if include_functions {
        for function in app.library.get_all_functions() {
            candidates.push(CompletionCandidate::new(
                function.name,
                CandidateType::Function,
            ));
        }
    }
    candidates
}

/// 生成候选列表
/// 对于命令，从命令列表读取，对于模块，从模块列表读取，对于模块参数名解析模块获取，对于模块参数值，从模块参数默认值和全局变量
/// AstRoot.global_variables 中获取
fn generate_completions(input: &str, app: &App) -> (Vec<CompletionCandidate>, CompletionAnalysis) {
    let analysis = analyze_completion(input, app);
    let context = &analysis.context;

    let candidates = match &context {
        CompletionContext::Command => {
            // 命令补全：获取所有命令，过滤以匹配输入前缀
            let all_commands: Vec<CompletionCandidate> = get_command_list(app)
                .iter()
                .map(|c| CompletionCandidate::new(c.clone(), CandidateType::Command))
                .collect();

            let prefix = input.trim();
            filter_by_prefix(&all_commands, prefix)
        }
        CompletionContext::CommandPath { candidates } => {
            let candidates = candidates
                .iter()
                .cloned()
                .map(|value| CompletionCandidate::new(value, CandidateType::Command))
                .collect::<Vec<_>>();
            let line = CommandLine::parse(input);
            let prefix = if line.trailing_separator {
                ""
            } else {
                line.tokens
                    .last()
                    .map(|token| token.value.as_str())
                    .unwrap_or("")
            };
            filter_by_prefix(&candidates, prefix)
        }
        CompletionContext::Module => {
            // 模块补全：获取所有模块，过滤以匹配输入中的模块部分
            let all_modules: Vec<CompletionCandidate> = get_module_list(app)
                .iter()
                .map(|c| CompletionCandidate::new(c.clone(), CandidateType::Module))
                .collect();
            // 提取可能已输入的部分模块名
            let parts: Vec<&str> = input.split_whitespace().collect();
            let prefix = if parts.len() > 1 {
                parts[1] // 已经输入的部分模块名
            } else {
                "" // 还没有输入模块名
            };
            filter_by_prefix(&all_modules, prefix)
        }
        CompletionContext::ModuleParam {
            cmd_type: _cmd_type,
            module_name,
        } => {
            // 模块参数补全：获取模块的所有参数，过滤掉已输入的参数
            if let Some(module_def) = app.library.get_module(module_name) {
                // 获取已输入的参数名
                let (_, param_str) = extract_module_and_param_str(app, input, _cmd_type);
                let entered_params = parse_parameter_names(&param_str);

                // 过滤掉已输入的参数
                let mut candidates: Vec<CompletionCandidate> = module_def
                    .parameters
                    .iter()
                    .map(|p| p.name.clone())
                    .filter(|name| !entered_params.contains(name))
                    .map(|c| CompletionCandidate::new(c.clone(), CandidateType::ModuleParam))
                    .collect();

                // 如果有部分输入的参数名，进行过滤
                // 查找当前正在输入的参数名部分
                let current_param_part = get_current_param_name_part(&param_str);
                if !current_param_part.is_empty() {
                    candidates = filter_by_prefix(&candidates, &current_param_part);
                }
                candidates
            } else {
                Vec::new()
            }
        }
        CompletionContext::ModuleParamValue {
            cmd_type: _cmd_type,
            module_name,
            module_param_name,
        } => {
            let (_, param_str) = extract_module_and_param_str(app, input, _cmd_type);
            let inside_container = value_has_open_container(&param_str, module_param_name);
            let default_value = app.library.get_module(module_name).and_then(|module_def| {
                module_def
                    .parameters
                    .iter()
                    .find(|p| p.name == *module_param_name)
                    .and_then(|parameter| parameter.default.clone())
            });
            let mut candidates = expression_candidates(
                app,
                &[],
                (!inside_container).then_some(default_value).flatten(),
                true,
            );

            // 如果有部分输入的值，进行过滤
            let current_value_part = get_current_param_value_part(&param_str, module_param_name);
            if !current_value_part.is_empty() {
                candidates = filter_by_prefix(&candidates, &current_value_part);
            }
            candidates
        }
        CompletionContext::NodeParam => {
            let candidates = node_parameter_candidates(app);
            let prefix = whitespace_token_range(input, 1)
                .map(|(start, end)| &input[start..end])
                .unwrap_or("");
            filter_by_prefix(&candidates, prefix)
        }
        CompletionContext::NodeParamUnset => {
            let candidates: Vec<CompletionCandidate> = node_parameter_names(app)
                .into_iter()
                .map(|name| CompletionCandidate::new(name, CandidateType::ModuleParam))
                .collect();
            let prefix = whitespace_token_range(input, 1)
                .map(|(start, end)| &input[start..end])
                .unwrap_or("");
            filter_by_prefix(&candidates, prefix)
        }
        CompletionContext::NodeParamValue { parameter_name } => {
            let default_value = completion_target_nodes(app)
                .first()
                .and_then(|target| app.library.get_module(&target.name))
                .and_then(|definition| {
                    definition
                        .parameters
                        .iter()
                        .find(|parameter| parameter.name == *parameter_name)
                        .and_then(|parameter| parameter.default.clone())
                });
            let candidates =
                expression_candidates(app, &module_scope_parameter_names(app), default_value, true);
            let value_source = input.split_once('=').map(|(_, value)| value).unwrap_or("");
            let (start, end) = value_fragment_range(value_source, 0, value_source.len());
            filter_by_prefix(&candidates, value_source[start..end].trim())
        }
        CompletionContext::ExpressionValue {
            local_identifiers, ..
        } => {
            let candidates = expression_candidates(app, local_identifiers, None, true);
            let value_source = input.split_once('=').map(|(_, value)| value).unwrap_or("");
            let (start, end) = value_fragment_range(value_source, 0, value_source.len());
            filter_by_prefix(&candidates, value_source[start..end].trim())
        }
        CompletionContext::DefinitionName { kind } => {
            let candidates = match kind {
                DefinitionCompletionKind::Function => app
                    .ast
                    .function_defines
                    .iter()
                    .map(|definition| {
                        CompletionCandidate::new(definition.name.clone(), CandidateType::Function)
                    })
                    .collect::<Vec<_>>(),
                DefinitionCompletionKind::Global => app
                    .ast
                    .global_variables
                    .iter()
                    .map(|variable| {
                        CompletionCandidate::new(variable.name.clone(), CandidateType::GlobalVar)
                    })
                    .collect::<Vec<_>>(),
            };
            let prefix = whitespace_token_range(input, 1)
                .map(|(start, end)| &input[start..end])
                .unwrap_or("");
            filter_by_prefix(&candidates, prefix)
        }
        CompletionContext::File {
            base_dir,
            partial_name,
            extensions,
            ..
        } => {
            // 文件补全 - 使用基础目录和部分名称
            let candidates: Vec<CompletionCandidate> =
                get_file_completions(base_dir, partial_name, extensions)
                    .iter()
                    .map(|c| CompletionCandidate::new(c.clone(), CandidateType::Path))
                    .collect();
            candidates
        }
        CompletionContext::Literal { candidates } => {
            let candidates = candidates
                .iter()
                .cloned()
                .map(|value| CompletionCandidate::new(value, CandidateType::Command))
                .collect::<Vec<_>>();
            let line = CommandLine::parse(input);
            let prefix = if line.trailing_separator {
                ""
            } else {
                line.tokens
                    .last()
                    .map(|token| token.value.as_str())
                    .unwrap_or("")
            };
            filter_by_prefix(&candidates, prefix)
        }
    };

    (candidates, analysis)
}

/// 预览选中的候选项, 替换缓冲区中的补全内容
fn preview_completion(app: &mut App) {
    if app.completion_candidates.is_empty() {
        return;
    }

    // 替换输入缓冲区中的范围
    let (start, end) = app.completion_replacement_range;
    let candidate = match &app.completion_context {
        CompletionContext::File {
            current_path: _,
            base_dir: _,
            partial_name: _,
            ends_with_separator: _,
            ..
        } => {
            if app.input_buffer.content().trim().ends_with("~") {
                format!(
                    "~/{}",
                    app.completion_candidates[app.completion_index].content
                )
            } else {
                app.completion_candidates[app.completion_index]
                    .content
                    .clone()
            }
        }
        _ => app.completion_candidates[app.completion_index]
            .content
            .clone(),
    };

    // Use InputBuffer's replace_range method
    app.input_buffer.replace_range(start, end, &candidate);
    app.completion_replacement_range = (start, start + candidate.len());
}

fn whitespace_token_range(input: &str, token_index: usize) -> Option<(usize, usize)> {
    let mut token_count = 0;
    let mut token_start = None;

    for (index, character) in input.char_indices() {
        if character.is_whitespace() {
            if let Some(start) = token_start.take() {
                if token_count == token_index {
                    return Some((start, index));
                }
                token_count += 1;
            }
        } else if token_start.is_none() {
            token_start = Some(index);
        }
    }

    token_start.and_then(|start| {
        if token_count == token_index {
            Some((start, input.len()))
        } else {
            None
        }
    })
}

fn file_replacement_range(input: &str, ends_with_separator: bool) -> (usize, usize) {
    let line = CommandLine::parse(input);
    if line.trailing_separator {
        return (input.len(), input.len());
    }
    let Some(token) = line.tokens.last() else {
        return (input.len(), input.len());
    };
    let raw = &input[token.range.clone()];
    let quote = raw
        .chars()
        .next()
        .filter(|character| matches!(character, '"' | '\''));
    let content_start = token.range.start + quote.map_or(0, char::len_utf8);
    let closed_quote = quote.is_some_and(|quote| raw.ends_with(quote) && raw.len() > 1);
    let content_end = token.range.end - if closed_quote { 1 } else { 0 };
    if ends_with_separator {
        return (content_end, content_end);
    }
    let content = &input[content_start..content_end];
    let start = content
        .rfind('/')
        .map(|slash| content_start + slash + 1)
        .unwrap_or(content_start);
    (start, content_end)
}

fn format_path_candidate(input: &str, replacement_start: usize, candidate: &str) -> String {
    let already_quoted = CommandLine::parse(input).tokens.into_iter().any(|token| {
        token.range.start <= replacement_start
            && replacement_start <= token.range.end
            && input[token.range]
                .chars()
                .next()
                .is_some_and(|character| matches!(character, '"' | '\''))
    });
    if !already_quoted && candidate.chars().any(char::is_whitespace) {
        format!(
            "\"{}\"",
            candidate.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        candidate.to_string()
    }
}

/// 获取输入缓冲区中需要替换的范围（起始索引和结束索引）
fn get_replacement_range(input: &str, context: &CompletionContext, app: &App) -> (usize, usize) {
    match context {
        CompletionContext::Command => {
            // 命令补全：替换第一个单词（或部分单词）
            let trimmed = input.trim();
            if trimmed.is_empty() {
                (input.len(), input.len())
            } else {
                // 找到第一个单词的结束位置
                let first_word_end = trimmed.find(' ').unwrap_or(trimmed.len());
                let first_word = &trimmed[..first_word_end];
                // 在原始输入中找到第一个单词的位置
                let offset = input.len() - trimmed.len();
                (offset, offset + first_word.len())
            }
        }
        CompletionContext::CommandPath { .. } => {
            let line = CommandLine::parse(input);
            if line.trailing_separator {
                (input.len(), input.len())
            } else {
                line.tokens
                    .last()
                    .map(|token| (token.range.start, token.range.end))
                    .unwrap_or((input.len(), input.len()))
            }
        }
        CompletionContext::Module => {
            // 模块补全：替换第二个单词（模块名部分）
            whitespace_token_range(input, 1).unwrap_or((input.len(), input.len()))
        }
        CompletionContext::ModuleParam {
            cmd_type: _cmd_type,
            module_name: _module_name,
        } => {
            // 模块参数补全：替换当前正在输入的参数名部分
            let (_, param_str) = extract_module_and_param_str(app, input, _cmd_type);
            let current_param_part = get_current_param_name_part(&param_str);

            // 在原始输入中找到参数部分的位置
            let param_start = if param_str.is_empty() {
                input.len()
            } else {
                input.rfind(&param_str).unwrap_or(input.len())
            };

            if current_param_part.is_empty() {
                // 用户尚未开始输入参数名，替换位置应该在最后一个逗号之后
                // 如果没有逗号，则在参数字符串末尾
                if let Some(comma_pos) = find_last_param_separator(&param_str) {
                    // 逗号之后的位置
                    (param_start + comma_pos + 1, param_start + comma_pos + 1)
                } else {
                    // 没有逗号，在参数字符串末尾
                    (param_start + param_str.len(), param_start + param_str.len())
                }
            } else {
                // 用户已输入部分参数名，替换该部分
                let current_part_start = param_str.rfind(&current_param_part).unwrap_or(0);
                (
                    param_start + current_part_start,
                    param_start + current_part_start + current_param_part.len(),
                )
            }
        }
        CompletionContext::ModuleParamValue {
            cmd_type: _cmd_type,
            module_name: _module_name,
            module_param_name,
        } => {
            // 模块参数值补全：替换当前参数的值部分
            // 使用 extract_module_and_param_str 获取参数字符串
            let (_, param_str) = extract_module_and_param_str(app, input, _cmd_type);

            // 找到参数值部分的位置
            let pattern = format!("{}=", module_param_name);
            if let Some(start) = param_str.find(&pattern) {
                let value_start = start + pattern.len();
                let value_end = find_next_param_separator_from(&param_str, value_start)
                    .unwrap_or(param_str.len());
                let (fragment_start, fragment_end) =
                    value_fragment_range(&param_str, value_start, value_end);
                // 在原始输入中找到参数字符串的位置
                let param_start = input.find(&param_str).unwrap_or(input.len());
                (param_start + fragment_start, param_start + fragment_end)
            } else {
                (input.len(), input.len())
            }
        }
        CompletionContext::NodeParam | CompletionContext::NodeParamUnset => {
            whitespace_token_range(input, 1).unwrap_or((input.len(), input.len()))
        }
        CompletionContext::NodeParamValue { .. } | CompletionContext::ExpressionValue { .. } => {
            let Some(equals) = input.find('=') else {
                return (input.len(), input.len());
            };
            let value_start = equals + 1;
            value_fragment_range(input, value_start, input.len())
        }
        CompletionContext::DefinitionName { .. } => {
            whitespace_token_range(input, 1).unwrap_or((input.len(), input.len()))
        }
        CompletionContext::File {
            current_path: _,
            base_dir: _,
            partial_name: _,
            ends_with_separator,
            ..
        } => file_replacement_range(input, *ends_with_separator),
        CompletionContext::Literal { .. } => {
            let line = CommandLine::parse(input);
            if line.trailing_separator {
                (input.len(), input.len())
            } else {
                line.tokens
                    .last()
                    .map(|token| (token.range.start, token.range.end))
                    .unwrap_or((input.len(), input.len()))
            }
        }
    }
}

/// 应用选中的候选项，并退出补全模式, 对于命令补全，需要追加空格，对于模块补全，需要追加空格
/// 对于模块参数名补全需要追加 "=" 等号，对于模块参数值补全需要追加 "," 逗号
fn apply_completion(app: &mut App) {
    if app.completion_candidates.is_empty() {
        return;
    }

    let candidate = &app.completion_candidates[app.completion_index];
    let (start, end) = app.completion_replacement_range;

    // 替换输入缓冲区中的范围
    let replacement = match &app.completion_context {
        CompletionContext::File { .. } => {
            format_path_candidate(app.input_buffer.content(), start, &candidate.content)
        }
        _ => candidate.content.clone(),
    };
    app.input_buffer.replace_range(start, end, &replacement);

    // 根据上下文追加分隔符
    match &app.completion_context {
        CompletionContext::ModuleParamValue {
            cmd_type: _cmd_type,
            module_name,
            module_param_name,
        } => {
            // 检查当前参数是否是最后一个参数, 不是最后一个参数，追加逗号
            let (_, param_str) =
                extract_module_and_param_str(app, app.input_buffer.content(), _cmd_type);
            if !is_last_parameter(app, module_name, &param_str)
                || candidate.candidate_type == CandidateType::Function
                || value_has_open_container(&param_str, module_param_name)
            {
                app.input_buffer
                    .insert_str(candidate.candidate_type.separator());
            }
        }
        CompletionContext::File {
            current_path: _current_path,
            base_dir: _base_dir,
            partial_name: _partial_name,
            ends_with_separator: _ends_with_separator,
            ..
        } => {
            // 需要检查实际文件系统来确定是否是目录
            // 构建完整路径来检查文件类型
            let full_path = Path::new(&_base_dir).join(&candidate.content);
            if let Ok(metadata) = full_path.metadata() {
                if metadata.is_dir() {
                    app.input_buffer
                        .insert_str(candidate.candidate_type.separator());
                } else {
                    // 对于文件，追加空格
                    app.input_buffer.insert_str(" ");
                }
            } else {
                // 如果无法获取元数据，默认追加空格
                app.input_buffer.insert_str(" ");
            }
        }
        CompletionContext::NodeParamValue { .. } | CompletionContext::ExpressionValue { .. } => {
            if candidate.candidate_type == CandidateType::Function {
                app.input_buffer.insert_str("(");
            }
        }
        CompletionContext::DefinitionName { kind } => match kind {
            DefinitionCompletionKind::Function => app.input_buffer.insert_str("("),
            DefinitionCompletionKind::Global => app.input_buffer.insert_str("="),
        },
        CompletionContext::NodeParam => {
            if !candidate.content.contains('=') {
                app.input_buffer.insert_str("=");
            }
        }
        CompletionContext::NodeParamUnset => {}
        _ => {
            app.input_buffer
                .insert_str(candidate.candidate_type.separator());
        }
    }

    // 退出补全模式
    app.completion_active = false;
    app.completion_candidates.clear();
    app.completion_index = 0;
}

/// Get list of available commands from the command registry
fn get_command_list(app: &App) -> Vec<String> {
    app.command_registry.child_names(&[])
}

/// Get list of available modules from library
fn get_module_list(app: &App) -> Vec<String> {
    app.library.get_module_names()
}

/// Get file completions for a given directory and path prefix
/// Returns entries in the directory that match the prefix (without trailing '/')
fn get_file_completions(dir_path: &str, prefix: &str, extensions: &[String]) -> Vec<String> {
    let mut completions = Vec::new();

    // Parse the directory path
    let dir = Path::new(dir_path);

    // Try to read the directory
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                // Using underscore to indicate unused
                if let Ok(file_name) = entry.file_name().into_string() {
                    // Check if it matches the partial name
                    let accepted_extension = extensions.is_empty()
                        || file_type.is_dir()
                        || Path::new(&file_name)
                            .extension()
                            .and_then(|extension| extension.to_str())
                            .is_some_and(|extension| {
                                extensions
                                    .iter()
                                    .any(|accepted| extension.eq_ignore_ascii_case(accepted))
                            });
                    if file_name.starts_with(prefix) && accepted_extension {
                        // Add the file/directory name (without trailing '/')
                        completions.push(file_name);
                    }
                }
            }
        }
    }

    // Sort alphabetically (directories first)
    // We need to get file types again to sort properly
    let mut sorted_completions = Vec::new();
    for name in completions {
        let full_path = Path::new(dir_path).join(&name);
        if let Ok(metadata) = full_path.metadata() {
            if metadata.is_dir() {
                sorted_completions.push((name, true)); // directory
            } else {
                sorted_completions.push((name, false)); // file
            }
        } else {
            sorted_completions.push((name, false)); // default to file if we can't determine
        }
    }

    sorted_completions.sort_by(|a, b| {
        // Sort directories first, then alphabetically
        match (a.1, b.1) {
            (true, false) => std::cmp::Ordering::Less, // directory first
            (false, true) => std::cmp::Ordering::Greater, // then file
            _ => a.0.cmp(&b.0),                        // both same type, alphabetical
        }
    });

    // Extract just the names
    sorted_completions
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_parameter_names_ignores_nested_commas() {
        let names = parse_parameter_names("size=[sin(10, 20), 3], center=true");
        assert_eq!(names, vec!["size", "center"]);
    }

    #[test]
    fn test_analyze_param_context_keeps_list_value_context() {
        let context = analyze_param_context("size=[1, si", "cube", CommandType::Module);
        assert_eq!(
            context,
            CompletionContext::ModuleParamValue {
                cmd_type: CommandType::Module,
                module_name: "cube".to_string(),
                module_param_name: "size".to_string(),
            }
        );
    }

    #[test]
    fn test_analyze_param_context_uses_top_level_comma() {
        let context = analyze_param_context(
            "size=[sin(10, 20), 3], center=tr",
            "cube",
            CommandType::Module,
        );
        assert_eq!(
            context,
            CompletionContext::ModuleParamValue {
                cmd_type: CommandType::Module,
                module_name: "cube".to_string(),
                module_param_name: "center".to_string(),
            }
        );
    }

    #[test]
    fn test_current_value_part_is_nested_expression_fragment() {
        assert_eq!(
            get_current_param_value_part("size=[1, sin(10), sq", "size"),
            "sq"
        );
        assert_eq!(
            get_current_param_value_part("size=sin(10, co", "size"),
            "co"
        );
    }

    #[test]
    fn test_value_fragment_range_preserves_nested_expression() {
        let input = "size=[1, sin(10), sq";
        let (start, end) = value_fragment_range(input, 5, input.len());
        assert_eq!(&input[start..end], "sq");
        assert_eq!(&input[..start], "size=[1, sin(10), ");
    }

    #[test]
    fn test_value_fragment_range_handles_expression_operators() {
        for (input, expected) in [
            ("sin(a) + co", "co"),
            ("width * sq", "sq"),
            ("values[si", "si"),
            ("angle > 0 ? si", "si"),
            ("$f", "$f"),
        ] {
            let (start, end) = value_fragment_range(input, 0, input.len());
            assert_eq!(&input[start..end], expected);
        }
    }

    #[test]
    fn test_generate_completions_filters_function_inside_list() {
        let app = App::new();
        let (candidates, analysis) = generate_completions("insert cube size=[1, si", &app);

        assert!(matches!(
            analysis.context,
            CompletionContext::ModuleParamValue {
                module_param_name,
                ..
            } if module_param_name == "size"
        ));
        assert!(candidates.iter().any(|candidate| {
            candidate.content == "sin" && candidate.candidate_type == CandidateType::Function
        }));
    }

    #[test]
    fn test_value_completion_always_includes_boolean_literals() {
        let app = App::new();
        let (candidates, analysis) = generate_completions("insert cube center=", &app);

        assert!(matches!(
            analysis.context,
            CompletionContext::ModuleParamValue { .. }
        ));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.content == "true"));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.content == "false"));
    }

    #[test]
    fn test_value_completion_hides_whole_default_inside_list() {
        let app = App::new();
        let (candidates, analysis) = generate_completions("insert translate v=[4, ", &app);

        assert!(matches!(
            analysis.context,
            CompletionContext::ModuleParamValue { .. }
        ));
        assert!(!candidates
            .iter()
            .any(|candidate| candidate.content == "[0, 0, 0]"));
        assert!(!candidates
            .iter()
            .any(|candidate| candidate.content == "[0,0,0]"));
    }

    #[test]
    fn test_preview_and_camera_commands_complete_in_stages() {
        let app = App::new();
        let (model, _) = generate_completions("model ", &app);
        assert!(model.iter().any(|candidate| candidate.content == "preview"));
        assert!(model.iter().any(|candidate| candidate.content == "view"));
        let (preview, _) = generate_completions("model preview ", &app);
        assert_eq!(
            preview
                .iter()
                .map(|candidate| candidate.content.as_str())
                .collect::<Vec<_>>(),
            vec!["--render"]
        );

        let (camera, _) = generate_completions("camera view ", &app);
        assert!(camera.iter().any(|candidate| candidate.content == "iso"));
        assert!(camera.iter().any(|candidate| candidate.content == "front"));

        let (_, analysis) = generate_completions("camera projection per", &app);
        assert_eq!(analysis.replacement_range, (18, 21));

        let (projection, _) = generate_completions("camera projection ", &app);
        assert!(projection
            .iter()
            .any(|candidate| candidate.content == "toggle"));
        let (auto_rotate, _) = generate_completions("camera auto-rotate ", &app);
        assert!(auto_rotate
            .iter()
            .any(|candidate| candidate.content == "toggle"));

        let (protocol, _) = generate_completions("display protocol ", &app);
        assert!(protocol
            .iter()
            .any(|candidate| candidate.content == "ascii"));
        assert!(protocol
            .iter()
            .any(|candidate| candidate.content == "halfblocks"));
        assert!(protocol
            .iter()
            .any(|candidate| candidate.content == "braille"));

        let (axes, _) = generate_completions("display axes ", &app);
        assert!(axes.iter().any(|candidate| candidate.content == "toggle"));

        let (visibility, _) = generate_completions("visibility ", &app);
        assert!(visibility
            .iter()
            .any(|candidate| candidate.content == "hide"));
    }

    #[test]
    fn test_successful_command_keeps_handler_feedback() {
        let mut app = App::new();
        app.set_error("stale error");
        assert!(execute_command_registry(
            &mut app,
            "display axes off",
            CommandOrigin::UserInput
        ));
        assert_eq!(app.message.as_deref(), Some("World axes disabled"));
        assert_eq!(app.message_type, crate::app::MessageType::Info);
    }

    #[test]
    fn test_hierarchical_commands_accept_quoted_arguments_and_reject_old_roots() {
        let mut app = App::new();

        execute_user_command(&mut app, "project rename \"vernier caliper\"");
        assert_eq!(app.project_name, "vernier caliper");
        for obsolete in [
            "new", "write", "open", "edit", "buffer", "library", "use", "include", "render",
            "preview", "view", "export", "camera", "protocol", "axes", "assembly",
        ] {
            assert!(
                app.command_registry.find(obsolete).is_none(),
                "obsolete command root should not be registered: {obsolete}"
            );
        }

        let input = "model view \"models/my mo";
        let (_, analysis) = generate_completions(input, &app);
        assert!(matches!(analysis.context, CompletionContext::File { .. }));
        assert_eq!(&input[analysis.replacement_range.0..], "my mo");
    }

    #[test]
    fn test_use_and_include_completion_list_all_project_sources() {
        let mut app = App::new();
        app.ast_mut()
            .embedded_sources
            .push(openscad_core::EmbeddedSourceFile {
                virtual_path: "libraries/gears/gears.scad".to_string(),
                original_path: None,
                role: openscad_core::EmbeddedSourceRole::Library,
                content: String::new(),
                editable: false,
                modules: Vec::new(),
                includes: Vec::new(),
                uses: Vec::new(),
                global_variables: Vec::new(),
                module_defines: Vec::new(),
                function_defines: Vec::new(),
            });
        app.ast_mut()
            .embedded_sources
            .push(openscad_core::EmbeddedSourceFile {
                virtual_path: "libraries/gears/helpers.scad".to_string(),
                original_path: None,
                role: openscad_core::EmbeddedSourceRole::Dependency,
                content: String::new(),
                editable: false,
                modules: Vec::new(),
                includes: Vec::new(),
                uses: Vec::new(),
                global_variables: Vec::new(),
                module_defines: Vec::new(),
                function_defines: Vec::new(),
            });

        let (candidates, analysis) = generate_completions("source use ge", &app);
        assert_eq!(
            analysis.context,
            CompletionContext::Literal {
                candidates: vec!["gears.scad".to_string(), "helpers.scad".to_string()],
            }
        );
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.content.as_str())
                .collect::<Vec<_>>(),
            ["gears.scad"]
        );

        let (include_candidates, include_analysis) =
            generate_completions("source include ge", &app);
        assert_eq!(include_analysis.context, analysis.context);
        assert_eq!(
            include_candidates
                .iter()
                .map(|candidate| candidate.content.as_str())
                .collect::<Vec<_>>(),
            ["gears.scad"]
        );
    }

    #[test]
    fn test_project_source_commands_complete_editable_buffers() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main.scad");
        fs::write(&main, "use <part.scad>; cube(1);").unwrap();
        fs::write(directory.path().join("part.scad"), "sphere(2);").unwrap();
        let mut app = App::new();
        commands::cmd_edit_scad(&mut app, main.to_str().unwrap()).unwrap();

        let (source_actions, _) = generate_completions("source ", &app);
        assert!(source_actions
            .iter()
            .any(|candidate| candidate.content == "next"));
        let (buffers, _) = generate_completions("source switch ", &app);
        assert!(buffers
            .iter()
            .any(|candidate| candidate.content == "part.scad"));
    }

    #[test]
    fn test_resource_export_completion_selects_command_then_file_path() {
        let app = App::new();
        let (model_actions, _) = generate_completions("model ", &app);
        assert!(model_actions
            .iter()
            .any(|candidate| candidate.content == "export"));

        let (_, analysis) = generate_completions("model export ./", &app);
        assert!(matches!(analysis.context, CompletionContext::File { .. }));
    }

    #[test]
    fn test_project_source_nodes_support_keyboard_navigation_and_opening() {
        use ratatui::{backend::TestBackend, Terminal};

        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main.scad");
        fs::write(&main, "use <part.scad>; cube(1);").unwrap();
        fs::write(directory.path().join("part.scad"), "sphere(2);").unwrap();
        let mut app = App::new();
        commands::cmd_edit_scad(&mut app, main.to_str().unwrap()).unwrap();
        app.init_tree_selection();
        assert_eq!(app.tree_state.borrow().selected(), ["__project_sources"]);

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), &mut app);
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut app);
        app.validate_tree_state();
        assert_eq!(
            app.tree_state
                .borrow()
                .selected()
                .last()
                .map(String::as_str),
            Some("__project_source_0")
        );

        handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut app);
        app.validate_tree_state();
        assert_eq!(
            app.tree_state
                .borrow()
                .selected()
                .last()
                .map(String::as_str),
            Some("__project_source_1")
        );
        handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);
        assert_eq!(app.ast.active_source.as_deref(), Some("part.scad"));
    }

    #[test]
    fn test_set_completion_uses_node_and_module_scope_parameters() {
        let mut app = App::new();
        let cube_id = commands::cmd_insert(&mut app, "cube", None, Some("size=10")).unwrap();
        app.selected_nodes = vec![cube_id];
        commands::cmd_moddef(&mut app, "my_box", Some("size=20")).unwrap();
        let body_id = app.ast.module_defines[0].body[0].id.clone();
        app.tree_state.borrow_mut().select(vec![
            "__moddefs".to_string(),
            "__moddef_my_box".to_string(),
            body_id,
        ]);

        let (parameter_candidates, parameter_analysis) = generate_completions("set si", &app);
        assert_eq!(parameter_analysis.context, CompletionContext::NodeParam);
        assert!(parameter_candidates
            .iter()
            .any(|candidate| candidate.content == "size=10"));

        let size_index = parameter_candidates
            .iter()
            .position(|candidate| candidate.content == "size=10")
            .unwrap();
        app.input_buffer.set_content("set si");
        app.completion_candidates = parameter_candidates;
        app.completion_context = parameter_analysis.context;
        app.completion_replacement_range = parameter_analysis.replacement_range;
        app.completion_index = size_index;
        app.completion_active = true;
        apply_completion(&mut app);
        assert_eq!(app.input_buffer.content(), "set size=10");

        let (value_candidates, value_analysis) = generate_completions("set size=si", &app);
        assert_eq!(
            value_analysis.context,
            CompletionContext::NodeParamValue {
                parameter_name: "size".to_string()
            }
        );
        assert!(value_candidates
            .iter()
            .any(|candidate| candidate.content == "size"));

        let (unset_candidates, unset_analysis) = generate_completions("unset si", &app);
        assert_eq!(unset_analysis.context, CompletionContext::NodeParamUnset);
        assert!(unset_candidates
            .iter()
            .any(|candidate| candidate.content == "size"));

        let mut completion_app = app;
        let size_index = unset_candidates
            .iter()
            .position(|candidate| candidate.content == "size")
            .unwrap();
        completion_app.input_buffer.set_content("unset si");
        completion_app.completion_candidates = unset_candidates;
        completion_app.completion_context = unset_analysis.context;
        completion_app.completion_replacement_range = unset_analysis.replacement_range;
        completion_app.completion_index = size_index;
        completion_app.completion_active = true;
        apply_completion(&mut completion_app);
        assert_eq!(completion_app.input_buffer.content(), "unset size");
    }

    #[test]
    fn test_set_completion_omits_value_when_selected_nodes_disagree() {
        let mut app = App::new();
        let first = commands::cmd_insert(&mut app, "cube", None, Some("size=10")).unwrap();
        let second = commands::cmd_insert(&mut app, "cube", None, Some("size=20")).unwrap();
        app.selected_nodes = vec![first, second];

        let (candidates, analysis) = generate_completions("set si", &app);
        let size_index = candidates
            .iter()
            .position(|candidate| candidate.content == "size")
            .expect("different values should not be echoed");
        app.input_buffer.set_content("set si");
        app.completion_candidates = candidates;
        app.completion_context = analysis.context;
        app.completion_replacement_range = analysis.replacement_range;
        app.completion_index = size_index;
        app.completion_active = true;
        apply_completion(&mut app);
        assert_eq!(app.input_buffer.content(), "set size=");
    }

    #[test]
    fn test_function_body_completion_includes_parameters_and_functions() {
        let app = App::new();

        let (parameter_candidates, parameter_analysis) =
            generate_completions("function wave(x, phase) = ph", &app);
        assert_eq!(
            parameter_analysis.context,
            CompletionContext::ExpressionValue {
                kind: ExpressionCompletionKind::FunctionBody,
                local_identifiers: vec!["x".to_string(), "phase".to_string()],
            }
        );
        assert!(parameter_candidates
            .iter()
            .any(|candidate| candidate.content == "phase"));

        let (function_candidates, _) = generate_completions("function wave(x) = si", &app);
        assert!(function_candidates.iter().any(|candidate| {
            candidate.content == "sin" && candidate.candidate_type == CandidateType::Function
        }));
    }

    #[test]
    fn test_function_completion_appends_open_parenthesis() {
        let mut app = App::new();
        let input = "function wave(x) = si";
        let (candidates, analysis) = generate_completions(input, &app);
        let sin_index = candidates
            .iter()
            .position(|candidate| candidate.content == "sin")
            .unwrap();
        app.input_buffer.set_content(input);
        app.completion_candidates = candidates;
        app.completion_context = analysis.context;
        app.completion_replacement_range = analysis.replacement_range;
        app.completion_index = sin_index;
        app.completion_active = true;

        apply_completion(&mut app);

        assert_eq!(app.input_buffer.content(), "function wave(x) = sin(");
    }

    #[test]
    fn test_function_completion_after_binary_operator() {
        let mut app = App::new();
        let input = "function wave(a) = sin(a) + co";
        let (candidates, analysis) = generate_completions(input, &app);
        let cos_index = candidates
            .iter()
            .position(|candidate| candidate.content == "cos")
            .unwrap();
        assert_eq!(&input[analysis.replacement_range.0..], "co");
        app.input_buffer.set_content(input);
        app.completion_candidates = candidates;
        app.completion_context = analysis.context;
        app.completion_replacement_range = analysis.replacement_range;
        app.completion_index = cos_index;
        app.completion_active = true;

        apply_completion(&mut app);

        assert_eq!(
            app.input_buffer.content(),
            "function wave(a) = sin(a) + cos("
        );
    }

    #[test]
    fn test_definition_name_completion_supports_redefinition() {
        let mut app = App::new();
        commands::cmd_global(&mut app, "width=10").unwrap();
        commands::cmd_funcdef(&mut app, "wave(x) = sin(x)").unwrap();

        let (global_candidates, global_analysis) = generate_completions("global wi", &app);
        assert_eq!(
            global_analysis.context,
            CompletionContext::DefinitionName {
                kind: DefinitionCompletionKind::Global,
            }
        );
        app.input_buffer.set_content("global wi");
        app.completion_candidates = global_candidates;
        app.completion_context = global_analysis.context;
        app.completion_replacement_range = global_analysis.replacement_range;
        app.completion_index = 0;
        app.completion_active = true;
        apply_completion(&mut app);
        assert_eq!(app.input_buffer.content(), "global width=");

        let (function_candidates, function_analysis) = generate_completions("function wa", &app);
        assert_eq!(
            function_analysis.context,
            CompletionContext::DefinitionName {
                kind: DefinitionCompletionKind::Function,
            }
        );
        app.input_buffer.set_content("function wa");
        app.completion_candidates = function_candidates;
        app.completion_context = function_analysis.context;
        app.completion_replacement_range = function_analysis.replacement_range;
        app.completion_index = 0;
        app.completion_active = true;
        apply_completion(&mut app);
        assert_eq!(app.input_buffer.content(), "function wave(");
    }

    #[test]
    fn test_global_value_completion_includes_existing_variables_and_functions() {
        let mut app = App::new();
        commands::cmd_global(&mut app, "width=10").unwrap();

        let (candidates, analysis) = generate_completions("global size=wi", &app);

        assert_eq!(
            analysis.context,
            CompletionContext::ExpressionValue {
                kind: ExpressionCompletionKind::GlobalValue,
                local_identifiers: Vec::new(),
            }
        );
        assert!(candidates
            .iter()
            .any(|candidate| candidate.content == "width"));
        let (function_candidates, _) = generate_completions("global size=si", &app);
        assert!(function_candidates.iter().any(|candidate| {
            candidate.content == "sin" && candidate.candidate_type == CandidateType::Function
        }));
    }

    #[test]
    fn test_expression_completion_preserves_special_variable_prefix() {
        let mut app = App::new();
        commands::cmd_global(&mut app, "$fn=64").unwrap();
        commands::cmd_global(&mut app, "width=10").unwrap();

        let (special_candidates, _) = generate_completions("global segments=$f", &app);
        assert!(special_candidates
            .iter()
            .any(|candidate| candidate.content == "$fn"));
        assert!(!special_candidates
            .iter()
            .any(|candidate| candidate.content == "fn"));

        let (regular_candidates, _) = generate_completions("global size=wi", &app);
        assert!(regular_candidates
            .iter()
            .any(|candidate| candidate.content == "width"));
    }

    #[test]
    fn test_replacement_range_only_covers_nested_fragment() {
        let app = App::new();
        let input = "insert cube size=[1, si";
        let analysis = analyze_completion(input, &app);
        let (start, end) = analysis.replacement_range;

        assert_eq!(&input[start..end], "si");
    }

    #[test]
    fn test_module_replacement_range_uses_second_token_position() {
        let app = App::new();
        let input = "insert s";
        let analysis = analyze_completion(input, &app);

        assert_eq!(analysis.context, CompletionContext::Module);
        assert_eq!(analysis.replacement_range, (7, 8));
    }

    #[test]
    fn test_tab_completion_does_not_replace_matching_text_in_command_name() {
        let mut app = App::new();
        app.input_mode = InputMode::Command;
        app.input_buffer.set_content("insert s");

        handle_tab_completion(&mut app);

        assert!(app.input_buffer.content().starts_with("insert "));
        assert_ne!(app.input_buffer.content(), "insert s");
    }

    #[test]
    fn test_tab_completion_cycles_within_the_analyzed_replacement_range() {
        let mut app = App::new();
        app.input_mode = InputMode::Command;
        app.input_buffer.set_content("insert s");

        handle_tab_completion(&mut app);
        assert!(app.completion_candidates.len() > 1);
        let first = app.completion_candidates[0].content.clone();
        assert_eq!(app.input_buffer.content(), format!("insert {}", first));

        handle_tab_completion(&mut app);
        let second = app.completion_candidates[1].content.clone();
        assert_eq!(app.input_buffer.content(), format!("insert {}", second));
        assert_eq!(app.completion_replacement_range, (7, 7 + second.len()));
    }

    #[test]
    fn test_value_has_open_container() {
        assert!(value_has_open_container("size=[1, sin(", "size"));
        assert!(!value_has_open_container("size=[1, sin(2)]", "size"));
    }

    #[test]
    fn test_normal_mode_structural_editing_keys() {
        let mut app = App::new();
        app.ast_mut()
            .modules
            .push(openscad_core::ModuleNode::new_leaf(
                "cube_1".to_string(),
                "cube".to_string(),
                Vec::new(),
            ));
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "cube_1".to_string()]);

        handle_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.node_clipboard[0].id, "cube_1");

        handle_key(
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.ast.modules.len(), 2);

        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "cube_1".to_string()]);
        handle_key(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            &mut app,
        );
        assert!(app.ast.find_node_by_id("cube_1").is_none());

        handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.input_mode, InputMode::Command);
        assert_eq!(app.input_buffer.content(), "replace ");

        app.input_mode = InputMode::Normal;
        app.input_buffer.clear();
        handle_key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.input_mode, InputMode::Command);
        assert_eq!(app.input_buffer.content(), "set ");

        app.input_mode = InputMode::Normal;
        app.input_buffer.clear();
        handle_key(
            KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT),
            &mut app,
        );
        assert_eq!(app.input_mode, InputMode::Command);
        assert_eq!(app.input_buffer.content(), "unset ");
    }

    #[test]
    fn test_space_toggles_visibility_and_upper_i_opens_insert_before_command() {
        let mut app = App::new();
        app.ast_mut()
            .modules
            .push(openscad_core::ModuleNode::new_leaf(
                "cube_1".into(),
                "cube".into(),
                Vec::new(),
            ));
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".into(), "cube_1".into()]);

        handle_key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.ast.modules[0].modifier, Some('*'));

        handle_key(
            KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT),
            &mut app,
        );
        assert_eq!(app.input_mode, InputMode::Command);
        assert_eq!(app.input_buffer.content(), "insert-before ");
    }

    #[test]
    fn test_d_then_p_swaps_adjacent_nodes_through_registered_commands() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            openscad_core::ModuleNode::new_leaf(
                "cube_1".to_string(),
                "cube".to_string(),
                Vec::new(),
            ),
            openscad_core::ModuleNode::new_leaf(
                "sphere_1".to_string(),
                "sphere".to_string(),
                Vec::new(),
            ),
        ];
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "cube_1".to_string()]);

        handle_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            &mut app,
        );
        handle_key(
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
            &mut app,
        );

        assert_eq!(
            app.ast
                .modules
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            ["sphere", "cube"]
        );
        assert_eq!(app.undo_stack.len(), 2);
    }

    #[test]
    fn test_file_shortcuts_use_project_and_source_semantics() {
        let mut app = App::new();
        handle_key(
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.input_buffer.content(), "project open ");

        app.input_mode = InputMode::Normal;
        app.input_buffer.clear();
        handle_key(
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.input_buffer.content(), "source import ");

        app.input_mode = InputMode::Normal;
        app.input_buffer.clear();
        handle_key(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.input_mode, InputMode::Command);
        assert_eq!(app.input_buffer.content(), "source new ");
    }

    #[test]
    fn test_command_history_navigation_restores_unexecuted_input() {
        let mut app = App::new();
        app.add_to_history("insert cube");
        app.input_mode = InputMode::Command;
        app.input_buffer.set_content("replace sph");

        handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &mut app);
        assert_eq!(app.input_buffer.content(), "insert cube");

        handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut app);
        assert_eq!(app.input_buffer.content(), "replace sph");
    }

    #[test]
    fn test_node_commands_do_not_expose_node_ids() {
        let mut app = App::new();
        app.ast_mut()
            .modules
            .push(openscad_core::ModuleNode::new_leaf(
                "cube_123".to_string(),
                "cube".to_string(),
                Vec::new(),
            ));

        let (candidates, analysis) = generate_completions("remove cube_", &app);
        assert_eq!(analysis.context, CompletionContext::Command);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_replace_completion_uses_module_then_parameter_stages() {
        let mut app = App::new();
        app.ast_mut()
            .modules
            .push(openscad_core::ModuleNode::new_leaf(
                "cube_123".to_string(),
                "cube".to_string(),
                Vec::new(),
            ));

        let (candidates, analysis) = generate_completions("replace sp", &app);
        assert_eq!(analysis.context, CompletionContext::Module);
        assert!(candidates
            .iter()
            .any(|candidate| candidate.content == "sphere"));
        assert!(!candidates
            .iter()
            .any(|candidate| candidate.content == "cube_123"));

        let (parameter_candidates, parameter_analysis) =
            generate_completions("replace sphere ", &app);
        assert_eq!(
            parameter_analysis.context,
            CompletionContext::ModuleParam {
                cmd_type: CommandType::Replace,
                module_name: "sphere".to_string(),
            }
        );
        assert!(parameter_candidates
            .iter()
            .any(|candidate| candidate.content == "r"));
    }

    #[test]
    fn test_cancel_replace_parameter_stage_keeps_original_node() {
        let mut app = App::new();
        app.ast_mut()
            .modules
            .push(openscad_core::ModuleNode::new_leaf(
                "shape_1".to_string(),
                "sphere".to_string(),
                Vec::new(),
            ));
        app.pending_module_action = Some(PendingModuleAction::Replace {
            target_ids: vec!["shape_1".to_string()],
        });
        app.pending_module_name = Some("cube".to_string());
        app.input_mode = InputMode::ModuleEnterParams;

        handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut app);

        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.ast.find_node_by_id("shape_1").is_some());
        assert!(app.pending_module_action.is_none());
        assert!(app.pending_module_name.is_none());
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn test_preview_shortcut_reuses_existing_render_and_requests_clear() {
        let mut app = App::new();
        app.model_preview.status = crate::preview::ModelPreviewStatus::Ready { triangles: 12 };
        app.model_preview.set_auto_rotate(true);
        handle_key(
            KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT),
            &mut app,
        );
        assert_eq!(app.screen, crate::app::Screen::ModelPreview);

        handle_key(
            KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT),
            &mut app,
        );
        assert_eq!(app.screen, crate::app::Screen::Editor);
        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(!app.model_preview.auto_rotate);
        assert!(app.take_terminal_clear_request());
    }

    #[test]
    fn test_preview_shortcut_renders_when_preview_is_empty() {
        let mut app = App::new();

        handle_key(
            KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT),
            &mut app,
        );

        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
        assert!(matches!(
            app.model_preview.status,
            crate::preview::ModelPreviewStatus::Loading
        ));
    }

    #[test]
    fn test_uppercase_r_shortcut_always_starts_a_fresh_render() {
        let mut app = App::new();
        app.model_preview.status = crate::preview::ModelPreviewStatus::Ready { triangles: 12 };

        handle_key(
            KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT),
            &mut app,
        );

        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
        assert!(matches!(
            app.model_preview.status,
            crate::preview::ModelPreviewStatus::Loading
        ));
    }

    #[test]
    fn test_model_mouse_drag_is_captured_until_button_up() {
        let mut app = App::new();
        app.enter_model_screen();
        app.ui_regions.preview = ratatui::layout::Rect::new(20, 0, 60, 20);

        handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 30, 5),
            &mut app,
        );
        assert!(app.mouse_drag.is_some());

        // A drag remains captured even after leaving the Preview rectangle.
        handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 10, 5),
            &mut app,
        );
        assert_eq!(app.mouse_drag.unwrap().last_column, 10);
        handle_mouse(
            mouse(MouseEventKind::Up(MouseButton::Left), 10, 5),
            &mut app,
        );
        assert!(app.mouse_drag.is_none());
    }

    #[test]
    fn test_right_mouse_drag_pans_and_middle_button_is_ignored() {
        let mut app = App::new();
        app.enter_model_screen();
        app.ui_regions.preview = ratatui::layout::Rect::new(20, 0, 60, 20);

        handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Right), 30, 5),
            &mut app,
        );
        assert!(app.mouse_drag.unwrap().pan);

        handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Right), 10, 5),
            &mut app,
        );
        assert_eq!(app.mouse_drag.unwrap().last_column, 10);
        handle_mouse(
            mouse(MouseEventKind::Up(MouseButton::Right), 10, 5),
            &mut app,
        );

        handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Middle), 30, 5),
            &mut app,
        );
        assert!(app.mouse_drag.is_none());
    }

    #[test]
    fn test_mouse_orbit_follows_drag_direction() {
        assert_eq!(mouse_orbit_delta(5, 0), (-16.0, 0.0));
        assert_eq!(mouse_orbit_delta(0, -5), (0.0, -16.0));
    }

    #[test]
    fn test_tree_mouse_click_uses_widget_hit_testing() {
        use ratatui::{backend::TestBackend, Terminal};

        let mut app = App::new();
        app.ast_mut()
            .modules
            .push(openscad_core::ModuleNode::new_leaf(
                "cube_mouse".to_string(),
                "cube".to_string(),
                Vec::new(),
            ));
        app.init_tree_selection();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();

        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string()]);
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        // Project Sources occupies the first row, so Modules and its child are rows 2 and 3.
        handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 1, 2),
            &mut app,
        );
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 2, 3),
            &mut app,
        );
        assert_eq!(
            app.tree_state
                .borrow()
                .selected()
                .last()
                .map(String::as_str),
            Some("cube_mouse")
        );
    }

    #[test]
    fn test_camera_source_button_returns_to_normal_source_preview() {
        let mut app = App::new();
        app.enter_model_screen();
        app.ui_regions
            .camera_buttons
            .push(crate::app::CameraButtonRegion {
                area: ratatui::layout::Rect::new(2, 20, 8, 1),
                command: "source preview".into(),
            });

        handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 3, 20),
            &mut app,
        );

        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.screen, crate::app::Screen::Editor);
        assert!(app.take_terminal_clear_request());
    }

    #[test]
    fn test_model_keys_map_to_registered_commands() {
        assert_eq!(
            model_key_command(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
            Some("camera orbit -5 0")
        );
        assert_eq!(
            model_key_command(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)),
            Some("camera projection toggle")
        );
        assert_eq!(
            model_key_command(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
            Some("camera auto-rotate toggle")
        );
        assert_eq!(
            model_key_command(KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE)),
            Some("camera view iso")
        );
        assert_eq!(
            model_key_command(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT)),
            Some("model render")
        );
        assert_eq!(
            model_key_command(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some("model close")
        );
    }

    #[test]
    fn test_q_quits_a_standalone_model_session_through_preview_close() {
        let mut app = App::new();
        app.enter_model_screen();
        app.preview_close_action = crate::app::PreviewCloseAction::Quit;

        handle_key(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            &mut app,
        );

        assert!(app.should_quit);
        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
    }

    #[test]
    fn test_toolbar_and_model_keys_share_the_same_commands() {
        use ratatui::{backend::TestBackend, Terminal};

        let mut app = App::new();
        app.enter_model_screen();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();

        let keys = [
            KeyCode::Char('P'),
            KeyCode::Char('f'),
            KeyCode::Char('p'),
            KeyCode::Char('x'),
            KeyCode::Char('1'),
            KeyCode::Char('5'),
            KeyCode::Char('7'),
            KeyCode::Char(' '),
        ];
        for key in keys {
            let command = model_key_command(KeyEvent::new(key, KeyModifiers::NONE)).unwrap();
            assert!(
                app.ui_regions
                    .camera_buttons
                    .iter()
                    .any(|button| button.command == command),
                "toolbar is missing the key command {command}"
            );
            let command_line = CommandLine::parse(command);
            assert!(app
                .command_registry
                .resolve(&command_line.values())
                .is_some());
        }
    }

    #[test]
    fn test_shortcut_commands_do_not_pollute_user_history() {
        let mut app = App::new();

        execute_shortcut(&mut app, "camera auto-rotate toggle");
        assert!(app.model_preview.auto_rotate);
        assert!(app.command_history.is_empty());

        execute_user_command(&mut app, "camera auto-rotate off");
        assert!(!app.model_preview.auto_rotate);
        assert_eq!(app.command_history, ["camera auto-rotate off"]);
    }

    #[test]
    fn test_model_screen_accepts_camera_commands() {
        let mut app = App::new();
        app.model_preview.status = crate::preview::ModelPreviewStatus::Ready { triangles: 12 };
        app.enter_model_screen();

        handle_key(
            KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.input_mode, InputMode::Command);

        // Command mode takes priority over model shortcuts: `h` edits the command line.
        handle_key(
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.input_buffer.content(), "h");

        app.input_buffer.set_content("camera auto-rotate on");
        handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);

        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.model_preview.auto_rotate);
        assert_eq!(app.command_history, ["camera auto-rotate on"]);
    }

    #[test]
    fn test_model_command_mode_supports_completion_and_escape() {
        let mut app = App::new();
        app.enter_model_screen();
        handle_key(
            KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE),
            &mut app,
        );
        app.input_buffer.set_content("camera projection ");

        handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &mut app);
        assert!(app.completion_active);
        assert!(app
            .completion_candidates
            .iter()
            .any(|candidate| candidate.content == "toggle"));

        handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut app);
        assert_eq!(app.input_mode, InputMode::Command);
        assert!(!app.completion_active);
        handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut app);
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
    }

    #[test]
    fn test_assembly_shortcuts_execute_registered_commands() {
        let mut app = App::new();
        commands::cmd_assembly(&mut app, &["new", "robot"]).unwrap();
        commands::cmd_assembly(&mut app, &["add", "main.scad", "body"]).unwrap();
        commands::cmd_assembly(&mut app, &["add", "main.scad", "arm"]).unwrap();
        app.selected_assembly_part = Some("body".into());

        handle_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.selected_assembly_part.as_deref(), Some("arm"));
        handle_key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut app,
        );
        assert!(app.assembly_preview.auto_rotate);
        assert!(app.assemblies[0].part("arm").unwrap().visible);
        handle_key(
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
            &mut app,
        );
        assert!(!app.assemblies[0].part("arm").unwrap().visible);
        handle_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut app,
        );
        handle_key(
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.selected_assembly_part.as_deref(), Some("arm2"));
        assert!(!app.assemblies[0].part("arm2").unwrap().visible);
        handle_key(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.input_mode, InputMode::Command);
        assert_eq!(app.input_buffer.content(), "assembly rotate arm2 0 0 0");
        handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut app);
        handle_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            &mut app,
        );
        assert!(app.assemblies[0].part("arm2").is_none());
        assert!(app.assemblies[0].part("arm").is_some());
        handle_key(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.screen, crate::app::Screen::Editor);
    }

    #[test]
    fn test_assembly_completion_includes_operations_sources_and_parts() {
        let mut app = App::new();
        commands::cmd_assembly(&mut app, &["new", "robot"]).unwrap();
        commands::cmd_assembly(&mut app, &["add", "main.scad", "body"]).unwrap();

        let operations = generate_completions("assembly ", &app);
        assert!(operations
            .0
            .iter()
            .any(|candidate| candidate.content == "render"));
        let sources = generate_completions("assembly add ", &app);
        assert!(sources
            .0
            .iter()
            .any(|candidate| candidate.content == "main.scad"));
        let parts = generate_completions("assembly translate ", &app);
        assert!(parts.0.iter().any(|candidate| candidate.content == "body"));
        let visibility = generate_completions("assembly visibility ", &app);
        assert!(visibility
            .0
            .iter()
            .any(|candidate| candidate.content == "toggle"));
        let parent = generate_completions("assembly parent ", &app);
        assert!(parent.0.iter().any(|candidate| candidate.content == "root"));
        let copy = generate_completions("assembly copy ", &app);
        assert!(copy.0.iter().any(|candidate| candidate.content == "body"));
        let paste = generate_completions("assembly paste ", &app);
        assert!(paste.0.iter().any(|candidate| candidate.content == "root"));
    }

    #[test]
    fn test_assembly_edit_shortcuts_prefill_current_values() {
        let mut app = App::new();
        commands::cmd_assembly(&mut app, &["new", "robot"]).unwrap();
        commands::cmd_assembly(&mut app, &["add", "main.scad", "arm"]).unwrap();
        commands::cmd_assembly(&mut app, &["translate", "arm", "1", "2", "3"]).unwrap();

        handle_key(
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.input_mode, InputMode::Command);
        assert_eq!(app.input_buffer.content(), "assembly translate arm 1 2 3");
        handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut app);

        handle_key(
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.input_buffer.content(), "assembly parent arm root");
        handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut app);

        handle_key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.input_buffer.content(), "assembly add ");
    }
}
