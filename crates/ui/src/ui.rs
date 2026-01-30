//! UI rendering module

use crate::app::App;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use tui_tree_widget::{Tree, TreeItem};

pub fn draw(f: &mut Frame, app: &App) {
    // 主布局：上部是内容区，下部是命令行
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(0)
        .constraints([Constraint::Min(10), Constraint::Length(4)].as_ref())
        .split(f.area());

    // 上部内容区：左侧树形图，右侧预览
    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
        .split(main_chunks[0]);

    // 绘制各个组件
    draw_tree(f, app, content_chunks[0]);
    draw_preview(f, app, content_chunks[1]);

    // Draw completion popup if active
    if app.completion_active && !app.completion_candidates.is_empty() {
        draw_completion_popup(f, app, main_chunks[1]);
    }

    draw_input(f, app, main_chunks[1]);

    // 如果在帮助模式，绘制帮助弹窗
    if app.input_mode == crate::app::InputMode::Help {
        draw_help_modal(f, app);
    }
}

fn draw_tree(f: &mut Frame, app: &App, area: Rect) {
    let current_file = &app.current_file.clone().unwrap_or("Untitled".to_string());
    let unsaved_flag = if app.saved { "" } else { "*" };
    let title = if app.selected_nodes.is_empty() {
        format!(" {}{} ", current_file, unsaved_flag)
    } else {
        format!(
            " {}{} ({}) ",
            current_file,
            unsaved_flag,
            app.selected_nodes.len()
        )
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Cyan));

    // Build tree items from AST (includes all sections)
    let tree_items = build_ast_tree_items(&app.ast, &app.selected_nodes);

    // Create tree widget
    match Tree::new(&tree_items) {
        Ok(tree) => {
            let tree_widget = tree
                .block(block)
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )
                .node_open_symbol("~ ")
                .node_closed_symbol("> ");

            // Render stateful tree using RefCell's borrow_mut
            f.render_stateful_widget(tree_widget, area, &mut app.tree_state.borrow_mut());
        }
        Err(_) => {
            let para = Paragraph::new("Failed to render tree")
                .block(block)
                .style(Style::default().fg(Color::Red));
            f.render_widget(para, area);
        }
    }
}

/// Build TreeItems from entire AST (all sections)
fn build_ast_tree_items(
    ast: &openscad_core::AstRoot,
    selected: &[String],
) -> Vec<TreeItem<'static, String>> {
    let mut items = Vec::new();

    // Includes section
    if !ast.includes.is_empty() {
        let include_children: Vec<TreeItem<String>> = ast
            .includes
            .iter()
            .enumerate()
            .map(|(i, inc)| {
                let id = format!("__include_{}", i);
                TreeItem::new(id, inc.to_string(), vec![]).expect("Failed to create TreeItem")
            })
            .collect();
        items.push(
            TreeItem::new(
                "__includes".to_string(),
                "[Includes]".to_string(),
                include_children,
            )
            .expect("Failed to create TreeItem"),
        );
    }

    // Uses section
    if !ast.uses.is_empty() {
        let use_children: Vec<TreeItem<String>> = ast
            .uses
            .iter()
            .enumerate()
            .map(|(i, u)| {
                let id = format!("__use_{}", i);
                TreeItem::new(id, u.to_string(), vec![]).expect("Failed to create TreeItem")
            })
            .collect();
        items.push(
            TreeItem::new("__uses".to_string(), "[Uses]".to_string(), use_children)
                .expect("Failed to create TreeItem"),
        );
    }

    // Global Variables section
    if !ast.global_variables.is_empty() {
        let var_children: Vec<TreeItem<String>> = ast
            .global_variables
            .iter()
            .map(|var| {
                let id = format!(
                    "__var_{}_{}",
                    if var.is_special { "s" } else { "n" },
                    var.name
                );
                let display = if var.is_special {
                    format!("${} = {}", var.name, var.value.to_scad())
                } else {
                    format!("{} = {}", var.name, var.value.to_scad())
                };
                TreeItem::new(id, display, vec![]).expect("Failed to create TreeItem")
            })
            .collect();
        items.push(
            TreeItem::new(
                "__globals".to_string(),
                "[Global Variables]".to_string(),
                var_children,
            )
            .expect("Failed to create TreeItem"),
        );
    }

    // Function Definitions section
    if !ast.function_defines.is_empty() {
        let func_children: Vec<TreeItem<String>> = ast
            .function_defines
            .iter()
            .map(|func| {
                let id = format!("__func_{}", func.name);
                let params = func
                    .parameters
                    .iter()
                    .map(|p| p.to_scad())
                    .collect::<Vec<_>>()
                    .join(", ");
                let display = format!("function {}({})", func.name, params);
                TreeItem::new(id, display, vec![]).expect("Failed to create TreeItem")
            })
            .collect();
        items.push(
            TreeItem::new(
                "__functions".to_string(),
                "[Functions]".to_string(),
                func_children,
            )
            .expect("Failed to create TreeItem"),
        );
    }

    // Module Definitions section
    if !ast.module_defines.is_empty() {
        let mod_def_children: Vec<TreeItem<String>> = ast
            .module_defines
            .iter()
            .map(|mod_def| {
                let id = format!("__moddef_{}", mod_def.name);
                let params = mod_def
                    .parameters
                    .iter()
                    .map(|p| p.to_scad())
                    .collect::<Vec<_>>()
                    .join(", ");
                let display = format!("module {}({})", mod_def.name, params);
                // Build children from module definition body
                let body_children = build_tree_items(&mod_def.body, selected);
                TreeItem::new(id, display, body_children).expect("Failed to create TreeItem")
            })
            .collect();
        items.push(
            TreeItem::new(
                "__moddefs".to_string(),
                "[Module Definitions]".to_string(),
                mod_def_children,
            )
            .expect("Failed to create TreeItem"),
        );
    }

    // Modules section (module instantiations)
    if !ast.modules.is_empty() {
        let module_children = build_tree_items(&ast.modules, selected);
        items.push(
            TreeItem::new(
                "__modules".to_string(),
                "[Modules]".to_string(),
                module_children,
            )
            .expect("Failed to create TreeItem"),
        );
    }

    items
}

/// Build TreeItems from AST modules
fn build_tree_items(
    modules: &[openscad_core::ModuleNode],
    selected: &[String],
) -> Vec<TreeItem<'static, String>> {
    modules
        .iter()
        .map(|module| build_tree_item(module, selected))
        .collect()
}

/// Build a single TreeItem with children
fn build_tree_item(
    module: &openscad_core::ModuleNode,
    selected: &[String],
) -> TreeItem<'static, String> {
    let marker = if selected.contains(&module.id) {
        "*"
    } else {
        ""
    };

    let text = format!("{}{}", marker, module.get_display_name());
    let id = module.id.clone();

    let children: Vec<TreeItem<String>> = module
        .children
        .iter()
        .map(|child| build_tree_item(child, selected))
        .collect();

    TreeItem::new(id, text, children).expect("Failed to create TreeItem")
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    use crate::app::InputMode;

    let title: String;
    let prompt: String;
    let style_fg: Color;

    match app.input_mode {
        InputMode::Normal => {
            title = " Normal Mode ".to_string();
            prompt = "i=insert  j/k=nav  h/l=collapse/expand  v=select  d=delete  u=undo  <c-r>=redo  enter=toggle  w=write  e=edit  :=cmd  ?=help  q=quit".to_string();
            style_fg = Color::Yellow;
        }
        InputMode::Command => {
            title = " Command Mode ".to_string();
            prompt = "Enter command (type help for commands, Esc to exit):".to_string();
            style_fg = Color::Green;
        }
        InputMode::InsertEnterParams => {
            title = " Insert Parameters ".to_string();
            prompt = format!(
                "Parameters for '{}': ",
                app.insert_module_name.as_deref().unwrap_or("?")
            );
            style_fg = Color::Cyan;
        }
        InputMode::ReplaceSelectModule => {
            title = " Replace Module ".to_string();
            prompt = "Enter replacement module name: ".to_string();
            style_fg = Color::Yellow;
        }
        InputMode::Help => {
            title = " Help ".to_string();
            prompt = "Press Esc or q to close".to_string();
            style_fg = Color::Cyan;
        }
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().fg(style_fg));

    // Create line-by-line content with proper styling
    let mut lines: Vec<Line> = Vec::new();

    // Add prompt line
    lines.push(Line::from(vec![Span::styled(
        prompt.clone(),
        Style::default().fg(style_fg),
    )]));

    // Add input line (only in command/param modes)
    if app.input_mode == InputMode::Command || app.input_mode == InputMode::InsertEnterParams {
        let cursor_pos = app.input_buffer.cursor_pos();
        let buffer = app.input_buffer.content();

        // Build spans for input buffer with cursor highlighting
        let mut spans = vec![Span::styled(
            "> ",
            Style::default().fg(style_fg).add_modifier(Modifier::BOLD),
        )];

        // Add each character with appropriate styling
        for (i, ch) in buffer.chars().enumerate() {
            if i == cursor_pos {
                // Cursor is on this character - highlight with different background
                spans.push(Span::styled(
                    ch.to_string(),
                    Style::default().fg(Color::White).bg(Color::DarkGray),
                ));
            } else {
                // Normal character
                spans.push(Span::styled(
                    ch.to_string(),
                    Style::default().fg(Color::White),
                ));
            }
        }

        // Handle cursor at end of buffer (after all characters)
        let char_count = buffer.chars().count();
        if cursor_pos == char_count {
            // Cursor at end - show a space with background
            spans.push(Span::styled(
                " ",
                Style::default().fg(Color::White).bg(Color::DarkGray),
            ));
        } else if cursor_pos > char_count {
            // Cursor out of bounds (shouldn't happen due to clamp_cursor, but handle gracefully)
            spans.push(Span::styled(
                " ",
                Style::default().fg(Color::White).bg(Color::DarkGray),
            ));
        }

        lines.push(Line::from(spans));
    }

    // Add error line if there's a message
    if let Some(ref msg) = app.message {
        let msg_color = match app.message_type {
            crate::app::MessageType::Error => Color::Red,
            crate::app::MessageType::Warning => Color::Yellow,
            crate::app::MessageType::Info => Color::Green,
        };
        lines.push(Line::from(vec![Span::styled(
            msg.clone(),
            Style::default().fg(msg_color),
        )]));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .style(Style::default().fg(style_fg));

    f.render_widget(paragraph, area);
}

fn draw_preview(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" Preview ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Green));

    let code = app.ast.to_scad();
    let lines: Vec<&str> = code.lines().collect();

    // 计算可显示的行数
    let visible_height = area.height.saturating_sub(2) as usize;
    let total_lines = lines.len();

    // 确保光标在可见范围内
    let preview_offset = if app.preview_offset >= total_lines {
        (total_lines).saturating_sub(visible_height)
    } else {
        app.preview_offset
    };

    let visible_lines: Vec<Line> = lines
        .iter()
        .skip(preview_offset)
        .take(visible_height)
        .enumerate()
        .map(|(i, line)| {
            let line_num = preview_offset + i + 1;
            let line_num_str = format!("{:3} ", line_num);
            Line::from(vec![
                Span::styled(
                    line_num_str,
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(line.to_string(), Style::default().fg(Color::Cyan)),
            ])
        })
        .collect();

    let paragraph = Paragraph::new(visible_lines).block(block).scroll((0, 0));

    f.render_widget(paragraph, area);
}

fn draw_help_modal(f: &mut Frame, app: &App) {
    let cloned_help_docs = app.help_doc.clone();
    let doc_count = app.help_doc_count;
    let modal_width = app.help_modal_width as u16;
    let modal_height = app.help_modal_height as u16;
    let scroll_offset = app.help_scroll_offset;
    let visible_line = app.help_modal_height - 2; // 减去边框

    // Create a centered modal area
    let area = f.area();
    let modal_x = (area.width.saturating_sub(modal_width)) / 2;
    let modal_y = (area.height.saturating_sub(modal_height)) / 2;

    let modal_area = Rect {
        x: modal_x,
        y: modal_y,
        width: modal_width,
        height: modal_height,
    };

    // Clear the background area first
    f.render_widget(Clear, modal_area);

    // Create help content with all available commands
    let help_content: Vec<Line> = cloned_help_docs
        .iter()
        .map(|doc| Line::from(doc.as_str()))
        .collect();

    // Get the visible portion of help content
    let visible_content: Vec<Line> = help_content
        .iter()
        .skip(scroll_offset)
        .take(visible_line)
        .cloned()
        .collect();

    // Create block for modal
    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Cyan));

    let modal = Paragraph::new(visible_content)
        .block(block)
        .style(Style::default().fg(Color::White));

    f.render_widget(modal, modal_area);

    // Add scroll indicator if there's more content
    if doc_count > visible_line {
        let scroll_info = format!(
            "({}/{})",
            (scroll_offset + visible_line).min(doc_count),
            doc_count
        );
        let scroll_text = Paragraph::new(Line::from(scroll_info)).style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        );

        // Position the scroll indicator at the bottom-right of the modal
        let scroll_area = Rect {
            x: modal_area.x + modal_area.width - 10,
            y: modal_area.y + modal_area.height - 1,
            width: 10,
            height: 1,
        };
        f.render_widget(scroll_text, scroll_area);
    }
}

/// Draw completion popup above the input area
fn draw_completion_popup(f: &mut Frame, app: &App, input_area: Rect) {
    use ratatui::widgets::{Clear, List, ListItem, ListState};

    if app.completion_candidates.is_empty() {
        return;
    }

    // Calculate popup dimensions
    let max_width = 30;
    let height = std::cmp::min(app.completion_candidates.len() as u16 + 2, 10); // Max 10 items

    // Position popup above input area
    let popup_width = std::cmp::min(max_width, input_area.width.saturating_sub(2));
    let popup_x = input_area.x;
    let popup_y = input_area.y.saturating_sub(height);

    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height,
    };

    // Clear the area first
    f.render_widget(Clear, popup_area);

    // Create list items
    let items: Vec<ListItem> = app
        .completion_candidates
        .iter()
        .enumerate()
        .map(|(i, candidate)| {
            let prefix = if i == app.completion_index {
                "> "
            } else {
                "  "
            };
            let content = format!(
                "{}{:<width$.width$} [{}]",
                prefix,
                candidate.content,
                candidate.candidate_type.flag(),
                width = (popup_width - 10) as usize
            );
            ListItem::new(content)
        })
        .collect();

    // Create list widget
    let list = List::new(items)
        .block(
            Block::default()
                .title(" Completions ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Cyan)),
        )
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    // Create a temporary list state to highlight the selected item
    let mut list_state = ListState::default();
    list_state.select(Some(app.completion_index));

    f.render_stateful_widget(list, popup_area, &mut list_state);
}

#[cfg(test)]
mod tests {
    use crate::app::App;

    #[test]
    fn test_tree_state_with_empty_ast() {
        let app = App::new();
        // TreeState should be empty when AST has no modules
        assert!(app.tree_state.borrow().selected().is_empty());
    }

    #[test]
    fn test_navigation_status_update() {
        let mut app = App::new();
        // Test that update_navigation_status works without panicking
        app.update_navigation_status();
        // When there's no selection, message should be None
        assert!(app.message.is_none());
    }
}
