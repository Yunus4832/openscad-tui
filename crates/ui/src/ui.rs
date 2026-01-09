//! UI rendering module

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use tui_tree_widget::{Tree, TreeItem};
use crate::app::App;

pub fn draw(f: &mut Frame, app: &App) {
    // 主布局：上部是内容区，下部是命令行
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(
            [
                Constraint::Min(15),
                Constraint::Length(3),
            ]
            .as_ref(),
        )
        .split(f.size());

    // 上部内容区：左侧树形图，右侧预览
    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage(30),
                Constraint::Percentage(70),
            ]
            .as_ref(),
        )
        .split(main_chunks[0]);

    // 绘制各个组件
    draw_tree(f, app, content_chunks[0]);
    draw_preview(f, app, content_chunks[1]);
    draw_input(f, app, main_chunks[1]);
}

fn draw_tree(f: &mut Frame, app: &App, area: Rect) {
    let title = if app.selected_nodes.is_empty() {
        " 📁 Tree ".to_string()
    } else {
        format!(" 📁 Tree ({}) ", app.selected_nodes.len())
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Cyan));

    // Build tree items from AST
    let tree_items = build_tree_items(&app.ast.modules, &app.selected_nodes);

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
                .highlight_symbol("📍 ");

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
        "✓ "
    } else {
        "  "
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
    let block = if app.command_mode {
        Block::default()
            .title(" 🔧 Command Mode ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    } else {
        Block::default()
            .title(" ⌨️  Input ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Green))
    };

    let text = if app.command_mode {
        format!(":{}", app.input_buffer)
    } else {
        app.input_buffer.clone()
    };
    
    // 显示错误信息或输入
    let display_text = if let Some(ref error) = app.error_message {
        format!("❌ {}", error)
    } else {
        text
    };

    let paragraph = Paragraph::new(display_text)
        .block(block)
        .style(if app.error_message.is_some() {
            Style::default().fg(Color::Red)
        } else if app.command_mode {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        });

    f.render_widget(paragraph, area);
}

fn draw_preview(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" 📄 Preview ")
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
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::Cyan),
                ),
            ])
        })
        .collect();

    let paragraph = Paragraph::new(visible_lines)
        .block(block)
        .scroll((0, 0));
    
    f.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_state_initialization() {
        let app = crate::app::App::new();
        // TreeState starts empty
        assert!(app.tree_state.borrow().selected().is_empty());
    }
}
