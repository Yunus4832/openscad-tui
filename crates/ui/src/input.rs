//! Input handling module - Two modes: Normal and Command
//!
//! Normal mode: Quick keybindings for common operations (i/j/k/h/l/v)
//! Command mode: Free text input for complex commands with parameter input

use crate::app::{
    App, CandidateType, CompletionCandidate, CompletionContext, ExpressionCompletionKind,
    InputMode, PendingModuleAction,
};
use crate::command_registry::CommandType;
use crate::commands;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use openscad_core::ModuleNode;
use std::fs;
use std::path::{Component, Path, PathBuf};

pub fn handle_key(key: KeyEvent, app: &mut App) {
    match app.input_mode {
        InputMode::Normal => handle_normal_input(key, app),
        InputMode::Command => handle_command_input(key, app),
        InputMode::ModuleEnterParams => handle_module_params_input(key, app),
        InputMode::Help => handle_help_input(key, app),
        InputMode::Camera => handle_camera_input(key, app),
    }
}

/// Normal mode: Quick keybindings
fn handle_normal_input(key: KeyEvent, app: &mut App) {
    match key.code {
        // i - insert module (mapped to :insert command)
        KeyCode::Char('i') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("insert ");
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

        // Vim-style structural editing
        KeyCode::Char('y') => {
            execute_command(app, "yank");
        }
        KeyCode::Char('p') => {
            execute_command(app, "paste");
        }
        KeyCode::Char('x') => {
            execute_command(app, "remove");
        }
        KeyCode::Char('c') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("replace ");
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
            app.input_buffer.set_content("rotate ");
        }

        // d - delete node
        KeyCode::Char('d') => {
            execute_command(app, "delete");
        }

        // w - write (save to JSON)
        KeyCode::Char('w') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("write ");
        }

        // e - edit (load from JSON)
        KeyCode::Char('e') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("edit ");
        }

        // L - library (load library JSON)
        KeyCode::Char('L') => {
            app.input_mode = InputMode::Command;
            app.input_buffer.set_content("library ");
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

        // M - enter direct model camera mode
        KeyCode::Char('M') => {
            app.model_preview.mode = crate::preview::PreviewMode::Model;
            app.input_mode = InputMode::Camera;
        }

        _ => {}
    }
}

fn handle_camera_input(key: KeyEvent, app: &mut App) {
    use openscad_render::{Projection, StandardView};

    let result = match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.input_mode = InputMode::Normal;
            return;
        }
        KeyCode::Char('h') => app.model_preview.orbit(-5.0, 0.0),
        KeyCode::Char('l') => app.model_preview.orbit(5.0, 0.0),
        KeyCode::Char('j') => app.model_preview.orbit(0.0, -5.0),
        KeyCode::Char('k') => app.model_preview.orbit(0.0, 5.0),
        KeyCode::Left => app.model_preview.pan(-0.05, 0.0),
        KeyCode::Right => app.model_preview.pan(0.05, 0.0),
        KeyCode::Up => app.model_preview.pan(0.0, 0.05),
        KeyCode::Down => app.model_preview.pan(0.0, -0.05),
        KeyCode::Char('+') | KeyCode::Char('=') => app.model_preview.zoom(0.85),
        KeyCode::Char('-') => app.model_preview.zoom(1.15),
        KeyCode::Char('f') => app.model_preview.fit(),
        KeyCode::Char('p') => {
            let use_orthographic = matches!(
                app.model_preview.camera.projection,
                Projection::Perspective { .. }
            );
            app.model_preview.set_projection(use_orthographic)
        }
        KeyCode::Char(' ') => {
            app.model_preview.auto_rotate = !app.model_preview.auto_rotate;
            Ok(())
        }
        KeyCode::Char('1') => app.model_preview.set_view(StandardView::Front),
        KeyCode::Char('2') => app.model_preview.set_view(StandardView::Back),
        KeyCode::Char('3') => app.model_preview.set_view(StandardView::Left),
        KeyCode::Char('4') => app.model_preview.set_view(StandardView::Right),
        KeyCode::Char('5') => app.model_preview.set_view(StandardView::Top),
        KeyCode::Char('6') => app.model_preview.set_view(StandardView::Bottom),
        KeyCode::Char('7') => app.model_preview.set_view(StandardView::Isometric),
        _ => return,
    };
    if let Err(error) = result {
        app.set_error(&error);
    } else {
        app.clear_error();
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
        let handler = cmd_def.handler;
        let min_args = cmd_def.min_args;
        let max_args = cmd_def.max_args;
        let change_ast = cmd_def.change_ast;
        let write_to_history = cmd_def.write_to_history;

        if write_to_history {
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

        // Execute the command
        match handler(app, args) {
            Ok(_) => {
                if change_ast {
                    app.mark_dirty();
                }
                app.set_info("");
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
    // Add unknown command to history so user can recall and edit it
    app.add_to_history(cmd);
    true
}

fn execute_command(app: &mut App, cmd: &str) {
    execute_command_registry(app, cmd);

    if app.input_mode == InputMode::Command {
        app.input_mode = InputMode::Normal;
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

    // 按空白字符分割输入
    let parts: Vec<&str> = trimmed.split_whitespace().collect();

    if parts.is_empty() {
        return CompletionContext::Command;
    }

    // 第一部分是命令
    let command = parts[0];

    // 使用命令注册表查找命令类型
    if let Some(cmd_def) = app.command_registry.find(command) {
        match &cmd_def.cmd_type {
            CommandType::File => {
                // 文件命令处理逻辑
                if parts.len() == 1 {
                    if input.ends_with(' ') {
                        CompletionContext::File {
                            current_path: String::new(),
                            base_dir: ".".to_string(),
                            partial_name: String::new(),
                            ends_with_separator: false,
                        }
                    } else {
                        CompletionContext::Command
                    }
                } else {
                    // 有路径部分
                    let path_part = parts[1..].join(" ").trim_end().to_string();
                    let ends_with_separator = path_part.ends_with('/');

                    // 解析路径，分离目录部分和文件名部分
                    let (base_dir, partial_name) = if path_part.contains('/') {
                        let last_slash = path_part.rfind('/').unwrap();
                        let base = &path_part[..last_slash + 1];
                        let partial = &path_part[last_slash + 1..];

                        // 处理相对路径和波浪号（~）
                        let normalized_base = if base.starts_with('/') {
                            // 绝对路径
                            base.to_string()
                        } else if let Some(stripped) = base.strip_prefix("~/") {
                            // 处理波浪号（~）表示 home 目录
                            if let Ok(home_dir) = std::env::var("HOME") {
                                format!("{}/{}", home_dir, stripped)
                            } else {
                                // 如果无法获取 home 目录，保持原样
                                base.to_string()
                            }
                        } else {
                            // 相对路径，需要与当前目录结合
                            if base == "./" || base.is_empty() {
                                ".".to_string()
                            } else {
                                normalize_path(base)
                            }
                        };

                        (normalized_base, partial.to_string())
                    } else {
                        // 没有分隔符，整个都是文件名部分
                        if path_part == "~" {
                            // 如果整个路径是 ~，转换为 home 目录
                            if let Ok(home_dir) = std::env::var("HOME") {
                                (home_dir, String::new())
                            } else {
                                (".".to_string(), path_part.clone())
                            }
                        } else {
                            (".".to_string(), path_part.clone())
                        }
                    };

                    // 检查完整路径是否存在且为文件，如果是且输入以空格结尾，切换回命令上下文
                    let full_path = Path::new(&path_part);
                    if input.ends_with(' ')
                        && !input.ends_with("/ ")
                        && full_path.exists()
                        && full_path.is_file()
                    {
                        // 用户已指定一个存在的文件并添加了空格，意味着完成文件选择
                        return CompletionContext::Command;
                    }

                    CompletionContext::File {
                        current_path: path_part,
                        base_dir,
                        partial_name,
                        ends_with_separator,
                    }
                }
            }
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
            CommandType::FunctionDefinition => input
                .find('=')
                .map(|_| CompletionContext::ExpressionValue {
                    kind: ExpressionCompletionKind::FunctionBody,
                    local_identifiers: function_definition_parameters(input),
                })
                .unwrap_or(CompletionContext::Command),
            CommandType::ModuleDefinition => CompletionContext::Command,
            CommandType::GlobalDefinition => input
                .find('=')
                .map(|_| CompletionContext::ExpressionValue {
                    kind: ExpressionCompletionKind::GlobalValue,
                    local_identifiers: Vec::new(),
                })
                .unwrap_or(CompletionContext::Command),
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
            CommandType::Preview => {
                literal_command_context(input, &parts, &["source", "model"], &[])
            }
            CommandType::Camera => {
                let second_level: &[&str] = match parts.get(1).copied() {
                    Some("projection") => &["perspective", "orthographic"],
                    Some("view") => &["front", "back", "left", "right", "top", "bottom", "iso"],
                    Some("auto-rotate") => &["on", "off"],
                    _ => &[],
                };
                literal_command_context(
                    input,
                    &parts,
                    &[
                        "projection",
                        "view",
                        "orbit",
                        "pan",
                        "zoom",
                        "fit",
                        "auto-rotate",
                    ],
                    second_level,
                )
            }
        }
    } else {
        CompletionContext::Command
    }
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
        CompletionContext::NodeParam | CompletionContext::NodeParamUnset => {
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
            kind,
            local_identifiers,
        } => {
            let candidates = expression_candidates(
                app,
                local_identifiers,
                None,
                matches!(kind, ExpressionCompletionKind::FunctionBody),
            );
            let value_source = input.split_once('=').map(|(_, value)| value).unwrap_or("");
            let (start, end) = value_fragment_range(value_source, 0, value_source.len());
            filter_by_prefix(&candidates, value_source[start..end].trim())
        }
        CompletionContext::File {
            base_dir,
            partial_name,
            ..
        } => {
            // 文件补全 - 使用基础目录和部分名称
            let candidates: Vec<CompletionCandidate> = get_file_completions(base_dir, partial_name)
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
            let prefix = input.split_whitespace().last().unwrap_or("");
            let prefix = if input.ends_with(' ') { "" } else { prefix };
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
        CompletionContext::File {
            current_path: _,
            base_dir: _,
            partial_name: _,
            ends_with_separator,
        } => {
            // 文件补全：替换路径部分的最后部分
            // 根据上下文决定替换范围
            // 查找最后一个 / 的位置
            if let Some(slash_pos) = input.rfind('/') {
                if *ends_with_separator {
                    // 如果路径以 / 结尾，从 / 位置之后开始替换
                    (slash_pos + 1, input.len())
                } else {
                    // 查找最后一个空格的位置
                    if let Some(space_pos) = input.rfind(' ') {
                        if slash_pos > space_pos {
                            // 最后一个是 / 在最后的空格之后，从 / 位置之后开始替换
                            (slash_pos + 1, input.len())
                        } else {
                            // 最后一个空格在最后的 / 之后，从空格之后开始替换
                            (space_pos + 1, input.len())
                        }
                    } else {
                        // 没有空格，从 / 之后开始替换
                        (slash_pos + 1, input.len())
                    }
                }
            } else {
                // 没有 /，按原逻辑处理（查找空格）
                let space_pos = input.rfind(' ');
                let path_start = if let Some(pos) = space_pos {
                    pos + 1 // 从空格后开始
                } else {
                    0 // 如果没有空格，从开头开始
                };
                (path_start, input.len()) // 替换从路径开始到末尾的所有内容
            }
        }
        CompletionContext::Literal { .. } => {
            if input.ends_with(' ') {
                (input.len(), input.len())
            } else {
                let token_index = input.split_whitespace().count().saturating_sub(1);
                whitespace_token_range(input, token_index).unwrap_or((input.len(), input.len()))
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
    app.input_buffer
        .replace_range(start, end, &candidate.content);

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
    app.command_registry.get_all_names()
}

/// Get list of available modules from library
fn get_module_list(app: &App) -> Vec<String> {
    app.library.get_module_names()
}

/// Get file completions for a given directory and path prefix
/// Returns entries in the directory that match the prefix (without trailing '/')
fn get_file_completions(dir_path: &str, prefix: &str) -> Vec<String> {
    let mut completions = Vec::new();

    // Parse the directory path
    let dir = Path::new(dir_path);

    // Try to read the directory
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(_file_type) = entry.file_type() {
                // Using underscore to indicate unused
                if let Ok(file_name) = entry.file_name().into_string() {
                    // Check if it matches the partial name
                    if file_name.starts_with(prefix) {
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
        let (preview, _) = generate_completions("preview ", &app);
        assert_eq!(
            preview
                .iter()
                .map(|candidate| candidate.content.as_str())
                .collect::<Vec<_>>(),
            vec!["source", "model"]
        );

        let (camera, _) = generate_completions("camera view ", &app);
        assert!(camera.iter().any(|candidate| candidate.content == "iso"));
        assert!(camera.iter().any(|candidate| candidate.content == "front"));

        let (_, analysis) = generate_completions("camera projection per", &app);
        assert_eq!(analysis.replacement_range, (18, 21));
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
            .any(|candidate| candidate.content == "size"));

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
    fn test_global_value_completion_is_intentionally_simple() {
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
        assert!(!function_candidates
            .iter()
            .any(|candidate| candidate.candidate_type == CandidateType::Function));
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
        assert_eq!(app.node_clipboard.as_ref().unwrap().id, "cube_1");

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
}
