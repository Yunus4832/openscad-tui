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

        KeyCode::Enter => {
            if app.completion_active {
                apply_completion(app);
            } else {
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
            if app.completion_active {
                apply_completion(app);
            } else {
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
    execute_command_registry(app, cmd);

    if app.input_mode == InputMode::Command {
        app.input_mode = InputMode::Normal;
    }
}

/// Handle Tab key for autocompletion
fn handle_tab_completion(app: &mut App) {
    if !app.completion_active {
        let (candidates, context) = generate_completions(&app.input_buffer, app);
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
fn analyze_input_context(input: &str, app: &App) -> crate::app::CompletionContext {
    let trimmed = input.trim();

    // 检查是否为 InsertEnterParams 模式
    if app.input_mode == crate::app::InputMode::InsertEnterParams {
        // 在 InsertEnterParams 模式下，输入只包含参数字符串
        // 模块名存储在 app.insert_module_name 中
        if let Some(ref module_name) = app.insert_module_name {
            return analyze_param_context(trimmed, module_name);
        } else {
            // 如果没有模块名，返回默认上下文
            return crate::app::CompletionContext::ModuleParam {
                module_name: String::new(),
                param_index: 0,
            };
        }
    }

    // 正常命令模式（以下为原有逻辑）

    // 空输入或只有空白字符：命令补全
    if trimmed.is_empty() {
        return crate::app::CompletionContext::Command;
    }

    // 按空白字符分割输入
    let parts: Vec<&str> = trimmed.split_whitespace().collect();

    // 第一部分是命令
    let command = parts[0];

    // 检查是否为文件相关命令
    let file_commands = ["write", "edit", "library"];
    if file_commands.contains(&command) {
        // 文件补全上下文
        if parts.len() == 1 {
            // 只输入了命令，没有路径
            if input.ends_with(' ') {
                // 命令后跟空格，等待文件路径
                return crate::app::CompletionContext::File {
                    current_path: String::new(),
                    current_dir: ".".to_string(),
                    at_path_end: false,
                };
            } else {
                // 仍在命令补全上下文（但命令已确定，可能不需要补全）
                return crate::app::CompletionContext::Command;
            }
        } else {
            // 有路径部分
            let path_part = parts[1..].join(" ");
            let current_dir = ".".to_string();
            let at_path_end = input.ends_with(' ') || input.ends_with('/');

            return crate::app::CompletionContext::File {
                current_path: path_part,
                current_dir,
                at_path_end,
            };
        }
    }

    if parts.len() == 1 {
        // 如果输入以空格结尾，则已经输入了命令，进入模块补全上下文
        if input.ends_with(' ') {
            return crate::app::CompletionContext::Module;
        } else {
            // 否则仍在命令补全上下文
            return crate::app::CompletionContext::Command;
        }
    }

    // 至少有命令和模块名（或部分模块名）
    let module_part = parts[1];

    // 如果只有命令和模块名，没有参数部分
    if parts.len() == 2 {
        // 检查输入是否以空格结尾：如果是，则进入模块参数补全上下文
        if input.ends_with(' ') {
            // 已经输入了模块名和空格，等待参数
            return crate::app::CompletionContext::ModuleParam {
                module_name: module_part.to_string(),
                param_index: 0,
            };
        } else {
            // 仍在模块补全上下文
            return crate::app::CompletionContext::Module;
        }
    }

    // 有参数部分（第三个及之后的单词）
    // 参数部分是一个整体，用逗号分隔的 name=value 对
    let param_str = parts[2..].join(" ");

    analyze_param_context(&param_str, module_part)
}

/// 分析参数字符串上下文（用于正常模式和 InsertEnterParams 模式）
fn analyze_param_context(param_str: &str, module_name: &str) -> crate::app::CompletionContext {
    // 解析参数字符串以确定当前上下文
    // 查找最后一个逗号、等号的位置
    let last_comma = param_str.rfind(',');
    let last_equal = param_str.rfind('=');

    // 确定当前是在参数名、等号后，还是值之后
    match (last_comma, last_equal) {
        (None, None) => {
            // 没有逗号也没有等号：正在输入第一个参数名
            crate::app::CompletionContext::ModuleParam {
                module_name: module_name.to_string(),
                param_index: 0,
            }
        }
        (Some(comma_pos), None) => {
            // 有逗号但没有等号（在逗号之后）：正在输入下一个参数名
            // 计算已经输入了多少个参数（逗号数量）
            let param_count = param_str[..=comma_pos].matches(',').count();
            crate::app::CompletionContext::ModuleParam {
                module_name: module_name.to_string(),
                param_index: param_count,
            }
        }
        (None, Some(equal_pos)) => {
            // 有等号但没有逗号：正在输入第一个参数的值
            // 提取参数名
            let param_name = param_str[..equal_pos].trim().to_string();
            crate::app::CompletionContext::ModuleParamValue {
                module_name: module_name.to_string(),
                module_param_name: param_name,
                value_index: 0,
            }
        }
        (Some(comma_pos), Some(equal_pos)) => {
            if comma_pos > equal_pos {
                // 最后一个逗号在等号之后：参数值已输入完成，等待下一个参数
                let param_count = param_str[..=comma_pos].matches(',').count();
                crate::app::CompletionContext::ModuleParam {
                    module_name: module_name.to_string(),
                    param_index: param_count,
                }
            } else {
                // 最后一个等号在逗号之后：正在输入当前参数的值
                // 提取最后一个等号之后的参数名
                let after_last_comma = param_str[comma_pos + 1..].trim();
                if let Some(param_equal_pos) = after_last_comma.find('=') {
                    let param_name = after_last_comma[..param_equal_pos].trim().to_string();
                    crate::app::CompletionContext::ModuleParamValue {
                        module_name: module_name.to_string(),
                        module_param_name: param_name,
                        value_index: 0,
                    }
                } else {
                    // 应该不会发生这种情况
                    crate::app::CompletionContext::ModuleParam {
                        module_name: module_name.to_string(),
                        param_index: param_str.matches(',').count(),
                    }
                }
            }
        }
    }
}

/// 根据前缀过滤字符串列表
fn filter_by_prefix(items: &[String], prefix: &str) -> Vec<String> {
    if prefix.is_empty() {
        return items.to_vec();
    }
    items
        .iter()
        .filter(|item| item.starts_with(prefix))
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
fn extract_module_and_param_str(app: &App, input: &str) -> (Option<String>, String) {
    if app.input_mode == crate::app::InputMode::InsertEnterParams {
        // InsertEnterParams 模式：模块名在 app.insert_module_name 中
        // 整个输入就是参数字符串
        let module_name = app.insert_module_name.clone();
        let param_str = input.trim().to_string();
        (module_name, param_str)
    } else {
        // 正常命令模式：从输入中提取模块名和参数字符串
        let parts: Vec<&str> = input.split_whitespace().collect();
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
    }
}

/// 生成候选列表
/// 对于命令，从命令列表读取，对于模块，从模块列表读取，对于模块参数名解析模块获取，对于模块参数值，从模块参数默认值和全局变量
/// AstRoot.global_variables 中获取
fn generate_completions(input: &str, app: &App) -> (Vec<String>, crate::app::CompletionContext) {
    let context = analyze_input_context(input, app);

    let candidates = match &context {
        crate::app::CompletionContext::Command => {
            // 命令补全：获取所有命令，过滤以匹配输入前缀
            let all_commands = get_command_list(app);
            let prefix = input.trim();
            filter_by_prefix(&all_commands, prefix)
        }
        crate::app::CompletionContext::Module => {
            // 模块补全：获取所有模块，过滤以匹配输入中的模块部分
            let all_modules = get_module_list(app);
            // 提取可能已输入的部分模块名
            let parts: Vec<&str> = input.split_whitespace().collect();
            let prefix = if parts.len() > 1 {
                parts[1] // 已经输入的部分模块名
            } else {
                "" // 还没有输入模块名
            };
            filter_by_prefix(&all_modules, prefix)
        }
        crate::app::CompletionContext::ModuleParam {
            module_name,
            param_index: _param_index,
        } => {
            // 模块参数补全：获取模块的所有参数，过滤掉已输入的参数
            if let Some(module_def) = app.library.get_module(module_name) {
                // 获取已输入的参数名
                let (_, param_str) = extract_module_and_param_str(app, input);
                let entered_params = parse_parameter_names(&param_str);

                // 过滤掉已输入的参数
                let mut candidates: Vec<String> = module_def
                    .parameters
                    .iter()
                    .map(|p| p.name.clone())
                    .filter(|name| !entered_params.contains(name))
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
        crate::app::CompletionContext::ModuleParamValue {
            module_name,
            module_param_name,
            value_index: _value_index,
        } => {
            // 模块参数值补全：获取参数的默认值（如果存在）和全局变量
            let mut candidates = Vec::new();

            // 首先，尝试获取参数的默认值
            if let Some(module_def) = app.library.get_module(module_name) {
                if let Some(param_def) = module_def
                    .parameters
                    .iter()
                    .find(|p| p.name == *module_param_name)
                {
                    if let Some(default_val) = &param_def.default {
                        candidates.push(default_val.clone());
                    }
                }
            }

            // 添加全局变量
            for var in &app.ast.global_variables {
                candidates.push(var.name.clone());
            }

            // 如果有部分输入的值，进行过滤
            let (_, param_str) = extract_module_and_param_str(app, input);
            let current_value_part = get_current_param_value_part(&param_str, module_param_name);
            if !current_value_part.is_empty() {
                candidates = filter_by_prefix(&candidates, &current_value_part);
            }

            candidates
        }
        crate::app::CompletionContext::File {
            current_path,
            current_dir,
            at_path_end: _at_path_end,
        } => {
            // 文件补全
            get_file_completions(current_dir, current_path)
        }
    };

    (candidates, context)
}

/// 预览选中的候选项, 替换缓冲区中的补全内容
fn preview_completion(app: &mut App) {
    if app.completion_candidates.is_empty() {
        return;
    }

    let candidate = &app.completion_candidates[app.completion_index];
    let (start, end) = get_replacement_range(&app.input_buffer, &app.completion_context, app);

    // 替换输入缓冲区中的范围
    let mut new_input = app.input_buffer.clone();
    new_input.replace_range(start..end, candidate);
    app.input_buffer = new_input;
}

/// 获取输入缓冲区中需要替换的范围（起始索引和结束索引）
fn get_replacement_range(
    input: &str,
    context: &crate::app::CompletionContext,
    app: &App,
) -> (usize, usize) {
    match context {
        crate::app::CompletionContext::Command => {
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
        crate::app::CompletionContext::Module => {
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
        crate::app::CompletionContext::ModuleParam {
            module_name: _module_name,
            param_index: _param_index,
        } => {
            // 模块参数补全：替换当前正在输入的参数名部分
            let (_, param_str) = extract_module_and_param_str(app, input);
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
        crate::app::CompletionContext::ModuleParamValue {
            module_name: _module_name,
            module_param_name,
            value_index: _value_index,
        } => {
            // 模块参数值补全：替换当前参数的值部分
            // 使用 extract_module_and_param_str 获取参数字符串
            let (_, param_str) = extract_module_and_param_str(app, input);

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
        crate::app::CompletionContext::File {
            current_path: _current_path,
            current_dir: _current_dir,
            at_path_end: _at_path_end,
        } => {
            // 文件补全：替换路径部分
            let parts: Vec<&str> = input.split_whitespace().collect();
            if parts.len() < 2 {
                (input.len(), input.len())
            } else {
                let path_part = parts[1..].join(" ");
                let path_start = input.find(&path_part).unwrap_or(input.len());
                (path_start, path_start + path_part.len())
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
    let (start, end) = get_replacement_range(&app.input_buffer, &app.completion_context, app);

    // 替换输入缓冲区中的范围
    let mut new_input = app.input_buffer.clone();
    new_input.replace_range(start..end, candidate);

    // 根据上下文追加分隔符
    match &app.completion_context {
        crate::app::CompletionContext::Command => {
            new_input.push(' ');
        }
        crate::app::CompletionContext::Module => {
            new_input.push(' ');
        }
        crate::app::CompletionContext::ModuleParam { .. } => {
            new_input.push('=');
        }
        crate::app::CompletionContext::ModuleParamValue {
            module_name,
            module_param_name: _,
            value_index: _value_index,
        } => {
            // 检查当前参数是否是最后一个参数, 不是最后一个参数，追加逗号
            let (_, param_str) = extract_module_and_param_str(app, &app.input_buffer);
            if !is_last_parameter(app, module_name, &param_str) {
                new_input.push(',');
            }
        }
        crate::app::CompletionContext::File {
            current_path: _current_path,
            current_dir: _current_dir,
            at_path_end: _at_path_end,
        } => {
            // 如果是目录，追加 "/"，否则追加空格
            // 这里简化处理：如果候选以 "/" 结尾或者是目录，则追加 "/"，否则追加空格
            // 暂时先追加空格
            new_input.push(' ');
        }
    }

    app.input_buffer = new_input;

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
