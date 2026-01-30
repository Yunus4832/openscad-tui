//! Input handling module - Two modes: Normal and Command
//!
//! Normal mode: Quick keybindings for common operations (i/j/k/h/l/v)
//! Command mode: Free text input for complex commands with parameter input

use crate::app::{App, CandidateType, CompletionCandidate, CompletionContext, InputMode};
use crate::command_registry::CommandType;
use crate::commands;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fs;
use std::path::{Component, Path, PathBuf};

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
            app.input_buffer.set_content("insert ");
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

        _ => {}
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
            if let Some(prev_cmd) = app.get_previous_command() {
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
            if let Some(prev_cmd) = app.get_previous_command() {
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
            if let Some(prev_cmd) = app.get_previous_command() {
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
fn handle_insert_params_input(key: KeyEvent, app: &mut App) {
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
            // User finished entering parameters
            if app.completion_active {
                apply_completion(app);
            } else {
                let params = app.input_buffer.content().trim().to_string();
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
    app.clamp_cursor();

    match key.code {
        KeyCode::Char(c) => {
            app.input_buffer.insert_char(c);
        }
        KeyCode::Backspace => {
            app.input_buffer.delete_before_cursor();
        }
        KeyCode::Delete => {
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
        KeyCode::Enter => {
            let _module_name = app.input_buffer.content().trim().to_string();
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
        // Close help modal
        KeyCode::Esc | KeyCode::Char('q') => {
            app.input_mode = InputMode::Normal;
        }
        // Scroll up
        KeyCode::Up | KeyCode::Char('k') => {
            app.help_scroll_offset = app.help_scroll_offset.saturating_sub(1).max(0);
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
            app.help_scroll_offset = app.help_scroll_offset.saturating_sub(10).max(0);
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
        let (candidates, context) = generate_completions(app.input_buffer.content(), app);
        if candidates.is_empty() {
            return;
        }

        app.completion_context = context;
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

/// 分析输入字符串，确定当前补全上下文
fn analyze_input_context(input: &str, app: &App) -> CompletionContext {
    let trimmed = input.trim();

    // 检查是否为 InsertEnterParams 模式
    if app.input_mode == InputMode::InsertEnterParams {
        // 在 InsertEnterParams 模式下，输入只包含参数字符串
        // 模块名存储在 app.insert_module_name 中
        if let Some(ref module_name) = app.insert_module_name {
            return analyze_param_context(trimmed, module_name, CommandType::Module);
        } else {
            // 如果没有模块名，返回默认上下文
            return CompletionContext::ModuleParam {
                cmd_type: CommandType::Module,
                module_name: String::new(),
                param_index: 0,
            };
        }
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
                                param_index: 0,
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
                            param_index: 0,
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
            CommandType::Definition => {
                // 定义命令：无需补全
                CompletionContext::Command
            }
        }
    } else {
        CompletionContext::Command
    }
}

/// 分析参数字符串上下文（用于正常模式和 InsertEnterParams 模式）
fn analyze_param_context(
    param_str: &str,
    module_name: &str,
    cmd_type: CommandType,
) -> CompletionContext {
    // 解析参数字符串以确定当前上下文
    // 查找最后一个逗号、等号的位置
    let last_comma = param_str.rfind(',');
    let last_equal = param_str.rfind('=');

    // 确定当前是在参数名、等号后，还是值之后
    match (last_comma, last_equal) {
        (None, None) => {
            // 没有逗号也没有等号：正在输入第一个参数名
            CompletionContext::ModuleParam {
                cmd_type,
                module_name: module_name.to_string(),
                param_index: 0,
            }
        }
        (Some(comma_pos), None) => {
            // 有逗号但没有等号（在逗号之后）：正在输入下一个参数名
            // 计算已经输入了多少个参数（逗号数量）
            let param_count = param_str[..=comma_pos].matches(',').count();
            CompletionContext::ModuleParam {
                cmd_type,
                module_name: module_name.to_string(),
                param_index: param_count,
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
                value_index: 0,
            }
        }
        (Some(comma_pos), Some(equal_pos)) => {
            if comma_pos > equal_pos {
                // 最后一个逗号在等号之后：参数值已输入完成，等待下一个参数
                let param_count = param_str[..=comma_pos].matches(',').count();
                CompletionContext::ModuleParam {
                    cmd_type,
                    module_name: module_name.to_string(),
                    param_index: param_count,
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
                        value_index: 0,
                    }
                } else {
                    // 应该不会发生这种情况
                    CompletionContext::ModuleParam {
                        cmd_type,
                        module_name: module_name.to_string(),
                        param_index: param_str.matches(',').count(),
                    }
                }
            }
        }
    }
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
    let mut in_brackets = 0;
    let mut last_comma = None;

    for (i, ch) in param_str.chars().enumerate() {
        match ch {
            '[' => in_brackets += 1,
            ']' if in_brackets > 0 => in_brackets -= 1,
            ',' if in_brackets == 0 => {
                last_comma = Some(i);
            }
            _ => {}
        }
    }
    last_comma
}

/// 从指定位置开始查找下一个参数分隔符逗号的位置（忽略括号内的逗号）
fn find_next_param_separator_from(param_str: &str, start: usize) -> Option<usize> {
    let mut in_brackets = 0;
    let chars: Vec<char> = param_str.chars().collect();
    for (i, &ch) in chars.iter().enumerate().skip(start) {
        match ch {
            '[' => in_brackets += 1,
            ']' if in_brackets > 0 => in_brackets -= 1,
            ',' if in_brackets == 0 => {
                return Some(i);
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
        param_str[value_start..end].trim().to_string()
    } else {
        String::new()
    }
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
    if app.input_mode == InputMode::InsertEnterParams {
        // InsertEnterParams 模式：模块名在 app.insert_module_name 中
        // 整个输入就是参数字符串
        let module_name = app.insert_module_name.clone();
        let param_str = input.trim().to_string();
        (module_name, param_str)
    } else {
        // 正常命令模式：从输入中提取模块名和参数字符串
        let parts: Vec<&str> = input.split_whitespace().collect();
        if cmd_type == &CommandType::Module {
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

/// 生成候选列表
/// 对于命令，从命令列表读取，对于模块，从模块列表读取，对于模块参数名解析模块获取，对于模块参数值，从模块参数默认值和全局变量
/// AstRoot.global_variables 中获取
fn generate_completions(input: &str, app: &App) -> (Vec<CompletionCandidate>, CompletionContext) {
    let context = analyze_input_context(input, app);

    let candidates = match &context {
        CompletionContext::Command => {
            // 命令补全：获取所有命令，过滤以匹配输入前缀
            let sep = " ".to_string();
            let all_commands: Vec<CompletionCandidate> = get_command_list(app)
                .iter()
                .map(|c| {
                    CompletionCandidate::new(
                        c.clone(),
                        CandidateType::Command {
                            separator: sep.clone(),
                        },
                    )
                })
                .collect();

            let prefix = input.trim();
            filter_by_prefix(&all_commands, prefix)
        }
        CompletionContext::Module => {
            // 模块补全：获取所有模块，过滤以匹配输入中的模块部分
            let sep = " ".to_string();
            let all_modules: Vec<CompletionCandidate> = get_module_list(app)
                .iter()
                .map(|c| {
                    CompletionCandidate::new(
                        c.clone(),
                        CandidateType::Module {
                            separator: sep.clone(),
                        },
                    )
                })
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
            param_index: _param_index,
        } => {
            // 模块参数补全：获取模块的所有参数，过滤掉已输入的参数
            if let Some(module_def) = app.library.get_module(module_name) {
                // 获取已输入的参数名
                let (_, param_str) = extract_module_and_param_str(app, input, _cmd_type);
                let entered_params = parse_parameter_names(&param_str);

                // 过滤掉已输入的参数
                let sep = "=".to_string();
                let mut candidates: Vec<CompletionCandidate> = module_def
                    .parameters
                    .iter()
                    .map(|p| p.name.clone())
                    .filter(|name| !entered_params.contains(name))
                    .map(|c| {
                        CompletionCandidate::new(
                            c.clone(),
                            CandidateType::ModuleParam {
                                separator: sep.clone(),
                            },
                        )
                    })
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
            value_index: _value_index,
        } => {
            // 模块参数值补全：获取参数的默认值（如果存在）和全局变量
            let mut candidates: Vec<CompletionCandidate> = Vec::new();
            let sep = ",".to_string();

            // 首先，尝试获取参数的默认值
            if let Some(module_def) = app.library.get_module(module_name) {
                if let Some(param_def) = module_def
                    .parameters
                    .iter()
                    .find(|p| p.name == *module_param_name)
                {
                    if let Some(default_val) = &param_def.default {
                        candidates.push(CompletionCandidate {
                            content: default_val.clone(),
                            candidate_type: CandidateType::Value {
                                separator: sep.clone(),
                            },
                        });
                    }
                }
            }

            // 添加全局变量
            for var in &app.ast.global_variables {
                candidates.push(CompletionCandidate {
                    content: var.name.clone(),
                    candidate_type: CandidateType::GlobalVar {
                        separator: sep.clone(),
                    },
                });
            }

            // 如果有部分输入的值，进行过滤
            let (_, param_str) = extract_module_and_param_str(app, input, _cmd_type);
            let current_value_part = get_current_param_value_part(&param_str, module_param_name);
            if !current_value_part.is_empty() {
                candidates = filter_by_prefix(&candidates, &current_value_part);
            }
            candidates
        }
        CompletionContext::File {
            base_dir,
            partial_name,
            ..
        } => {
            // 文件补全 - 使用基础目录和部分名称
            let sep = "/".to_string();
            let candidates: Vec<CompletionCandidate> = get_file_completions(base_dir, partial_name)
                .iter()
                .map(|c| {
                    CompletionCandidate::new(
                        c.clone(),
                        CandidateType::Path {
                            separator: sep.clone(),
                        },
                    )
                })
                .collect();
            candidates
        }
    };

    (candidates, context)
}

/// 预览选中的候选项, 替换缓冲区中的补全内容
fn preview_completion(app: &mut App) {
    if app.completion_candidates.is_empty() {
        return;
    }

    // 替换输入缓冲区中的范围
    let (start, end) =
        get_replacement_range(app.input_buffer.content(), &app.completion_context, app);
    let candidate = match &app.completion_context {
        CompletionContext::File {
            current_path: _,
            base_dir: _,
            partial_name: _,
            ends_with_separator: _,
        } => {
            if app.input_buffer.content().trim().ends_with("~") {
                let candidate_clone = &app.completion_candidates[app.completion_index].clone();
                &format!("{}{}", "~/", candidate_clone.content)
            } else {
                &app.completion_candidates[app.completion_index].content
            }
        }
        _ => &app.completion_candidates[app.completion_index].content,
    };

    // Use InputBuffer's replace_range method
    app.input_buffer.replace_range(start, end, candidate);
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
            let parts: Vec<&str> = input.split_whitespace().collect();
            if parts.len() < 2 {
                // 没有模块名，在末尾替换
                (input.len(), input.len())
            } else {
                // 找到第二个单词在原始输入中的位置
                let module_part = parts[1];
                let module_start = input.find(module_part).unwrap_or(input.len());
                (module_start, module_start + module_part.len())
            }
        }
        CompletionContext::ModuleParam {
            cmd_type: _cmd_type,
            module_name: _module_name,
            param_index: _param_index,
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
            value_index: _value_index,
        } => {
            // 模块参数值补全：替换当前参数的值部分
            // 使用 extract_module_and_param_str 获取参数字符串
            let (_, param_str) = extract_module_and_param_str(app, input, _cmd_type);

            // 找到参数值部分的位置
            let pattern = format!("{}=", module_param_name);
            if let Some(start) = param_str.find(&pattern) {
                let value_start = start + pattern.len();
                let end = find_next_param_separator_from(&param_str, value_start)
                    .unwrap_or(param_str.len());
                // 在原始输入中找到参数字符串的位置
                let param_start = input.find(&param_str).unwrap_or(input.len());
                (param_start + value_start, param_start + end)
            } else {
                (input.len(), input.len())
            }
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
    }
}

/// 应用选中的候选项，并退出补全模式, 对于命令补全，需要追加空格，对于模块补全，需要追加空格
/// 对于模块参数名补全需要追加 "=" 等号，对于模块参数值补全需要追加 "," 逗号
fn apply_completion(app: &mut App) {
    if app.completion_candidates.is_empty() {
        return;
    }

    let candidate = &app.completion_candidates[app.completion_index];
    let (start, end) =
        get_replacement_range(app.input_buffer.content(), &app.completion_context, app);

    // 替换输入缓冲区中的范围
    app.input_buffer
        .replace_range(start, end, &candidate.content);
    let separator = match &candidate.candidate_type {
        CandidateType::Module { separator } => separator,
        CandidateType::ModuleParam { separator } => separator,
        CandidateType::Function { separator } => separator,
        CandidateType::FunctionParam { separator } => separator,
        CandidateType::Path { separator } => separator,
        CandidateType::GlobalVar { separator } => separator,
        CandidateType::Value { separator } => separator,
        CandidateType::Command { separator } => separator,
    };

    // 根据上下文追加分隔符
    match &app.completion_context {
        CompletionContext::ModuleParamValue {
            cmd_type: _cmd_type,
            module_name,
            module_param_name: _,
            value_index: _value_index,
        } => {
            // 检查当前参数是否是最后一个参数, 不是最后一个参数，追加逗号
            let (_, param_str) =
                extract_module_and_param_str(app, app.input_buffer.content(), _cmd_type);
            if !is_last_parameter(app, module_name, &param_str) {
                app.input_buffer.insert_str(separator);
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
                    app.input_buffer.insert_str(separator);
                } else {
                    // 对于文件，追加空格
                    app.input_buffer.insert_str(" ");
                }
            } else {
                // 如果无法获取元数据，默认追加空格
                app.input_buffer.insert_str(" ");
            }
        }
        _ => {
            app.input_buffer.insert_str(separator);
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
