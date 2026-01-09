//! UI rendering module

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};
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

    // 构建扁平化树节点
    let flat_tree = build_flat_tree_from_app(app);

    // 转换为列表项
    let items: Vec<ListItem> = flat_tree
        .iter()
        .enumerate()
        .map(|(i, node)| {
            // 构建显示文本
            let indent = "  ".repeat(node.depth);
            let marker = if app.selected_nodes.contains(&node.id) {
                "✓ "
            } else {
                "  "
            };
            let expand_symbol = if node.has_children {
                if node.is_expanded { "▼ " } else { "▶ " }
            } else {
                "  "
            };
            
            let display_text = format!(
                "{}{}{}{}",
                indent, expand_symbol, marker, node.name
            );

            // 高亮当前光标位置
            let line = if i == app.tree_cursor {
                Line::from(vec![Span::styled(
                    display_text,
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )])
            } else {
                Line::from(display_text)
            };

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

/// 从 App 构建扁平化的树节点列表用于显示
fn build_flat_tree_from_app(app: &App) -> Vec<crate::app::FlatTreeNode> {
    let mut result = Vec::new();
    build_flat_tree_recursive(&app.ast.modules, &mut result, 0, &app.expanded_nodes);
    result
}

fn build_flat_tree_recursive(
    modules: &[openscad_core::ModuleNode],
    result: &mut Vec<crate::app::FlatTreeNode>,
    depth: usize,
    expanded: &std::collections::HashSet<String>,
) {
    for module in modules {
        let has_children = !module.children.is_empty();
        let is_expanded = expanded.contains(&module.id);

        result.push(crate::app::FlatTreeNode {
            id: module.id.clone(),
            name: module.get_display_name(),
            depth,
            has_children,
            is_expanded,
        });

        // 只有当节点被展开时，才递归处理子节点
        if is_expanded {
            build_flat_tree_recursive(&module.children, result, depth + 1, expanded);
        }
    }
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    use crate::app::InputMode;
    
    let block = if app.input_mode == InputMode::Command {
        Block::default()
            .title(" 🔧 Command Mode ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    } else if app.input_mode == InputMode::InsertSelectModule {
        Block::default()
            .title(" 📝 Insert - Select Module ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Cyan))
    } else if app.input_mode == InputMode::InsertEnterParams {
        Block::default()
            .title(" 📝 Insert - Enter Parameters ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Cyan))
    } else {
        Block::default()
            .title(" ⌨️  Input ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Green))
    };

    let text = match app.input_mode {
        InputMode::Command => format!(":{}", app.input_buffer),
        InputMode::InsertSelectModule => {
            format!("Search module: {}", app.input_buffer)
        },
        InputMode::InsertEnterParams => {
            format!("Parameters for {}: {}", 
                app.insert_module_name.as_deref().unwrap_or("?"),
                app.input_buffer)
        },
        _ => app.input_buffer.clone(),
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
        } else if matches!(app.input_mode, InputMode::Command) {
            Style::default().fg(Color::Yellow)
        } else if matches!(app.input_mode, InputMode::InsertSelectModule | InputMode::InsertEnterParams) {
            Style::default().fg(Color::Cyan)
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
    fn test_flatten_tree() {
        let app = crate::app::App::new();
        let flat = build_flat_tree_from_app(&app);
        assert_eq!(flat.len(), 0);  // Empty tree
    }
}
