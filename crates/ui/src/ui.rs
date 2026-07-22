//! UI rendering module

use crate::app::{App, CameraButtonRegion, Screen};
use crate::preview::ModelPreviewStatus;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use tui_tree_widget::{Tree, TreeItem};
use unicode_width::UnicodeWidthChar;

const MODEL_PROTOCOL_WIDTH: usize = 10;
const MODEL_STATUS_WIDTH: usize = 18;
const MODEL_FRAME_SIZE_WIDTH: usize = 9;
const MODEL_TIME_WIDTH: usize = 4;
const MODEL_FPS_WIDTH: usize = 4;
const MODEL_SIZE_KB_WIDTH: usize = 5;

pub fn draw(f: &mut Frame, app: &mut App) {
    match app.screen {
        Screen::Editor => draw_editor_screen(f, app),
        Screen::ModelPreview => draw_model_screen(f, app),
        Screen::Assembly => draw_assembly_screen(f, app),
    }
}

fn draw_assembly_screen(f: &mut Frame, app: &mut App) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(f.area());
    let content = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(38), Constraint::Min(20)])
        .split(main_chunks[0]);
    app.ui_regions.camera_buttons.clear();
    app.ui_regions.preview = content[1];
    app.ui_regions.input = main_chunks[1];
    if content[0].height >= 14 {
        let sidebar = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(11)])
            .split(content[0]);
        app.ui_regions.tree = sidebar[0];
        draw_assembly_parts(f, app, sidebar[0]);
        draw_assembly_details(f, app, sidebar[1]);
    } else {
        app.ui_regions.tree = content[0];
        draw_assembly_parts(f, app, content[0]);
    }
    draw_assembly_preview(f, app, content[1]);
    match app.input_mode {
        crate::app::InputMode::Command | crate::app::InputMode::ModuleEnterParams => {
            if app.completion_active && !app.completion_candidates.is_empty() {
                draw_completion_popup(f, app, main_chunks[1]);
            }
            draw_input(f, app, main_chunks[1]);
        }
        crate::app::InputMode::Normal | crate::app::InputMode::Help => {
            draw_camera_toolbar(f, app, main_chunks[1]);
        }
    }
    if app.input_mode == crate::app::InputMode::Help {
        draw_help_modal(f, app);
    }
}

fn draw_assembly_parts(f: &mut Frame, app: &mut App, area: Rect) {
    let active = app.active_assembly.as_deref().and_then(|id| {
        app.assemblies
            .iter()
            .find(|assembly| assembly.id == id || assembly.name == id)
    });
    let title = active
        .map(|assembly| format!(" Assembly: {} ", assembly.name))
        .unwrap_or_else(|| " Assembly ".to_string());
    let rows = active.map(|assembly| assembly.hierarchy_rows());
    let visible_height = area.height.saturating_sub(2) as usize;
    if let (Some(assembly), Some(rows)) = (active, rows.as_ref()) {
        if let Some(selected_row) = app.selected_assembly_part.as_deref().and_then(|selected| {
            rows.iter()
                .position(|(index, _)| assembly.parts[*index].id == selected)
        }) {
            if selected_row < app.assembly_scroll_offset {
                app.assembly_scroll_offset = selected_row;
            } else if visible_height > 0
                && selected_row >= app.assembly_scroll_offset + visible_height
            {
                app.assembly_scroll_offset = selected_row + 1 - visible_height;
            }
        }
        app.assembly_scroll_offset = app
            .assembly_scroll_offset
            .min(rows.len().saturating_sub(visible_height.max(1)));
    } else {
        app.assembly_scroll_offset = 0;
    }
    let lines = active
        .map(|assembly| {
            rows.as_ref()
                .expect("active assembly has hierarchy rows")
                .iter()
                .skip(app.assembly_scroll_offset)
                .take(visible_height)
                .map(|(index, depth)| {
                    let part = &assembly.parts[*index];
                    let selected = app.selected_assembly_part.as_deref() == Some(&part.id);
                    let marker = if selected { ">" } else { " " };
                    let visibility = if part.visible { "●" } else { "○" };
                    Line::styled(
                        format!("{marker}{} {visibility} {}", "  ".repeat(*depth), part.name),
                        if selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        },
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![Line::styled("No active assembly", Color::DarkGray)]);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Yellow)),
        ),
        area,
    );
}

fn draw_assembly_details(f: &mut Frame, app: &App, area: Rect) {
    let part = app.active_assembly.as_deref().and_then(|active| {
        let selected = app.selected_assembly_part.as_deref()?;
        app.assemblies
            .iter()
            .find(|assembly| assembly.id == active || assembly.name == active)?
            .part(selected)
    });
    let lines = part
        .map(|part| {
            vec![
                Line::from(format!("Name: {}", part.name)),
                Line::from(format!("ID: {}", part.id)),
                Line::from(format!("Source: {}", part.source.virtual_path())),
                Line::from(format!(
                    "Parent: {}",
                    part.parent.as_deref().unwrap_or("root")
                )),
                Line::from(format!(
                    "Visible: {}",
                    if part.visible { "yes" } else { "no" }
                )),
                Line::from(format_assembly_vec3("T", part.transform.translation)),
                Line::from(format_assembly_vec3("R", part.transform.rotation_degrees)),
                Line::from(format_assembly_vec3("S", part.transform.scale)),
                Line::from(format_assembly_vec3("Pivot R/S", part.transform.pivot)),
            ]
        })
        .unwrap_or_else(|| vec![Line::styled("No part selected", Color::DarkGray)]);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(" Selected Part ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Cyan)),
        ),
        area,
    );
}

fn format_assembly_vec3(label: &str, values: [f32; 3]) -> String {
    format!(
        "{label}: [{:.3}, {:.3}, {:.3}]",
        values[0], values[1], values[2]
    )
}

fn draw_model_screen(f: &mut Frame, app: &mut App) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(f.area());
    app.ui_regions.camera_buttons.clear();
    app.ui_regions.tree = Rect::default();
    app.ui_regions.preview = main_chunks[0];
    app.ui_regions.input = main_chunks[1];
    draw_model_preview(f, app, main_chunks[0]);
    match app.input_mode {
        crate::app::InputMode::Command | crate::app::InputMode::ModuleEnterParams => {
            if app.completion_active && !app.completion_candidates.is_empty() {
                draw_completion_popup(f, app, main_chunks[1]);
            }
            draw_input(f, app, main_chunks[1]);
        }
        crate::app::InputMode::Normal | crate::app::InputMode::Help => {
            draw_camera_toolbar(f, app, main_chunks[1]);
        }
    }
    if app.input_mode == crate::app::InputMode::Help {
        draw_help_modal(f, app);
    }
}

fn draw_editor_screen(f: &mut Frame, app: &mut App) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(4)])
        .split(f.area());
    app.ui_regions.camera_buttons.clear();

    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
        .split(main_chunks[0]);

    app.ui_regions.tree = content_chunks[0];
    app.ui_regions.preview = content_chunks[1];
    app.ui_regions.input = main_chunks[1];

    draw_tree(f, app, content_chunks[0]);
    draw_preview(f, app, content_chunks[1]);

    if app.completion_active && !app.completion_candidates.is_empty() {
        draw_completion_popup(f, app, main_chunks[1]);
    }

    draw_input(f, app, main_chunks[1]);

    if app.input_mode == crate::app::InputMode::Help {
        draw_help_modal(f, app);
    }
}

fn draw_camera_toolbar(f: &mut Frame, app: &mut App, area: Rect) {
    let close_label = match app.screen {
        Screen::Assembly => "Source",
        Screen::Editor | Screen::ModelPreview => match app.preview_close_action {
            crate::app::PreviewCloseAction::Source => "Source",
            crate::app::PreviewCloseAction::Quit => "Quit",
        },
    };
    let preview = match app.screen {
        Screen::Assembly => &app.assembly_preview,
        Screen::Editor | Screen::ModelPreview => &app.model_preview,
    };
    let projection = match preview.camera.projection {
        openscad_render::Projection::Perspective { .. } => "Ortho",
        openscad_render::Projection::Orthographic { .. } => "Persp",
    };
    let auto = if preview.auto_rotate { "Stop" } else { "Auto" };
    let axes = if preview.axes_visible {
        "Axes-"
    } else {
        "Axes+"
    };
    let buttons = if app.screen == Screen::Assembly {
        let selected_visible = app.active_assembly.as_deref().and_then(|active| {
            let selected = app.selected_assembly_part.as_deref()?;
            app.assemblies
                .iter()
                .find(|assembly| assembly.id == active || assembly.name == active)?
                .part(selected)
                .map(|part| part.visible)
        });
        let visibility_label = match selected_visible {
            Some(true) => "Hide",
            Some(false) => "Show",
            None => "Select",
        };
        let visibility = if selected_visible.is_some() {
            "assembly visibility toggle".to_string()
        } else {
            "assembly select next".to_string()
        };
        vec![
            ("P", "Source".to_string(), "assembly close".to_string()),
            ("R", "Render".to_string(), "assembly render".to_string()),
            ("v", visibility_label.to_string(), visibility),
            ("y", "Copy".to_string(), "assembly copy".to_string()),
            ("p", "Paste".to_string(), "assembly paste".to_string()),
            (
                "Space",
                auto.to_string(),
                "camera auto-rotate toggle".to_string(),
            ),
            (
                "Proj",
                projection.to_string(),
                "camera projection toggle".to_string(),
            ),
        ]
    } else {
        vec![
            ("P", close_label.to_string(), "model close".to_string()),
            ("f", "Fit".to_string(), "camera fit".to_string()),
            (
                "p",
                projection.to_string(),
                "camera projection toggle".to_string(),
            ),
            ("x", axes.to_string(), "display axes toggle".to_string()),
            ("1", "Front".to_string(), "camera view front".to_string()),
            ("5", "Top".to_string(), "camera view top".to_string()),
            ("7", "Iso".to_string(), "camera view iso".to_string()),
            (
                "Space",
                auto.to_string(),
                "camera auto-rotate toggle".to_string(),
            ),
        ]
    };
    let block = Block::default()
        .title(if app.screen == Screen::Assembly {
            " Assembly Controls "
        } else {
            " Camera Controls "
        })
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Magenta));
    let inner = block.inner(area);
    let mut spans = Vec::new();
    let mut x = inner.x;
    for (shortcut, label, command) in buttons {
        let text = format!("[{shortcut} {label}]");
        let width = text.chars().count() as u16;
        if x.saturating_add(width) > inner.right() {
            break;
        }
        app.ui_regions.camera_buttons.push(CameraButtonRegion {
            area: Rect::new(x, inner.y, width, 1),
            command,
        });
        spans.push(Span::styled(
            text,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        x = x.saturating_add(width + 1);
    }
    let shortcut_help = Line::styled(
        if app.screen == Screen::Assembly {
            "j/k Select  a Add  t/r/s Transform  o Pivot  g Parent  v Hide  y/p Copy/Paste  Space Auto"
        } else {
            "h/j/k/l Orbit  Arrows Pan  +/- Zoom  x Axes  1..7 Views  Esc/q Source  : Command"
        },
        Style::default().fg(Color::DarkGray),
    );
    f.render_widget(
        Paragraph::new(vec![Line::from(spans), shortcut_help]),
        inner,
    );
    f.render_widget(block, area);
}

fn draw_tree(f: &mut Frame, app: &App, area: Rect) {
    let project_name = &app.project_name;
    let active_source = app.ast.active_source.as_deref();
    let unsaved_flag = if app.saved { "" } else { "*" };
    let document = active_source
        .map(|source| format!(" [{source}]"))
        .unwrap_or_default();
    let title = if app.selected_nodes.is_empty() {
        format!(" {project_name}{document}{unsaved_flag} ")
    } else {
        format!(
            " {project_name}{document}{unsaved_flag} ({}) ",
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

    if ast.embedded_sources.iter().any(|source| source.editable) {
        let source_children = ast
            .embedded_sources
            .iter()
            .enumerate()
            .filter(|(_, source)| source.editable)
            .map(|(index, source)| {
                let included = ast.source_dependencies.iter().any(|dependency| {
                    dependency.to == source.virtual_path
                        && dependency.kind == openscad_core::SourceDependencyKind::Include
                });
                let used = ast.source_dependencies.iter().any(|dependency| {
                    dependency.to == source.virtual_path
                        && dependency.kind == openscad_core::SourceDependencyKind::Use
                });
                let role = match source.role {
                    openscad_core::EmbeddedSourceRole::Entry => "entry",
                    openscad_core::EmbeddedSourceRole::Library => match (included, used) {
                        (true, true) => "library/include/use",
                        (true, false) => "library/include",
                        (false, true) => "library/use",
                        (false, false) => "library",
                    },
                    openscad_core::EmbeddedSourceRole::Dependency if source.editable => "part",
                    openscad_core::EmbeddedSourceRole::Dependency => match (included, used) {
                        (true, true) => "library/include/use",
                        (true, false) => "library/include",
                        (false, true) => "library/use",
                        (false, false) => "library/dependency",
                    },
                };
                let active = ast.active_source.as_deref() == Some(&source.virtual_path);
                let marker = if active { "* " } else { "  " };
                TreeItem::new(
                    format!("__project_source_{index}"),
                    format!("{marker}[{role}] {}", source.virtual_path),
                    vec![],
                )
                .expect("Failed to create TreeItem")
            })
            .collect();
        items.push(
            TreeItem::new(
                "__project_sources".to_string(),
                "[Project Sources]".to_string(),
                source_children,
            )
            .expect("Failed to create TreeItem"),
        );
    }

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
                let id = format!("__var_{}", var.name);
                let display = format!("{} = {}", var.name, var.value.to_scad());
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
            prompt =
                "i/I insert after/before  Space show/hide  j/k move  v select  P model  : command"
                    .to_string();
            style_fg = Color::Yellow;
        }
        InputMode::Command => {
            title = " Command Mode ".to_string();
            prompt = "Enter command (type help for commands, Esc to exit):".to_string();
            style_fg = Color::Green;
        }
        InputMode::ModuleEnterParams => {
            let action = match app.pending_module_action {
                Some(crate::app::PendingModuleAction::Replace { .. }) => "Replace",
                Some(crate::app::PendingModuleAction::InsertBefore) => "Insert Before",
                _ => "Insert",
            };
            title = format!(" {} Parameters ", action);
            prompt = format!(
                "Parameters for '{}': ",
                app.pending_module_name.as_deref().unwrap_or("module")
            );
            style_fg = Color::Cyan;
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
    if shows_input_buffer(app.input_mode) {
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

fn shows_input_buffer(mode: crate::app::InputMode) -> bool {
    matches!(
        mode,
        crate::app::InputMode::Command | crate::app::InputMode::ModuleEnterParams
    )
}

fn draw_preview(f: &mut Frame, app: &mut App, area: Rect) {
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

fn draw_model_preview(f: &mut Frame, app: &mut App, area: Rect) {
    let title = model_preview_title(app);
    draw_preview_surface(f, &mut app.model_preview, area, title);
}

fn draw_assembly_preview(f: &mut Frame, app: &mut App, area: Rect) {
    let title = preview_title(&app.assembly_preview, "Assembly");
    draw_preview_surface(f, &mut app.assembly_preview, area, title);
}

fn draw_preview_surface(
    f: &mut Frame,
    preview: &mut crate::preview::ModelPreview,
    area: Rect,
    title: String,
) {
    preview.set_area(area);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Green));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if matches!(preview.status, ModelPreviewStatus::Empty) {
        f.render_widget(
            Paragraph::new(preview_status(preview)).style(Style::default().fg(Color::DarkGray)),
            inner,
        );
    } else if let Some(image) = preview.image_widget() {
        f.render_widget(image, inner);
    } else {
        f.render_widget(
            Paragraph::new(preview_status(preview)).style(Style::default().fg(Color::Red)),
            inner,
        );
    }
}

fn preview_status(preview: &crate::preview::ModelPreview) -> String {
    if let Some(error) = preview.presentation_error() {
        return format!("display failed: {error}");
    }
    match &preview.status {
        ModelPreviewStatus::Empty => "not rendered".to_string(),
        ModelPreviewStatus::Stale => "stale — run :render".to_string(),
        ModelPreviewStatus::Loading => "loading model…".to_string(),
        ModelPreviewStatus::Rasterizing => "rasterizing…".to_string(),
        ModelPreviewStatus::Ready { triangles } => format!("{triangles} triangles"),
        ModelPreviewStatus::Failed(error) => format!("failed: {error}"),
    }
}

fn model_preview_title(app: &App) -> String {
    preview_title(&app.model_preview, "Model")
}

fn preview_title(preview: &crate::preview::ModelPreview, label: &str) -> String {
    let protocol = fixed_display_width(
        &format!("{:?}", preview.protocol_type()),
        MODEL_PROTOCOL_WIDTH,
    );
    let status = fixed_display_width(&preview_status(preview), MODEL_STATUS_WIDTH);
    let frame_size = preview
        .metrics
        .frame_size
        .map(|size| format!("{}x{}", size.width, size.height))
        .unwrap_or_else(|| "-".to_string());
    let frame_size = fixed_display_width(&frame_size, MODEL_FRAME_SIZE_WIDTH);
    let metrics = &preview.metrics;
    let generation = fixed_metric(
        metrics.generation_time.as_secs_f64() * 1000.0,
        MODEL_TIME_WIDTH,
        0,
    );
    let raster = fixed_metric(
        metrics.raster_time.as_secs_f64() * 1000.0,
        MODEL_TIME_WIDTH,
        0,
    );
    let encode = fixed_metric(
        metrics.encode_time.as_secs_f64() * 1000.0,
        MODEL_TIME_WIDTH,
        0,
    );
    let draw = fixed_metric(
        metrics.ui_draw_time.as_secs_f64() * 1000.0,
        MODEL_TIME_WIDTH,
        0,
    );
    let fps = fixed_metric(metrics.presented_fps.into(), MODEL_FPS_WIDTH, 1);
    let size_kb = fixed_metric(
        (metrics.encoded_bytes / 1024) as f64,
        MODEL_SIZE_KB_WIDTH,
        0,
    );
    let size_estimate_marker = if metrics.encoded_bytes_estimated {
        "~"
    } else {
        " "
    };

    format!(
        " {label} [{protocol}] {status} | {frame_size} G:{generation} R:{raster} E:{encode} D:{draw}ms {fps}fps {size_estimate_marker}{size_kb}KB ",
    )
}

fn fixed_display_width(value: &str, width: usize) -> String {
    let mut result = String::new();
    let mut display_width = 0;
    for character in value.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if display_width + character_width > width {
            break;
        }
        result.push(character);
        display_width += character_width;
    }
    result.extend(std::iter::repeat_n(' ', width - display_width));
    result
}

fn fixed_metric(value: f64, width: usize, precision: usize) -> String {
    let formatted = if value.is_finite() {
        format!("{value:.precision$}")
    } else {
        "-".to_string()
    };
    if formatted.len() > width {
        return if width == 0 {
            String::new()
        } else if width == 1 {
            "+".to_string()
        } else {
            format!("{}+", "9".repeat(width - 1))
        };
    }
    format!("{formatted:>width$}")
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
    use super::{draw, model_preview_title, shows_input_buffer};
    use crate::app::{App, InputMode};
    use crate::commands::{cmd_assembly, cmd_edit_scad, cmd_load_library};
    use crate::preview::ModelPreviewStatus;
    use ratatui::{backend::TestBackend, Terminal};
    use std::fs;
    use std::time::Duration;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn test_new_app_selects_project_sources() {
        let app = App::new();
        assert_eq!(app.tree_state.borrow().selected(), ["__project_sources"]);
    }

    #[test]
    fn test_navigation_status_update() {
        let mut app = App::new();
        app.update_navigation_status();
        assert_eq!(app.message.as_deref(), Some("> [Project Sources]"));
    }

    #[test]
    fn test_project_sources_hide_read_only_libraries() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main.scad");
        let library = directory.path().join("hidden_external_library.scad");
        fs::write(&main, "cube(1);").unwrap();
        fs::write(&library, "module helper() { sphere(1); }").unwrap();
        let mut app = App::new();
        cmd_edit_scad(&mut app, main.to_str().unwrap()).unwrap();
        cmd_load_library(&mut app, library.to_str().unwrap()).unwrap();
        app.clear_message();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        app.tree_state.borrow_mut().key_right();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let buffer = terminal.backend().buffer();
        let screen = (0..buffer.area.height)
            .flat_map(|y| (0..buffer.area.width).map(move |x| buffer[(x, y)].symbol()))
            .collect::<String>();
        assert!(screen.contains("Project Sources"));
        assert!(screen.contains("main.scad"));
        assert!(!screen.contains("hidden_external_library.scad"));
    }

    #[test]
    fn test_replace_parameter_mode_shows_input_buffer() {
        assert!(shows_input_buffer(InputMode::ModuleEnterParams));
        assert!(shows_input_buffer(InputMode::Command));
        assert!(!shows_input_buffer(InputMode::Normal));
        assert!(!shows_input_buffer(InputMode::Help));
    }

    #[test]
    fn test_model_preview_uses_full_width_and_camera_toolbar_with_shortcuts() {
        let mut app = App::new();
        app.enter_model_screen();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        assert_eq!(app.ui_regions.tree.width, 0);
        assert_eq!(app.ui_regions.preview.width, 100);
        assert!(app
            .ui_regions
            .camera_buttons
            .iter()
            .any(|button| button.command == "model close"));
        assert!(app
            .ui_regions
            .camera_buttons
            .iter()
            .any(|button| button.command == "camera view iso"));
        assert!(app
            .ui_regions
            .camera_buttons
            .iter()
            .any(|button| button.command == "display axes toggle"));

        let buffer = terminal.backend().buffer();
        let row = |y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        };
        let buttons = row(27);
        assert!(buttons.contains("[P Source]"));
        assert!(buttons.contains("[f Fit]"));
        assert!(buttons.contains("[p Ortho]"));
        assert!(buttons.contains("[x Axes-]"));
        assert!(buttons.contains("[Space Auto]"));
        let shortcuts = row(28);
        assert!(shortcuts.contains("h/j/k/l Orbit"));
        assert!(shortcuts.contains("Arrows Pan"));
        assert!(shortcuts.contains("+/- Zoom"));
        assert!(shortcuts.contains("1..7 Views"));
        assert!(shortcuts.contains("Esc/q Source"));
    }

    #[test]
    fn test_assembly_screen_draws_parts_and_uses_command_backed_toolbar() {
        let mut app = App::new();
        cmd_assembly(&mut app, &["new", "robot"]).unwrap();
        cmd_assembly(&mut app, &["add", "main.scad", "body"]).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        assert_eq!(app.screen, crate::app::Screen::Assembly);
        assert_eq!(app.ui_regions.tree.width, 38);
        assert!(app
            .ui_regions
            .camera_buttons
            .iter()
            .any(|button| button.command == "assembly close"));
        assert!(app
            .ui_regions
            .camera_buttons
            .iter()
            .any(|button| button.command == "assembly visibility toggle"));
        assert!(app
            .ui_regions
            .camera_buttons
            .iter()
            .any(|button| button.command == "camera auto-rotate toggle"));
        assert!(app
            .ui_regions
            .camera_buttons
            .iter()
            .any(|button| button.command == "assembly copy"));
        assert!(app
            .ui_regions
            .camera_buttons
            .iter()
            .any(|button| button.command == "assembly paste"));
        let buffer = terminal.backend().buffer();
        let screen = (0..buffer.area.height)
            .flat_map(|y| (0..buffer.area.width).map(move |x| buffer[(x, y)].symbol()))
            .collect::<String>();
        assert!(screen.contains("Assembly: robot"));
        assert!(screen.contains("body"));
        assert!(screen.contains("Selected Part"));
        assert!(screen.contains("Source: main.scad"));
        assert!(screen.contains("T: [0.000, 0.000, 0.000]"));
    }

    #[test]
    fn test_model_preview_metric_labels_stay_in_fixed_columns() {
        let mut app = App::new();
        app.model_preview.status = ModelPreviewStatus::Rasterizing;
        app.model_preview.metrics.generation_time = Duration::from_millis(1);
        app.model_preview.metrics.raster_time = Duration::from_millis(9);
        app.model_preview.metrics.encode_time = Duration::from_millis(99);
        app.model_preview.metrics.ui_draw_time = Duration::from_millis(999);
        app.model_preview.metrics.presented_fps = 1.0;
        app.model_preview.metrics.encoded_bytes = 1024;
        let short_values = model_preview_title(&app);

        app.model_preview.status = ModelPreviewStatus::Ready { triangles: 123_456 };
        app.model_preview.metrics.generation_time = Duration::from_secs(120);
        app.model_preview.metrics.raster_time = Duration::from_secs(12);
        app.model_preview.metrics.encode_time = Duration::from_millis(1_234);
        app.model_preview.metrics.ui_draw_time = Duration::from_millis(10_000);
        app.model_preview.metrics.presented_fps = 1200.0;
        app.model_preview.metrics.encoded_bytes = 128 * 1024 * 1024;
        app.model_preview.metrics.encoded_bytes_estimated = true;
        let long_values = model_preview_title(&app);

        for label in [" G:", " R:", " E:", " D:", "fps", "KB"] {
            let short_index = short_values.find(label).unwrap();
            let long_index = long_values.find(label).unwrap();
            assert_eq!(
                UnicodeWidthStr::width(&short_values[..short_index]),
                UnicodeWidthStr::width(&long_values[..long_index]),
                "{label} moved when its preceding value changed"
            );
        }
    }

    #[test]
    fn test_model_preview_replaces_toolbar_with_command_input() {
        let mut app = App::new();
        app.enter_model_screen();
        app.input_mode = InputMode::Command;
        app.input_buffer.set_content("camera view iso");
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        assert!(app.ui_regions.camera_buttons.is_empty());
        assert_eq!(app.ui_regions.input.height, 4);
        let buffer = terminal.backend().buffer();
        let screen = (0..buffer.area.height)
            .flat_map(|y| (0..buffer.area.width).map(move |x| buffer[(x, y)].symbol()))
            .collect::<String>();
        assert!(screen.contains("Command Mode"));
        assert!(screen.contains("camera view iso"));
    }
}
