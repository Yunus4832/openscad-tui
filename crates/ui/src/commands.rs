//! Commands module for OpenSCAD TUI

use openscad_core::{Argument, ArgumentSelector, AstError, Expr, ModuleNode};
use openscad_library::{LibraryError, ModuleDef};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

use crate::app::{App, InputMode, PendingModuleAction, Screen};
use crate::command_registry::CommandType;

const MAX_RECURSION_DEPTH: usize = 1000;

pub fn cmd_render(app: &mut App) -> CommandResult<()> {
    let source = app.ast.to_scad();
    let current_file = app.current_file.clone();
    app.model_preview
        .render(source, current_file.as_deref())
        .map_err(CommandError::Custom)?;
    app.enter_model_screen();
    Ok(())
}

pub fn cmd_preview(app: &mut App, mode: &str) -> CommandResult<()> {
    match mode {
        "source" => app.enter_editor_screen(),
        "model"
            if matches!(
                app.model_preview.status,
                crate::preview::ModelPreviewStatus::Empty
            ) =>
        {
            return cmd_render(app)
        }
        "model" => app.enter_model_screen(),
        "toggle" => match app.screen {
            Screen::Editor => return cmd_preview(app, "model"),
            Screen::ModelPreview => return cmd_preview(app, "source"),
        },
        _ => {
            return Err(CommandError::InvalidCommand(
                "Usage: preview source|model|toggle".to_string(),
            ))
        }
    };
    Ok(())
}

pub fn cmd_camera(app: &mut App, args: &[&str]) -> CommandResult<()> {
    use openscad_render::{Projection, StandardView};

    let invalid = || {
        CommandError::InvalidCommand(
        "Usage: camera projection perspective|orthographic|toggle | view front|back|left|right|top|bottom|iso | orbit <yaw-deg> <pitch-deg> | pan <x> <y> | zoom <factor> | fit | auto-rotate on|off|toggle"
            .to_string(),
    )
    };
    let parse = |value: &str| value.parse::<f32>().map_err(|_| invalid());
    let result = match args {
        ["projection", "perspective"] => app.model_preview.set_projection(false),
        ["projection", "orthographic"] => app.model_preview.set_projection(true),
        ["projection", "toggle"] => {
            let use_orthographic = matches!(
                app.model_preview.camera.projection,
                Projection::Perspective { .. }
            );
            app.model_preview.set_projection(use_orthographic)
        }
        ["view", name] => {
            let view = match *name {
                "front" => StandardView::Front,
                "back" => StandardView::Back,
                "left" => StandardView::Left,
                "right" => StandardView::Right,
                "top" => StandardView::Top,
                "bottom" => StandardView::Bottom,
                "iso" | "isometric" => StandardView::Isometric,
                _ => return Err(invalid()),
            };
            app.model_preview.set_view(view)
        }
        ["orbit", yaw, pitch] => app.model_preview.orbit(parse(yaw)?, parse(pitch)?),
        ["pan", horizontal, vertical] => {
            app.model_preview.pan(parse(horizontal)?, parse(vertical)?)
        }
        ["zoom", factor] => app.model_preview.zoom(parse(factor)?),
        ["fit"] => app.model_preview.fit(),
        ["auto-rotate", value] => {
            let enabled = match *value {
                "on" => true,
                "off" => false,
                "toggle" => !app.model_preview.auto_rotate,
                _ => return Err(invalid()),
            };
            app.model_preview.set_auto_rotate(enabled);
            Ok(())
        }
        _ => return Err(invalid()),
    };
    result.map_err(CommandError::Custom)
}

#[derive(Error, Debug)]
pub enum CommandError {
    #[error("Invalid command: {0}")]
    InvalidCommand(String),

    #[error("AST error: {0}")]
    AstError(#[from] AstError),

    #[error("Library error: {0}")]
    LibraryError(#[from] LibraryError),

    #[error("Parameter parsing error: {0}")]
    ParameterError(String),

    #[error("No node selected")]
    #[allow(dead_code)]
    NoNodeSelected,

    #[error("No children selected")]
    NoChildrenSelected,

    #[error("{0}")]
    Custom(String),
}

pub type CommandResult<T> = std::result::Result<T, CommandError>;

struct PreparedModule {
    node: ModuleNode,
    accepts_children: bool,
    source_file: Option<String>,
}

fn prepare_module(
    app: &App,
    module_name: &str,
    params: Option<&str>,
) -> CommandResult<PreparedModule> {
    let definition = app
        .library
        .get_module(module_name)
        .ok_or_else(|| CommandError::InvalidCommand(format!("Unknown module: {}", module_name)))?;
    let args = match params {
        Some(params) if !params.trim().is_empty() => parse_arguments(params, &definition)?,
        _ => Vec::new(),
    };
    let id = format!(
        "{}_{}",
        module_name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let mut node = if definition.accepts_children {
        ModuleNode::new_container(id, module_name.to_string(), args)
    } else {
        ModuleNode::new_leaf(id, module_name.to_string(), args)
    };
    let (source_library, source_file) = app.library.get_module_source(module_name);
    node.source_library = source_library;
    Ok(PreparedModule {
        node,
        accepts_children: definition.accepts_children,
        source_file,
    })
}

fn add_module_include(app: &mut App, source_file: Option<&String>) {
    if let Some(source_file) = source_file {
        if !app.ast.includes.contains(source_file) {
            app.ast_mut().includes.push(source_file.clone());
        }
    }
}

/// Insert command
/// Insert a new module in the tree.
///
/// For modules that accept children (accepts_children: true):
///   - If child nodes are selected, create the module and move selected nodes as children
///   - If no child nodes are selected, return NoChildrenSelected error
///
/// For leaf modules (accepts_children: false):
///   - Insert after the currently selected node if there is one
///   - If no node is selected, or selected node is not in Modules section, insert at root level of Modules
pub fn cmd_insert(
    app: &mut App,
    module_name: &str,
    _parent_id: Option<&str>,
    params: Option<&str>,
) -> CommandResult<String> {
    let prepared = prepare_module(app, module_name, params)?;
    insert_prepared_module(app, module_name, prepared)
}

fn insert_prepared_module(
    app: &mut App,
    module_name: &str,
    prepared: PreparedModule,
) -> CommandResult<String> {
    let node_id = prepared.node.id.clone();

    // Check if this module accepts children
    if prepared.accepts_children {
        // Use the shared implementation for inserting container with selected nodes
        let selected_nodes = if app.selected_nodes.is_empty() {
            if let Some(last_selected) = app.tree_state.borrow().selected().last() {
                let mut vec_from_tree = vec![last_selected.clone()];
                vec_from_tree.retain(|item| !item.starts_with("__"));
                vec_from_tree
            } else {
                Vec::new()
            }
        } else {
            app.selected_nodes.clone()
        };

        // For container modules, we need selected child nodes
        if selected_nodes.is_empty() {
            return Err(CommandError::NoChildrenSelected);
        }
        add_module_include(app, prepared.source_file.as_ref());
        insert_container_with_selected_nodes(app, prepared.node, &selected_nodes)
    } else {
        let module = prepared.node;

        // Determine insertion point based on current selection
        let selected = app.tree_state.borrow().selected().last().cloned();

        // Check if selected node is in a module definition
        let mut in_module_def = if let Some(ref selected_id) = selected {
            app.find_module_definition_for_node(selected_id).is_some()
        } else {
            false
        };

        // Children module can only be used inside module definitions
        if module_name == "children" && !in_module_def {
            return Err(CommandError::InvalidCommand(
                "children module can only be used inside module definitions".to_string(),
            ));
        }

        add_module_include(app, prepared.source_file.as_ref());

        // Special case: inserting a module with the same name as the module definition when selected is the definition header
        // This should create an instance in the modules section, not add to definition body
        if in_module_def {
            let selected_id = selected.as_ref().unwrap();
            if let Some(mod_def_name) = app.find_module_definition_for_node(selected_id) {
                if module_name == mod_def_name && selected_id.starts_with("__moddef_") {
                    in_module_def = false;
                }
            }
        }

        if in_module_def {
            // Insert into module definition body
            let selected_id = selected.unwrap();
            let mod_def_name = app.find_module_definition_for_node(&selected_id).unwrap();

            // Find the module definition
            let mod_def_idx = app
                .ast
                .module_defines
                .iter()
                .position(|md| md.name == mod_def_name)
                .ok_or_else(|| {
                    CommandError::InvalidCommand(format!(
                        "Module definition not found: {}",
                        mod_def_name
                    ))
                })?;

            // Check if selected node is the module definition itself
            if selected_id.starts_with("__moddef_") {
                // Insert at the end of module definition body
                app.ast_mut().module_defines[mod_def_idx].body.push(module);
            } else {
                // Insert after the selected node in module definition body
                app.ast_mut().insert_after(&selected_id, module)?;
            }
        } else {
            // Check if selected node is in Modules section (not a section header)
            let insert_at_root = match &selected {
                None => true,
                Some(id) => {
                    // If selected is a section header or not a module node, insert at root
                    id.starts_with("__") || app.ast.find_node_by_id(id).is_none()
                }
            };

            if insert_at_root {
                // Insert at root level of Modules section
                app.ast_mut().add_module(module)?;
            } else if let Some(selected_id) = selected {
                // Find the selected node and insert after it
                app.ast_mut().insert_after(&selected_id, module)?;
            }
        }

        // Select the newly inserted module for continued operations
        // Use the full path to ensure proper navigation in nested trees
        if let Some(path) = app.find_node_path(&node_id) {
            app.tree_state.borrow_mut().select(path.clone());
            // Open the appropriate section based on path
            if !path.is_empty() {
                // Open all parent sections
                let mut parent_path = Vec::new();
                for item in path.iter().take(path.len() - 1) {
                    parent_path.push(item.clone());
                    app.tree_state.borrow_mut().open(parent_path.clone());
                }
            }
        } else {
            // Fallback: just select by ID if path not found
            // Try to determine if this is in module definitions or modules
            let section = if in_module_def {
                "__moddefs"
            } else {
                "__modules"
            };
            app.tree_state
                .borrow_mut()
                .select(vec![section.to_string(), node_id.clone()]);
            app.tree_state.borrow_mut().open(vec![section.to_string()]);
        }

        // If we inserted a children module into a module definition, update the custom module's accepts_children flag
        if module_name == "children" && in_module_def {
            app.library
                .reload_custom_modules_from_ast(&app.ast.module_defines);
        }

        Ok(node_id)
    }
}

/// Delete command
pub fn cmd_delete(app: &mut App, node_id: &str) -> CommandResult<()> {
    let node_ids = if !node_id.is_empty() {
        vec![node_id.to_string()]
    } else if !app.selected_nodes.is_empty() {
        app.selected_nodes.clone()
    } else {
        vec![app
            .tree_state
            .borrow()
            .selected()
            .last()
            .cloned()
            .ok_or(CommandError::NoNodeSelected)?]
    };

    if let Some(section_id) = node_ids.iter().find(|id| {
        id.starts_with("__")
            && !id.starts_with("__var_")
            && !id.starts_with("__func_")
            && !id.starts_with("__moddef_")
    }) {
        return Err(CommandError::Custom(format!(
            "Cannot delete section header: {}",
            section_id
        )));
    }

    for target_id in &node_ids {
        if let Some(name) = target_id.strip_prefix("__var_") {
            app.ast_mut().remove_global_variable(name)?;
        } else if let Some(name) = target_id.strip_prefix("__func_") {
            app.ast_mut().remove_function_define(name)?;
        } else if let Some(name) = target_id.strip_prefix("__moddef_") {
            app.ast_mut().remove_module_define(name)?;
        } else if app.ast.find_node_anywhere(target_id).is_some() {
            // A selected descendant may already have been removed with its parent.
            let _ = app.ast_mut().delete_node(target_id);
        }
    }
    app.library
        .reload_custom_functions_from_ast(&app.ast.function_defines);
    app.library
        .reload_custom_modules_from_ast(&app.ast.module_defines);
    app.selected_nodes.clear();

    app.restore_tree_selection();

    Ok(())
}

/// Apply boolean operation (union, difference, intersection)
#[allow(dead_code)]
pub fn cmd_boolean_op(
    app: &mut App,
    operation: &str,
    node_ids: &[String],
) -> CommandResult<String> {
    // For boolean operations, we need to select the nodes first
    // Save current selection
    let current_selection = app.selected_nodes.clone();

    // Temporarily set selection to the provided node_ids
    app.selected_nodes = node_ids.to_vec();

    // Call cmd_insert with no parameters (boolean operations don't have parameters)
    let result = cmd_insert(app, operation, None, None);

    // If cmd_insert failed, restore original selection
    if result.is_err() {
        app.selected_nodes = current_selection;
    }
    // If cmd_insert succeeded, it will have cleared the selection
    // (through insert_container_with_selected_nodes)

    result
}

/// Insert a container module with selected nodes as children
/// Handles both modules section and module definition contexts
fn insert_container_with_selected_nodes(
    app: &mut App,
    container: ModuleNode,
    selected_node_ids: &[String],
) -> CommandResult<String> {
    if selected_node_ids.is_empty() {
        return Err(CommandError::NoChildrenSelected);
    }

    let container_id = container.id.clone();
    let first_selected = selected_node_ids.first().cloned();
    let in_module_def = if let Some(ref first_id) = first_selected {
        app.find_module_definition_for_node(first_id).is_some()
    } else {
        false
    };

    // Validate all selected nodes are in the same context
    let mut context_module_def_name: Option<String> = None;
    for node_id in selected_node_ids {
        let node_in_module_def = app.find_module_definition_for_node(node_id);
        match (node_in_module_def, &context_module_def_name) {
            (Some(ref mod_def_name), Some(ref context_name)) => {
                if mod_def_name != context_name {
                    return Err(CommandError::InvalidCommand(format!(
                        "Selected nodes are in different module definitions: {} vs {}",
                        mod_def_name, context_name
                    )));
                }
            }
            (Some(ref mod_def_name), None) => {
                context_module_def_name = Some(mod_def_name.clone());
            }
            (None, Some(_)) => {
                return Err(CommandError::InvalidCommand(
                    "Selected nodes are in mixed contexts (module definition vs modules section)"
                        .to_string(),
                ));
            }
            (None, None) => {
                // All nodes are in modules section, context remains None
            }
        }
    }
    // Ensure context consistency with first node
    if in_module_def && context_module_def_name.is_none() {
        // This shouldn't happen but handle edge case
        return Err(CommandError::InvalidCommand(
            "Inconsistent context detection".to_string(),
        ));
    }

    if in_module_def {
        // Handle insertion into module definition body
        let first_id = first_selected.unwrap();
        let mod_def_name = context_module_def_name.unwrap();

        // Find the module definition index
        let mod_def_idx = app
            .ast
            .module_defines
            .iter()
            .position(|md| md.name == mod_def_name)
            .ok_or_else(|| {
                CommandError::InvalidCommand(format!(
                    "Module definition not found: {}",
                    mod_def_name
                ))
            })?;

        // Find parent in module definition body
        let parent_id = find_node_parent(&app.ast.module_defines[mod_def_idx].body, &first_id);

        // Insert the container in module definition body
        if let Some(parent_id_val) = &parent_id {
            insert_child_before(
                &mut app.ast_mut().module_defines[mod_def_idx].body,
                parent_id_val,
                &first_id,
                container.clone(),
            )
            .map_err(CommandError::InvalidCommand)?;
        } else {
            // First selected node was at root level of module definition body
            if let Some(pos) = app.ast_mut().module_defines[mod_def_idx]
                .body
                .iter()
                .position(|m| m.id == first_id)
            {
                app.ast_mut().module_defines[mod_def_idx]
                    .body
                    .insert(pos, container.clone());
            } else {
                // Fallback: add to end of module definition body
                app.ast_mut().module_defines[mod_def_idx]
                    .body
                    .push(container.clone());
            }
        }

        // Collect nodes to move from module definition body
        let mut nodes_to_move = Vec::new();
        for node_id in selected_node_ids {
            // Search in module definition body
            if let Some(node) =
                find_node_in_module_definition(&app.ast.module_defines[mod_def_idx].body, node_id)
            {
                nodes_to_move.push(node.clone());
            }
        }

        // Delete nodes from module definition body
        for node_id in selected_node_ids {
            delete_node_from_module_definition(
                &mut app.ast_mut().module_defines[mod_def_idx].body,
                node_id,
            )
            .map_err(CommandError::InvalidCommand)?;
        }

        // Add collected nodes to the container
        if let Some(container_mut) = find_node_in_module_definition_mut(
            &mut app.ast_mut().module_defines[mod_def_idx].body,
            &container_id,
        ) {
            for node in nodes_to_move {
                container_mut.children.push(node);
            }
        }
    } else {
        // Original logic for modules section
        let parent_id = if let Some(ref first_id) = first_selected {
            find_node_parent(&app.ast.modules, first_id)
        } else {
            None
        };

        // Insert the container BEFORE deleting the selected nodes
        if let Some(parent_id_val) = &parent_id {
            insert_child_before(
                &mut app.ast_mut().modules,
                parent_id_val,
                first_selected.as_ref().unwrap(),
                container.clone(),
            )
            .map_err(CommandError::InvalidCommand)?;
        } else {
            // First selected node was at root level
            if let Some(pos) = app
                .ast
                .modules
                .iter()
                .position(|m| m.id == *first_selected.as_ref().unwrap())
            {
                app.ast_mut().modules.insert(pos, container.clone());
            } else {
                app.ast_mut().add_module(container.clone())?;
            }
        }

        // Collect nodes to move before modifying the tree
        let nodes_to_move: Vec<ModuleNode> = selected_node_ids
            .iter()
            .filter_map(|node_id| app.ast.find_node_by_id(node_id).cloned())
            .collect();

        // Delete the selected nodes from the tree
        for node_id in selected_node_ids {
            app.ast_mut().delete_node(node_id)?;
        }

        // Add collected nodes to the container
        if let Some(container_mut) = app.ast_mut().find_node_mut(&container_id) {
            for node in nodes_to_move {
                container_mut.children.push(node);
            }
        }
    }

    // Select the newly created container module for continued operations
    // Use the full path to ensure proper navigation in nested trees
    if let Some(path) = app.find_node_path(&container_id) {
        app.tree_state.borrow_mut().select(path);
    } else {
        // Fallback: just select by ID if path not found
        app.tree_state
            .borrow_mut()
            .select(vec![container_id.clone()]);
    }

    // Clear the selected nodes since they've been moved into the container
    app.selected_nodes.clear();

    Ok(container_id)
}

/// Select command
#[allow(dead_code)]
pub fn cmd_select(app: &mut App, node_id: &str) -> CommandResult<()> {
    if app.ast.find_node_by_id(node_id).is_none() {
        return Err(CommandError::InvalidCommand(format!(
            "Node not found: {}",
            node_id
        )));
    }

    if !app.selected_nodes.contains(&node_id.to_string()) {
        app.selected_nodes.push(node_id.to_string());
    }

    Ok(())
}

/// Deselect command
#[allow(dead_code)]
pub fn cmd_deselect(app: &mut App, node_id: &str) -> CommandResult<()> {
    app.selected_nodes.retain(|id| id != node_id);
    Ok(())
}

/// Clear selection
#[allow(dead_code)]
pub fn cmd_clear_selection(app: &mut App) {
    app.selected_nodes.clear();
}

/// Navigation commands
/// Move cursor down (next)
#[allow(dead_code)]
pub fn cmd_next(app: &mut App) -> CommandResult<()> {
    app.tree_state.borrow_mut().key_down();
    app.update_navigation_status();
    Ok(())
}

/// Move cursor up (previous)
#[allow(dead_code)]
pub fn cmd_prev(app: &mut App) -> CommandResult<()> {
    app.tree_state.borrow_mut().key_up();
    app.update_navigation_status();
    Ok(())
}

/// Collapse node (move left)
#[allow(dead_code)]
pub fn cmd_collapse(app: &mut App) -> CommandResult<()> {
    app.tree_state.borrow_mut().key_left();
    app.update_navigation_status();
    Ok(())
}

/// Expand node (move right)
#[allow(dead_code)]
pub fn cmd_expand(app: &mut App) -> CommandResult<()> {
    app.tree_state.borrow_mut().key_right();
    app.update_navigation_status();
    Ok(())
}

/// Toggle node (move right)
#[allow(dead_code)]
pub fn cmd_toggle(app: &mut App) -> CommandResult<()> {
    app.tree_state.borrow_mut().toggle_selected();
    app.update_navigation_status();
    Ok(())
}

/// Select/toggle current node
#[allow(dead_code)]
pub fn cmd_select_toggle(app: &mut App) -> CommandResult<()> {
    let selected = app.tree_state.borrow().selected().last().cloned();
    if let Some(node_id) = selected {
        // Prevent selection of section headers
        if node_id.starts_with("__") {
            app.set_info("Cannot select section headers");
            return Ok(());
        }

        if app.selected_nodes.contains(&node_id) {
            app.selected_nodes.retain(|n| n != &node_id);
            app.set_info(&format!("Deselected: {}", node_id));
        } else {
            app.selected_nodes.push(node_id.clone());
            app.set_info(&format!("Selected: {}", node_id));
        }
        Ok(())
    } else {
        Err(CommandError::NoNodeSelected)
    }
}

/// Clear all selections
#[allow(dead_code)]
pub fn cmd_deselect_all(app: &mut App) -> CommandResult<()> {
    app.selected_nodes.clear();
    app.set_info("All nodes deselected");
    Ok(())
}

// Helper function to parse arguments
/// Parse arguments from a string, respecting nested structures (lists, etc.)
///
/// Handles complex parameters like:
/// - Simple values: 10, 1.5, "text", true
/// - Lists: [10, 20, 30] or [1.5, 2.5, 3.5]
/// - Named parameters: size=[10,10,10], center=true
/// - Mixed: 10, [20,30], center=true
fn parse_arguments(param_str: &str, _module_def: &ModuleDef) -> CommandResult<Vec<Argument>> {
    let mut args = Vec::new();

    if param_str.trim().is_empty() {
        return Ok(args);
    }

    // Split parameters while respecting brackets and quotes
    let parts = split_parameters(param_str)?;

    for (i, part) in parts.iter().enumerate() {
        let part = part.trim();

        // Check if this is a named parameter (contains '=')
        if let Some(eq_pos) = part.find('=') {
            let name = part[..eq_pos].trim();
            let value_str = part[eq_pos + 1..].trim();

            let value = Expr::parse(value_str).map_err(|e| {
                CommandError::ParameterError(format!(
                    "Invalid parameter value for '{}': {} - {}",
                    name, value_str, e
                ))
            })?;

            args.push(Argument::Named {
                name: name.to_string(),
                value,
            });
        } else {
            // Positional parameter
            let value = Expr::parse(part).map_err(|e| {
                CommandError::ParameterError(format!(
                    "Invalid parameter at position {}: {} - {}",
                    i, part, e
                ))
            })?;

            args.push(Argument::Positional(value));
        }
    }

    Ok(args)
}

/// Split parameters respecting brackets and quotes
/// This function splits by commas but ignores commas inside brackets or quotes
///
/// Examples:
/// "10, 20, 30" → ["10", "20", "30"]
/// "[10, 20], 30" → ["[10, 20]", "30"]
/// "size=[10,20,30], center=true" → ["size=[10,20,30]", "center=true"]
fn split_parameters(input: &str) -> CommandResult<Vec<String>> {
    let mut params = Vec::new();
    let mut current = String::new();
    let mut bracket_depth = 0;
    let mut paren_depth = 0;
    let mut in_quotes = false;
    let mut escape_next = false;

    for ch in input.chars() {
        if escape_next {
            current.push(ch);
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if in_quotes => {
                escape_next = true;
                current.push(ch);
            }
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            '[' if !in_quotes => {
                bracket_depth += 1;
                current.push(ch);
            }
            ']' if !in_quotes && bracket_depth > 0 => {
                bracket_depth -= 1;
                current.push(ch);
            }
            '(' if !in_quotes => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' if !in_quotes && paren_depth > 0 => {
                paren_depth -= 1;
                current.push(ch);
            }
            ',' if !in_quotes && bracket_depth == 0 && paren_depth == 0 => {
                // This comma is a parameter separator
                if !current.trim().is_empty() {
                    params.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    // Add the last parameter
    if !current.trim().is_empty() {
        params.push(current.trim().to_string());
    }

    // Validate bracket/quote balance
    if in_quotes {
        return Err(CommandError::ParameterError(
            "Unclosed quoted string in parameters".to_string(),
        ));
    }
    if bracket_depth != 0 {
        return Err(CommandError::ParameterError(
            "Mismatched brackets in parameters".to_string(),
        ));
    }
    if paren_depth != 0 {
        return Err(CommandError::ParameterError(
            "Mismatched parentheses in parameters".to_string(),
        ));
    }

    Ok(params)
}

/// Save AST to JSON file
pub fn cmd_write(app: &mut App, filename: &str) -> CommandResult<()> {
    if filename.is_empty() {
        app.current_file.clone().ok_or(CommandError::Custom(
            "No current file specified and no filename provided to write to".to_string(),
        ))?;
    } else {
        let expanded = expand_tilde(filename);

        // Check if file exists and warn user if it's different from current file
        if expanded.exists() {
            if let Some(ref current_file) = app.current_file {
                let current_expanded = expand_tilde(current_file);
                if expanded != current_expanded {
                    return Err(CommandError::Custom(format!(
                        "File '{}' exists, use w!, write! or save! to force write",
                        filename
                    )));
                }
            } else {
                return Err(CommandError::Custom(format!(
                    "File '{}' exists, use w!, write! or save! to force write",
                    filename
                )));
            }
        }
    }
    cmd_write_force(app, filename)
}

/// Force save AST to JSON file
pub fn cmd_write_force(app: &mut App, filename: &str) -> CommandResult<()> {
    // Expand tilde in filename
    let expanded_filepath = if filename.is_empty() {
        let current_file_str = app.current_file.clone().ok_or(CommandError::Custom(
            "No current file specified and no filename provided to write to".to_string(),
        ))?;
        expand_tilde(current_file_str)
    } else {
        expand_tilde(filename)
    };

    // Ensure filename ends with .json
    let filepath = if !expanded_filepath
        .extension()
        .map(|ext| ext == "json")
        .unwrap_or(false)
    {
        expanded_filepath.with_extension("json")
    } else {
        expanded_filepath
    };

    // Serialize AST to JSON
    let json = serde_json::to_string_pretty(&*app.ast)
        .map_err(|e| CommandError::Custom(format!("Failed to serialize AST: {}", e)))?;

    // Write to file
    fs::write(&filepath, json).map_err(|e| {
        CommandError::Custom(format!(
            "Failed to write file '{}': {}",
            filepath.display(),
            e
        ))
    })?;

    // If app.current_file is None but we're saving with a filename, update current_file
    // This handles the case where we're saving a new unnamed file
    if app.current_file.is_none() && !filename.is_empty() {
        app.current_file = Some(filename.to_string());
    }
    app.mark_saved();

    Ok(())
}

/// Load AST from JSON file
pub fn cmd_load(app: &mut App, filename: &str) -> CommandResult<()> {
    if !app.saved {
        return Err(CommandError::Custom(
            "File is not saved, use 'e!' or 'edit!' to force load file".to_string(),
        ));
    }
    cmd_load_force(app, filename)
}

/// Force load AST from JSON file
pub fn cmd_load_force(app: &mut App, filename: &str) -> CommandResult<()> {
    // Expand tilde in filename
    let expanded_filename = expand_tilde(filename);

    // Check file exists
    if !expanded_filename.exists() {
        return Err(CommandError::Custom(format!(
            "File '{}' not found",
            expanded_filename.display()
        )));
    }

    // Read file
    let content = fs::read_to_string(&expanded_filename).map_err(|e| {
        CommandError::Custom(format!(
            "Failed to read file '{}': {}",
            expanded_filename.display(),
            e
        ))
    })?;

    // Deserialize from JSON
    let ast = serde_json::from_str(&content)
        .map_err(|e| CommandError::Custom(format!("Failed to parse JSON: {}", e)))?;

    // Replace AST
    app.ast = Arc::new(ast);
    app.model_preview.mark_stale();

    // First, reload custom modules in library manager
    app.library
        .reload_custom_modules_from_ast(&app.ast.module_defines);
    app.library
        .reload_custom_functions_from_ast(&app.ast.function_defines);

    // Reload libraries that were used in the project
    for library_file in &app.ast.loaded_libraries {
        let expanded_path = expand_tilde(library_file);
        if expanded_path.exists() {
            if let Err(e) = app.library.load_library(&expanded_path) {
                // Log warning but continue loading other libraries
                eprintln!("Warning: Could not load library '{}': {}", library_file, e);
            }
        } else {
            // Library file doesn't exist anymore
            eprintln!("Warning: Library file '{}' no longer exists", library_file);
        }
    }

    // Reset navigation state
    app.selected_nodes.clear();
    app.tree_state.borrow_mut().select(Vec::new());
    app.current_file = Some(filename.to_string());

    Ok(())
}

/// Export AST to OpenSCAD code file
pub fn cmd_export(app: &App, filename: &str) -> CommandResult<()> {
    // Expand tilde in filename
    let expanded_filepath = expand_tilde(filename);

    // Ensure filename ends with .scad
    let filepath = if !expanded_filepath
        .extension()
        .map(|ext| ext == "scad")
        .unwrap_or(false)
    {
        expanded_filepath.with_extension("scad")
    } else {
        expanded_filepath
    };

    // Generate OpenSCAD code
    let code = app.ast.to_scad();

    // Write to file
    fs::write(&filepath, code).map_err(|e| {
        CommandError::Custom(format!(
            "Failed to write file '{}': {}",
            filepath.display(),
            e
        ))
    })?;

    Ok(())
}

/// Load a library from a JSON file
/// This command loads third-party module libraries into the LibraryManager
/// Libraries should be in JSON format with the same schema as stdlib.json
pub fn cmd_load_library(app: &mut App, filename: &str) -> CommandResult<()> {
    // Expand tilde in filename
    let expanded_path = expand_tilde(filename);

    // Check if file exists
    if !expanded_path.exists() {
        return Err(CommandError::Custom(format!(
            "Library file '{}' not found",
            expanded_path.display()
        )));
    }

    // Load library
    app.library.load_library(&expanded_path)?;

    // Read the library file to get the 'file' property
    let library_content = std::fs::read_to_string(&expanded_path).map_err(|e| {
        CommandError::Custom(format!(
            "Could not read library file '{}': {}",
            expanded_path.display(),
            e
        ))
    })?;

    if let Ok(library_def) = serde_json::from_str::<openscad_library::LibraryDef>(&library_content)
    {
        // Add the .scad file to includes so it's available when modules from this library are used
        if !app.ast.includes.contains(&library_def.file) {
            app.ast_mut().includes.push(library_def.file);
        }
    }

    // Record the library JSON file in the AST so it can be reloaded when the project is opened
    if !app.ast.loaded_libraries.contains(&filename.to_string()) {
        app.ast_mut().loaded_libraries.push(filename.to_string());
    }

    Ok(())
}

/// Define a global variable
/// Syntax: global <var_name>=<value>
/// Example: global width=100
///          global size=[10,20,30]
///          global $fn=50  (special variable)
pub fn cmd_global(app: &mut App, var_spec: &str) -> CommandResult<()> {
    use openscad_core::GlobalVariable;

    let var_spec = var_spec.trim();

    // Find the equals sign to separate name and value
    let equals_pos = var_spec.find('=');

    if equals_pos.is_none() {
        return Err(CommandError::InvalidCommand(
            "Invalid global variable syntax. Use: global <name>=<value>".to_string(),
        ));
    }

    let pos = equals_pos.unwrap();
    let name_part = var_spec[..pos].trim();
    let value_part = var_spec[pos + 1..].trim();

    // Validate identifier
    let identifier_body = name_part.strip_prefix('$').unwrap_or(name_part);
    if !is_valid_identifier(identifier_body) {
        return Err(CommandError::InvalidCommand(format!(
            "Invalid variable name: {}",
            name_part
        )));
    }

    // Parse the value
    let value = openscad_core::Expr::parse(value_part).map_err(|e| {
        CommandError::ParameterError(format!("Invalid value expression: {} - {}", value_part, e))
    })?;

    // Create global variable
    let global_var = GlobalVariable::new(name_part.to_string(), value);

    // Add or replace in the AST while preserving the definition's position.
    app.ast_mut()
        .upsert_global_variable(global_var)
        .map_err(CommandError::AstError)?;

    Ok(())
}

/// Helper function to validate identifier names
fn is_valid_identifier(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    let first = name.chars().next().unwrap();
    if !first.is_alphabetic() && first != '_' {
        return false;
    }

    name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Define a new custom function
/// Helper function to parse function signature with support for parentheses
fn parse_function_signature(sig: &str) -> CommandResult<(String, String)> {
    // Look for = to separate parameters/body
    let equals_pos = sig.find('=');

    if let Some(eq_pos) = equals_pos {
        let params_part = sig[..eq_pos].trim();
        let body_part = sig[eq_pos + 1..].trim();

        // Check if there are parentheses around parameters
        let cleaned_params = params_part.trim();

        // Check if it has parentheses format like (a, b, c)
        if cleaned_params.starts_with('(') && cleaned_params.ends_with(')') {
            let inner = &cleaned_params[1..cleaned_params.len() - 1]; // Remove parentheses
            Ok((inner.trim().to_string(), body_part.to_string()))
        } else {
            // Old format: a, b, c
            Ok((cleaned_params.to_string(), body_part.to_string()))
        }
    } else {
        // No equals sign - entire string is treated as body with no parameters
        Ok(("".to_string(), sig.trim().to_string()))
    }
}

pub fn cmd_funcdef(app: &mut App, func_def: &str) -> CommandResult<()> {
    use openscad_core::{Expr, FunctionDefinition, Parameter};

    let trimmed = func_def.trim();

    // Find where the function name ends and parameters begin
    if let Some(open_paren_pos) = trimmed.find('(') {
        let func_name = &trimmed[..open_paren_pos].trim();

        // Extract the part after the function name (should be "(params) = body")
        let params_and_body_part = &trimmed[open_paren_pos..];

        // Validate function name
        if !is_valid_identifier(func_name) {
            return Err(CommandError::InvalidCommand(format!(
                "Invalid function name: {}",
                func_name
            )));
        }

        // Parse parameters and body from the params_and_body_part
        let (params_part, body_part) = parse_function_signature(params_and_body_part)?;

        // Parse parameters
        let parameters = if params_part.is_empty() {
            Vec::new()
        } else {
            // Split parameters by comma
            let param_names: Vec<&str> = params_part.split(',').map(|s| s.trim()).collect();
            let mut params = Vec::new();
            for param_name in param_names {
                if !param_name.is_empty() {
                    params.push(Parameter::new(param_name.to_string()));
                }
            }
            params
        };

        // Parse body expression
        let body = Expr::parse(&body_part).map_err(|e| {
            CommandError::ParameterError(format!(
                "Invalid function body expression: {} - {}",
                body_part, e
            ))
        })?;

        // Create FunctionDefinition for AST
        let function_def = FunctionDefinition::new(func_name.to_string(), parameters.clone(), body);

        // Add to AST
        app.ast_mut()
            .upsert_function_define(function_def)
            .map_err(CommandError::AstError)?;
    } else {
        // No parentheses found - just a function name with no parameters
        let func_name = trimmed;

        if !is_valid_identifier(func_name) {
            return Err(CommandError::InvalidCommand(format!(
                "Invalid function name: {}",
                func_name
            )));
        }

        // Create FunctionDefinition for AST with empty parameters and placeholder body
        let function_def =
            FunctionDefinition::new(func_name.to_string(), Vec::new(), Expr::Integer(0));

        // Add to AST
        app.ast_mut()
            .upsert_function_define(function_def)
            .map_err(CommandError::AstError)?;
    }

    // Reload custom functions in library manager
    app.library
        .reload_custom_functions_from_ast(&app.ast.function_defines);

    Ok(())
}

/// Quit app command
pub fn cmd_quit(app: &mut App) -> CommandResult<()> {
    if !app.saved {
        return Err(CommandError::Custom(
            "File is not saved, use 'q!' or 'quit!' to force quit".to_string(),
        ));
    }
    cmd_quit_force(app)
}

/// Force Quit app command
pub fn cmd_quit_force(app: &mut App) -> CommandResult<()> {
    app.should_quit = true;
    Ok(())
}

/// Write and quit app command
pub fn cmd_write_and_quit(app: &mut App) -> CommandResult<()> {
    cmd_write(app, "")?;
    cmd_quit(app)
}

/// Undo command
pub fn cmd_undo(app: &mut App) -> CommandResult<()> {
    app.undo();
    Ok(())
}

/// Redo command
pub fn cmd_redo(app: &mut App) -> CommandResult<()> {
    app.redo();
    Ok(())
}

fn general_help_doc(app: &App) -> Vec<String> {
    let mut docs = vec![
        "OpenSCAD TUI - Command Reference".to_string(),
        "".to_string(),
        "Normal mode keys:".to_string(),
        "  j/k/h/l or arrows  navigate and expand/collapse the tree".to_string(),
        "  Enter              toggle node expansion".to_string(),
        "  v                  select/deselect current node".to_string(),
        "  y / p              yank / paste module subtree".to_string(),
        "  x                  remove node and promote its children".to_string(),
        "  c                  change current node (replace)".to_string(),
        "  i                  start insert command".to_string(),
        "  t/r/s              start translate/rotate/scale command".to_string(),
        "  d                  delete current or selected nodes".to_string(),
        "  u / Ctrl+R         undo / redo".to_string(),
        "  w/e/L              save / load project / load library".to_string(),
        "  :                  enter command mode".to_string(),
        "  ?                  open this help".to_string(),
        "  q / Ctrl+C         quit".to_string(),
        "".to_string(),
        "Commands (type `help <command>` for details):".to_string(),
    ];

    for name in app.command_registry.get_primary_names() {
        if let Some(def) = app.command_registry.find(&name) {
            docs.push(format!("  {:<34} {}", def.usage, def.description));
        }
    }

    docs.extend([
        "".to_string(),
        "Command mode: Tab completes, Up/Down browse history, Esc cancels.".to_string(),
        "Help: j/k or arrows scroll, Ctrl+F/Ctrl+B page, Esc/q closes.".to_string(),
    ]);
    docs
}

fn command_help_doc(app: &App, command: &str) -> CommandResult<Vec<String>> {
    let def = app.command_registry.find(command).ok_or_else(|| {
        CommandError::InvalidCommand(format!("No help found for command: {}", command))
    })?;
    let aliases = if def.aliases.is_empty() {
        "(none)".to_string()
    } else {
        def.aliases.join(", ")
    };
    let mut docs = vec![
        format!("Help: {}", def.name),
        "".to_string(),
        format!("Description: {}", def.description),
        format!("Usage: {}", def.usage),
        format!("Aliases: {}", aliases),
    ];
    if !def.examples.is_empty() {
        docs.push("".to_string());
        docs.push("Examples:".to_string());
        docs.extend(def.examples.iter().map(|example| format!("  {}", example)));
    }
    docs.extend(["".to_string(), "Press Esc or q to close help.".to_string()]);
    Ok(docs)
}

/// Help command - Show the command overview or details for one command.
pub fn cmd_help(app: &mut App, command: Option<&str>) -> CommandResult<()> {
    let docs = match command {
        Some(command) => command_help_doc(app, command)?,
        None => general_help_doc(app),
    };
    app.set_help_doc(docs);
    app.input_mode = InputMode::Help;
    Ok(())
}

/// Expand tilde (~) in file paths to the user's home directory
fn expand_tilde<P: AsRef<Path>>(path: P) -> PathBuf {
    let path = path.as_ref();

    if path.starts_with("~") {
        if let Some(home) = dirs::home_dir() {
            // Replace ~ with home directory
            let mut expanded = home;
            if path.components().count() > 1 {
                // Add remaining components after ~
                for component in path.components().skip(1) {
                    expanded.push(component);
                }
            }
            expanded
        } else {
            // If no home directory found, return original path
            path.to_path_buf()
        }
    } else {
        // If path doesn't start with ~, return as-is
        path.to_path_buf()
    }
}

/// Define a new custom module
pub fn cmd_moddef(app: &mut App, module_name: &str, params: Option<&str>) -> CommandResult<()> {
    use openscad_core::ModuleDefinition;

    // Parse parameters
    let parameters = if let Some(param_str) = params {
        parse_module_parameters(param_str)?
    } else {
        Vec::new()
    };

    // Collect children from selected nodes (copy them with new IDs to avoid duplication)
    let mut children = Vec::new();
    for node_id in &app.selected_nodes {
        if let Some(node) = app.ast.find_node_by_id(node_id).cloned() {
            let node_with_new_id = clone_module_with_new_ids(&node);
            children.push(node_with_new_id);
        }
    }

    // Clear selection after copying
    app.selected_nodes.clear();

    // Create ModuleDefinition for AST
    let module_def = ModuleDefinition::new(module_name.to_string(), parameters, children);

    // Add or replace in the AST while preserving the definition's position.
    app.ast_mut()
        .upsert_module_define(module_def)
        .map_err(CommandError::AstError)?;
    app.library
        .reload_custom_modules_from_ast(&app.ast.module_defines);

    // Update UI selection to show the new module definition
    if let Some(path) = app.find_node_path(&format!("__moddef_{}", module_name)) {
        app.tree_state.borrow_mut().select(path);
    }

    Ok(())
}

/// Parse module parameters from string
/// Format: "name1=expr1, name2=expr2, name3" (name without default)
fn parse_module_parameters(param_str: &str) -> CommandResult<Vec<openscad_core::Parameter>> {
    use openscad_core::{Expr, Parameter};

    let mut parameters = Vec::new();

    if param_str.trim().is_empty() {
        return Ok(parameters);
    }

    // Split by commas while respecting brackets and quotes (reuse split_parameters)
    let parts = split_parameters(param_str)?;

    for part in parts {
        let part = part.trim();

        // Check if this is a parameter with default value (contains '=')
        if let Some(eq_pos) = part.find('=') {
            let name = part[..eq_pos].trim();
            let value_str = part[eq_pos + 1..].trim();

            let value = Expr::parse(value_str).map_err(|e| {
                CommandError::ParameterError(format!(
                    "Invalid default value for parameter '{}': {} - {}",
                    name, value_str, e
                ))
            })?;

            parameters.push(Parameter::with_default(name.to_string(), value));
        } else {
            // Parameter without default value
            parameters.push(Parameter::new(part.to_string()));
        }
    }

    Ok(parameters)
}

/// Clone a module node and all its children, generating new unique IDs for each
fn clone_module_with_new_ids(node: &openscad_core::ModuleNode) -> openscad_core::ModuleNode {
    // Generate new ID with timestamp to ensure uniqueness
    let new_id = format!(
        "{}_{}",
        node.name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    // Clone the node with new ID
    let mut new_node = node.clone();
    new_node.id = new_id.clone();

    // Recursively clone children with new IDs
    new_node.children = node
        .children
        .iter()
        .map(clone_module_with_new_ids)
        .collect();

    new_node
}

fn selected_tree_id(app: &App) -> CommandResult<String> {
    app.tree_state
        .borrow()
        .selected()
        .last()
        .cloned()
        .ok_or(CommandError::NoNodeSelected)
}

fn selected_or_current_node_ids(app: &App) -> CommandResult<Vec<String>> {
    if !app.selected_nodes.is_empty() {
        Ok(app.selected_nodes.clone())
    } else {
        Ok(vec![selected_tree_id(app)?])
    }
}

enum PlannedParameterUpdate {
    Existing(ArgumentSelector),
    AddNamed,
}

fn plan_parameter_update(
    app: &App,
    node_id: &str,
    parameter_name: &str,
) -> CommandResult<PlannedParameterUpdate> {
    let node = find_module_node(app, node_id)
        .ok_or_else(|| CommandError::InvalidCommand(format!("Node not found: {}", node_id)))?;
    let definition = app
        .library
        .get_module(&node.name)
        .ok_or_else(|| CommandError::InvalidCommand(format!("Unknown module: {}", node.name)))?;
    let parameter_position = definition
        .parameters
        .iter()
        .position(|parameter| parameter.name == parameter_name)
        .ok_or_else(|| {
            CommandError::InvalidCommand(format!(
                "Module '{}' has no parameter named '{}'",
                node.name, parameter_name
            ))
        })?;

    if node
        .args
        .iter()
        .any(|argument| matches!(argument, Argument::Named { name, .. } if name == parameter_name))
    {
        return Ok(PlannedParameterUpdate::Existing(ArgumentSelector::Named(
            parameter_name.to_string(),
        )));
    }
    if node
        .args
        .iter()
        .filter(|argument| matches!(argument, Argument::Positional(_)))
        .nth(parameter_position)
        .is_some()
    {
        return Ok(PlannedParameterUpdate::Existing(
            ArgumentSelector::Position(parameter_position),
        ));
    }
    Ok(PlannedParameterUpdate::AddNamed)
}

pub fn cmd_set_parameter(app: &mut App, parameter_spec: &str) -> CommandResult<()> {
    let (parameter_name, value_source) = parameter_spec.split_once('=').ok_or_else(|| {
        CommandError::InvalidCommand("Usage: set <parameter_name>=<expression>".to_string())
    })?;
    let parameter_name = parameter_name.trim();
    let value_source = value_source.trim();
    if parameter_name.is_empty() || value_source.is_empty() {
        return Err(CommandError::InvalidCommand(
            "Usage: set <parameter_name>=<expression>".to_string(),
        ));
    }
    let value = Expr::parse(value_source).map_err(|error| {
        CommandError::ParameterError(format!(
            "Invalid value for '{}': {} - {}",
            parameter_name, value_source, error
        ))
    })?;
    let target_ids = selected_or_current_node_ids(app)?;
    if target_ids.iter().any(|node_id| node_id.starts_with("__")) {
        return Err(CommandError::InvalidCommand(
            "Only module node parameters can be changed".to_string(),
        ));
    }

    // Plan every update before mutating the AST so multi-node edits are atomic.
    let updates = target_ids
        .iter()
        .map(|node_id| {
            plan_parameter_update(app, node_id, parameter_name)
                .map(|update| (node_id.clone(), update))
        })
        .collect::<CommandResult<Vec<_>>>()?;

    for (node_id, update) in updates {
        let node = app
            .ast_mut()
            .find_node_anywhere_mut(&node_id)
            .ok_or_else(|| CommandError::InvalidCommand(format!("Node not found: {}", node_id)))?;
        match update {
            PlannedParameterUpdate::Existing(selector) => {
                node.set_argument(&selector, value.clone())?;
            }
            PlannedParameterUpdate::AddNamed => {
                node.add_named_argument(parameter_name.to_string(), value.clone())?;
            }
        }
    }
    app.set_info(&format!(
        "Set '{}' on {} node(s)",
        parameter_name,
        target_ids.len()
    ));
    Ok(())
}

pub fn cmd_unset_parameter(app: &mut App, parameter_name: &str) -> CommandResult<()> {
    let parameter_name = parameter_name.trim();
    if parameter_name.is_empty() {
        return Err(CommandError::InvalidCommand(
            "Usage: unset <parameter_name>".to_string(),
        ));
    }
    let target_ids = selected_or_current_node_ids(app)?;
    if target_ids.iter().any(|node_id| node_id.starts_with("__")) {
        return Err(CommandError::InvalidCommand(
            "Only module node parameters can be removed".to_string(),
        ));
    }

    // Resolve every selector before mutating anything so multi-node edits are atomic.
    let removals = target_ids
        .iter()
        .map(
            |node_id| match plan_parameter_update(app, node_id, parameter_name)? {
                PlannedParameterUpdate::Existing(selector) => Ok((node_id.clone(), selector)),
                PlannedParameterUpdate::AddNamed => Err(CommandError::InvalidCommand(format!(
                    "Parameter '{}' is not explicitly set on node '{}'",
                    parameter_name, node_id
                ))),
            },
        )
        .collect::<CommandResult<Vec<_>>>()?;

    for (node_id, selector) in removals {
        app.ast_mut()
            .find_node_anywhere_mut(&node_id)
            .ok_or_else(|| CommandError::InvalidCommand(format!("Node not found: {}", node_id)))?
            .remove_argument(&selector)?;
    }
    app.set_info(&format!(
        "Unset '{}' on {} node(s)",
        parameter_name,
        target_ids.len()
    ));
    Ok(())
}

fn find_module_node(app: &App, node_id: &str) -> Option<ModuleNode> {
    app.ast.find_node_anywhere(node_id).cloned()
}

pub fn cmd_yank(app: &mut App, node_id: Option<&str>) -> CommandResult<()> {
    let node_id = app
        .selected_nodes
        .last()
        .cloned()
        .or_else(|| node_id.map(str::to_string))
        .map(Ok)
        .unwrap_or_else(|| selected_tree_id(app))?;
    if node_id.starts_with("__") {
        return Err(CommandError::InvalidCommand(
            "Only module nodes can be yanked".to_string(),
        ));
    }
    let node = find_module_node(app, &node_id)
        .ok_or_else(|| CommandError::InvalidCommand(format!("Node not found: {}", node_id)))?;
    app.node_clipboard = Some(node);
    app.set_info(&format!("Yanked node: {}", node_id));
    Ok(())
}

pub fn cmd_paste(app: &mut App) -> CommandResult<String> {
    let clipboard = app
        .node_clipboard
        .as_ref()
        .ok_or_else(|| CommandError::InvalidCommand("Clipboard is empty".to_string()))?;
    let pasted = clone_module_with_new_ids(clipboard);
    let pasted_id = pasted.id.clone();
    let target_id = selected_tree_id(app).unwrap_or_else(|_| "__modules".to_string());

    if target_id == "__modules" {
        app.ast_mut().modules.push(pasted);
    } else if let Some(module_name) = target_id.strip_prefix("__moddef_") {
        let definition = app
            .ast_mut()
            .module_defines
            .iter_mut()
            .find(|definition| definition.name == module_name)
            .ok_or_else(|| {
                CommandError::InvalidCommand(format!(
                    "Module definition not found: {}",
                    module_name
                ))
            })?;
        definition.body.push(pasted);
    } else if target_id.starts_with("__") {
        return Err(CommandError::InvalidCommand(
            "Select a module node or the Modules section before pasting".to_string(),
        ));
    } else {
        app.ast_mut().insert_after(&target_id, pasted)?;
    }

    if let Some(path) = app.find_node_path(&pasted_id) {
        app.tree_state.borrow_mut().select(path);
    }
    app.set_info(&format!("Pasted node: {}", pasted_id));
    Ok(pasted_id)
}

pub fn cmd_remove(app: &mut App, node_id: Option<&str>) -> CommandResult<()> {
    let node_ids = if app.selected_nodes.is_empty() {
        match node_id {
            Some(node_id) => vec![node_id.to_string()],
            None => selected_or_current_node_ids(app)?,
        }
    } else {
        selected_or_current_node_ids(app)?
    };
    if node_ids.iter().any(|node_id| node_id.starts_with("__")) {
        return Err(CommandError::InvalidCommand(
            "Only module nodes can be removed".to_string(),
        ));
    }

    for node_id in &node_ids {
        app.ast_mut().remove_node_promote_children(node_id)?;
    }

    app.selected_nodes.clear();
    app.restore_tree_selection();
    app.set_info(&format!("Removed {} node(s)", node_ids.len()));
    Ok(())
}

pub fn cmd_replace(
    app: &mut App,
    node_id: Option<&str>,
    new_module_name: &str,
    params: Option<&str>,
) -> CommandResult<String> {
    let node_ids = if app.selected_nodes.is_empty() {
        match node_id {
            Some(node_id) => vec![node_id.to_string()],
            None => selected_or_current_node_ids(app)?,
        }
    } else {
        selected_or_current_node_ids(app)?
    };
    if node_ids.iter().any(|node_id| node_id.starts_with("__")) {
        return Err(CommandError::InvalidCommand(
            "Only module nodes can be replaced".to_string(),
        ));
    }
    let mut replacement_id = None;
    for node_id in node_ids {
        if find_module_node(app, &node_id).is_none() {
            continue;
        }
        let prepared = prepare_module(app, new_module_name, params)?;
        replacement_id = Some(replace_with_prepared_module(
            app,
            &node_id,
            new_module_name,
            prepared,
        )?);
    }
    replacement_id
        .ok_or_else(|| CommandError::InvalidCommand("None of the target nodes exist".to_string()))
}

fn replace_with_prepared_module(
    app: &mut App,
    node_id: &str,
    new_module_name: &str,
    prepared: PreparedModule,
) -> CommandResult<String> {
    if find_module_node(app, node_id).is_none() {
        return Err(CommandError::InvalidCommand(format!(
            "Node not found: {}",
            node_id
        )));
    }

    let replacement_id = prepared.node.id.clone();
    add_module_include(app, prepared.source_file.as_ref());

    app.ast_mut().replace_node(node_id, prepared.node)?;

    app.selected_nodes.retain(|selected| selected != node_id);
    if let Some(path) = app.find_node_path(&replacement_id) {
        app.tree_state.borrow_mut().select(path);
    }
    app.set_info(&format!("Replaced {} with {}", node_id, new_module_name));
    Ok(replacement_id)
}

pub fn begin_pending_module_action(app: &mut App, action: PendingModuleAction, module_name: &str) {
    app.pending_module_action = Some(action);
    app.pending_module_name = Some(module_name.to_string());
    app.input_mode = InputMode::ModuleEnterParams;
    app.input_buffer.clear();
    app.set_info(&format!(
        "Enter parameters for '{}' (or press Enter to use defaults):",
        module_name
    ));
}

pub fn commit_pending_module_action(app: &mut App, params: &str) -> CommandResult<String> {
    let action = app
        .pending_module_action
        .clone()
        .ok_or_else(|| CommandError::InvalidCommand("No pending module action".to_string()))?;
    let module_name = app.pending_module_name.clone().ok_or_else(|| {
        CommandError::InvalidCommand("No module selected for pending action".to_string())
    })?;

    // Validate module lookup and parameters before creating the single undo point.
    let prepared = prepare_module(app, &module_name, Some(params))?;
    app.push_undo();
    let result = match action {
        PendingModuleAction::Insert => insert_prepared_module(app, &module_name, prepared),
        PendingModuleAction::Replace { target_ids } => {
            let mut result = None;
            let mut first_prepared = Some(prepared);
            for target_id in &target_ids {
                if find_module_node(app, target_id).is_none() {
                    continue;
                }
                let prepared = match first_prepared.take() {
                    Some(prepared) => prepared,
                    None => prepare_module(app, &module_name, Some(params))?,
                };
                result = Some(replace_with_prepared_module(
                    app,
                    target_id,
                    &module_name,
                    prepared,
                )?);
            }
            result.ok_or_else(|| {
                CommandError::InvalidCommand("None of the target nodes exist".to_string())
            })
        }
    }?;
    app.update_navigation_status();
    Ok(result)
}

/// Find the parent ID of a node in a module tree
fn find_node_parent(nodes: &[openscad_core::ModuleNode], target_id: &str) -> Option<String> {
    // Use explicit stack to avoid recursion depth issues
    let mut stack: Vec<(&openscad_core::ModuleNode, usize)> =
        nodes.iter().map(|n| (n, 0)).collect();

    while let Some((node, depth)) = stack.pop() {
        if depth >= MAX_RECURSION_DEPTH {
            continue; // Skip to avoid infinite recursion
        }

        // Check if target is a direct child of this node
        for child in &node.children {
            if child.id == target_id {
                return Some(node.id.clone());
            }
            // Push child to stack for deeper search
            stack.push((child, depth + 1));
        }
    }

    None
}

/// Insert a child node before a target node in its parent's children list
#[allow(clippy::ptr_arg)]
fn insert_child_before(
    nodes: &mut Vec<openscad_core::ModuleNode>,
    parent_id: &str,
    target_id: &str,
    new_node: openscad_core::ModuleNode,
) -> Result<(), String> {
    // Find the parent node
    for node in nodes.iter_mut() {
        if node.id == parent_id {
            // Find position of target child
            if let Some(pos) = node.children.iter().position(|c| c.id == target_id) {
                node.children.insert(pos, new_node);
                return Ok(());
            } else {
                return Err(format!(
                    "Target node {} not found in parent {}",
                    target_id, parent_id
                ));
            }
        }

        // Recursively search in children
        if !node.children.is_empty() {
            // Use explicit stack to avoid recursion depth issues
            let mut stack: Vec<&mut Vec<openscad_core::ModuleNode>> = vec![&mut node.children];
            let mut depth = 0;

            while let Some(children) = stack.pop() {
                if depth >= MAX_RECURSION_DEPTH {
                    break;
                }

                for child in children.iter_mut() {
                    if child.id == parent_id {
                        if let Some(pos) = child.children.iter().position(|c| c.id == target_id) {
                            child.children.insert(pos, new_node);
                            return Ok(());
                        } else {
                            return Err(format!(
                                "Target node {} not found in parent {}",
                                target_id, parent_id
                            ));
                        }
                    }

                    if !child.children.is_empty() {
                        stack.push(&mut child.children);
                    }
                }
                depth += 1;
            }
        }
    }

    Err(format!("Parent node {} not found", parent_id))
}

/// Find a node in a module definition body
fn find_node_in_module_definition(
    nodes: &[openscad_core::ModuleNode],
    target_id: &str,
) -> Option<openscad_core::ModuleNode> {
    // Use explicit stack to avoid recursion depth issues
    let mut stack: Vec<(&openscad_core::ModuleNode, usize)> =
        nodes.iter().map(|n| (n, 0)).collect();

    while let Some((node, depth)) = stack.pop() {
        if depth >= MAX_RECURSION_DEPTH {
            continue;
        }

        if node.id == target_id {
            return Some(node.clone());
        }

        // Push children to stack
        for child in &node.children {
            stack.push((child, depth + 1));
        }
    }

    None
}

/// Find a node in a module definition body (mutable version)
fn find_node_in_module_definition_mut<'a>(
    nodes: &'a mut [openscad_core::ModuleNode],
    target_id: &str,
) -> Option<&'a mut openscad_core::ModuleNode> {
    // Use explicit stack to avoid recursion depth issues
    let mut stack: Vec<(&mut openscad_core::ModuleNode, usize)> =
        nodes.iter_mut().map(|n| (n, 0)).collect();

    while let Some((node, depth)) = stack.pop() {
        if depth >= MAX_RECURSION_DEPTH {
            continue;
        }

        if node.id == target_id {
            return Some(node);
        }

        // Push children to stack
        for child in &mut node.children {
            stack.push((child, depth + 1));
        }
    }

    None
}

/// Delete a node from a module definition body
fn delete_node_from_module_definition(
    nodes: &mut Vec<openscad_core::ModuleNode>,
    target_id: &str,
) -> Result<(), String> {
    // First try to find and remove from root level
    if let Some(pos) = nodes.iter().position(|n| n.id == target_id) {
        nodes.remove(pos);
        return Ok(());
    }

    // Search in children recursively
    for node in nodes.iter_mut() {
        // Use explicit stack to avoid recursion depth issues
        let mut stack: Vec<&mut Vec<openscad_core::ModuleNode>> = vec![&mut node.children];
        let mut depth = 0;

        while let Some(children) = stack.pop() {
            if depth >= MAX_RECURSION_DEPTH {
                break;
            }

            if let Some(pos) = children.iter().position(|n| n.id == target_id) {
                children.remove(pos);
                return Ok(());
            }

            // Continue search in deeper children
            for child in children.iter_mut() {
                if !child.children.is_empty() {
                    stack.push(&mut child.children);
                }
            }
            depth += 1;
        }
    }

    Err(format!("Node {} not found", target_id))
}

/// Initialize the command registry with all available commands
pub fn init_command_registry(registry: &mut crate::command_registry::CommandRegistry) {
    use crate::command_registry::CommandDef;

    // Navigation commands (no arguments)
    registry.register(CommandDef::new(
        "next",
        vec!["j", "down"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "next command takes no arguments".to_string(),
                ));
            }
            cmd_next(app)
        },
        "Move cursor down",
        0,
        Some(0),
        "next",
        vec!["j", "next"],
        CommandType::NoArg,
        false,
        false,
    ));

    registry.register(CommandDef::new(
        "prev",
        vec!["k", "up"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "prev command takes no arguments".to_string(),
                ));
            }
            cmd_prev(app)
        },
        "Move cursor up",
        0,
        Some(0),
        "prev",
        vec!["k", "prev"],
        CommandType::NoArg,
        false,
        false,
    ));

    registry.register(CommandDef::new(
        "collapse",
        vec!["h", "left"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "collapse command takes no arguments".to_string(),
                ));
            }
            cmd_collapse(app)
        },
        "Collapse node or move left",
        0,
        Some(0),
        "collapse",
        vec!["h", "collapse"],
        CommandType::NoArg,
        false,
        false,
    ));

    registry.register(CommandDef::new(
        "expand",
        vec!["l", "right"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "expand command takes no arguments".to_string(),
                ));
            }
            cmd_expand(app)
        },
        "Expand node or move right",
        0,
        Some(0),
        "expand",
        vec!["l", "expand"],
        CommandType::NoArg,
        false,
        false,
    ));

    registry.register(CommandDef::new(
        "toggle",
        vec![] as Vec<String>,
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "toggle command takes no arguments".to_string(),
                ));
            }
            cmd_toggle(app)
        },
        "Toggle node expansion",
        0,
        Some(0),
        "toggle",
        vec!["toggle"],
        CommandType::NoArg,
        false,
        false,
    ));

    // Selection commands
    registry.register(CommandDef::new(
        "select",
        vec!["v"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "select command takes no arguments".to_string(),
                ));
            }
            cmd_select_toggle(app)
        },
        "Select/deselect current node",
        0,
        Some(0),
        "select",
        vec!["v", "select"],
        CommandType::NoArg,
        false,
        false,
    ));

    registry.register(CommandDef::new(
        "deselect-all",
        vec!["deselect_all", "clear-selection"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "deselect-all command takes no arguments".to_string(),
                ));
            }
            cmd_deselect_all(app)
        },
        "Clear all selections",
        0,
        Some(0),
        "deselect-all",
        vec!["deselect-all"],
        CommandType::NoArg,
        false,
        false,
    ));

    // Edit commands
    registry.register(CommandDef::new(
        "undo",
        vec!["u"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "undo command takes no arguments".to_string(),
                ));
            }
            cmd_undo(app)
        },
        "Undo last operation",
        0,
        Some(0),
        "undo",
        vec!["u", "undo"],
        CommandType::NoArg,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "redo",
        vec!["r"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "redo command takes no arguments".to_string(),
                ));
            }
            cmd_redo(app)
        },
        "Redo last undone operation",
        0,
        Some(0),
        "redo",
        vec!["r", "redo"],
        CommandType::NoArg,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "delete",
        vec!["d", "dd", "D"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "delete command takes no arguments".to_string(),
                ));
            }
            app.push_undo();
            cmd_delete(app, "")
        },
        "Delete selected module subtrees or the current node, global, or function definition",
        0,
        Some(0),
        "delete",
        vec!["delete", "d", "dd", "D"],
        CommandType::NoArg,
        true,
        true,
    ));

    // File operations
    registry.register(CommandDef::new(
        "write",
        vec!["save", "w"],
        |app, args| {
            if args.len() > 1 {
                return Err(CommandError::InvalidCommand(
                    "write command takes at most 1 argument".to_string(),
                ));
            }
            let file_name = if let Some(file) = args.first() {
                (*file).to_string()
            } else {
                // Use current selection (handled in cmd_delete)
                String::new()
            };
            cmd_write(app, &file_name)
        },
        "Save AST to JSON file",
        0,
        Some(1),
        "write [filename]",
        vec!["write test.json", "save project.json"],
        CommandType::File,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "write!",
        vec!["save!", "w!"],
        |app, args| {
            if args.len() > 1 {
                return Err(CommandError::InvalidCommand(
                    "write! command takes at most 1 argument".to_string(),
                ));
            }
            let file_name = if let Some(file) = args.first() {
                (*file).to_string()
            } else {
                // Use current selection (handled in cmd_delete)
                String::new()
            };
            cmd_write_force(app, &file_name)
        },
        "Force save AST to JSON file",
        0,
        Some(1),
        "write! [filename]",
        vec!["write! test.json", "save! project.json"],
        CommandType::File,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "edit",
        vec!["e"],
        |app, args| {
            if args.len() > 1 {
                return Err(CommandError::InvalidCommand(
                    "edit command takes at most 1 argument".to_string(),
                ));
            }
            let file_name = if let Some(file) = args.first() {
                (*file).to_string()
            } else {
                // Use current selection (handled in cmd_delete)
                String::new()
            };
            cmd_load(app, &file_name)
        },
        "Load AST from JSON file",
        0,
        Some(1),
        "edit [filename]",
        vec!["edit test.json", "e project.json"],
        CommandType::File,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "edit!",
        vec!["e!"],
        |app, args| {
            if args.len() > 1 {
                return Err(CommandError::InvalidCommand(
                    "edit! command takes at most 1 argument".to_string(),
                ));
            }
            let file_name = if let Some(file) = args.first() {
                (*file).to_string()
            } else {
                // Use current selection (handled in cmd_delete)
                String::new()
            };
            cmd_load_force(app, &file_name)
        },
        "Force load AST from JSON file",
        0,
        Some(1),
        "edit! [filename]",
        vec!["edit! test.json", "e! project.json"],
        CommandType::File,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "export",
        vec![] as Vec<String>,
        |app, args| {
            if args.len() != 1 {
                return Err(CommandError::InvalidCommand(
                    "export command requires a filename".to_string(),
                ));
            }
            cmd_export(app, args[0])
        },
        "Export AST to OpenSCAD file",
        1,
        Some(1),
        "export <filename>",
        vec!["export model.scad"],
        CommandType::File,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "library",
        vec![] as Vec<String>,
        |app, args| {
            if args.len() != 1 {
                return Err(CommandError::InvalidCommand(
                    "library command requires a filename".to_string(),
                ));
            }
            cmd_load_library(app, args[0])
        },
        "Load a library from JSON file",
        1,
        Some(1),
        "library <filename>",
        vec!["library mylib.json"],
        CommandType::File,
        true,
        true,
    ));

    // System commands
    registry.register(CommandDef::new(
        "quit",
        vec!["q"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "quit command takes no arguments".to_string(),
                ));
            }
            cmd_quit(app)
        },
        "Exit the application",
        0,
        Some(0),
        "quit",
        vec!["quit", "q"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "quit!",
        vec!["q!"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "quit command takes no arguments".to_string(),
                ));
            }
            cmd_quit_force(app)
        },
        "Force exit the application",
        0,
        Some(0),
        "quit!",
        vec!["quit!", "q!"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "wq",
        vec!["wq"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "wq command takes no arguments".to_string(),
                ));
            }
            cmd_write_and_quit(app)
        },
        "Save and exit the application",
        0,
        Some(0),
        "wq",
        vec!["wq"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "help",
        vec!["?"],
        |app, args| {
            if args.len() > 1 {
                return Err(CommandError::InvalidCommand(
                    "help command takes at most 1 argument".to_string(),
                ));
            }

            cmd_help(app, args.first().copied())
        },
        "Show help",
        0,
        Some(1),
        "help [command]",
        vec!["help", "help write", "?"],
        CommandType::NoArg,
        false,
        true,
    ));

    // Transform commands
    registry.register(CommandDef::new(
        "translate",
        vec![] as Vec<String>,
        |app, args| {
            // Get parameters if provided
            let params = if !args.is_empty() {
                Some(args.join(" "))
            } else {
                None
            };

            app.push_undo();
            cmd_insert(app, "translate", None, params.as_deref()).map(|_| {
                app.set_info("Applied translate to selected nodes");
                app.update_navigation_status();
            })
        },
        "Apply translate transformation to selected nodes",
        0,
        None, // Variable number of parameters (optional)
        "translate [x,y,z]",
        vec!["translate", "translate 10,0,0"],
        CommandType::Param,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "rotate",
        vec![] as Vec<String>,
        |app, args| {
            let params = if !args.is_empty() {
                Some(args.join(" "))
            } else {
                None
            };

            app.push_undo();
            cmd_insert(app, "rotate", None, params.as_deref()).map(|_| {
                app.set_info("Applied rotate to selected nodes");
                app.update_navigation_status();
            })
        },
        "Apply rotate transformation to selected nodes",
        0,
        None,
        "rotate [a,vx,vy,vz]",
        vec!["rotate", "rotate 45,0,0,1"],
        CommandType::Param,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "scale",
        vec![] as Vec<String>,
        |app, args| {
            let params = if !args.is_empty() {
                Some(args.join(" "))
            } else {
                None
            };

            app.push_undo();
            cmd_insert(app, "scale", None, params.as_deref()).map(|_| {
                app.set_info("Applied scale to selected nodes");
                app.update_navigation_status();
            })
        },
        "Apply scale transformation to selected nodes",
        0,
        None,
        "scale [x,y,z]",
        vec!["scale", "scale 2,2,2"],
        CommandType::Param,
        true,
        true,
    ));

    // Boolean commands
    registry.register(CommandDef::new(
        "union",
        vec![] as Vec<String>,
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "union command takes no arguments".to_string(),
                ));
            }

            app.push_undo();
            cmd_insert(app, "union", None, None).map(|_| {
                app.set_info("Applied union to selected nodes");
                app.update_navigation_status();
            })
        },
        "Apply union operation to selected nodes",
        0,
        Some(0),
        "union",
        vec!["union"],
        CommandType::NoArg,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "difference",
        vec![] as Vec<String>,
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "difference command takes no arguments".to_string(),
                ));
            }

            app.push_undo();
            cmd_insert(app, "difference", None, None).map(|_| {
                app.set_info("Applied difference to selected nodes");
                app.update_navigation_status();
            })
        },
        "Apply difference operation to selected nodes",
        0,
        Some(0),
        "difference",
        vec!["difference"],
        CommandType::NoArg,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "intersection",
        vec![] as Vec<String>,
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "intersection command takes no arguments".to_string(),
                ));
            }

            app.push_undo();
            cmd_insert(app, "intersection", None, None).map(|_| {
                app.set_info("Applied intersection to selected nodes");
                app.update_navigation_status();
            })
        },
        "Apply intersection operation to selected nodes",
        0,
        Some(0),
        "intersection",
        vec!["intersection"],
        CommandType::NoArg,
        true,
        true,
    ));

    // Insert command with multi-stage parameter handling
    registry.register(CommandDef::new(
        "insert",
        vec!["i"],
        |app, args| {
            if args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "Usage: insert <module_name> [params]".to_string(),
                ));
            }

            let module_name = args[0];

            // Get module definition to check parameters and children requirements
            let module_def = app.library.get_module(module_name);
            if module_def.is_none() {
                return Err(CommandError::InvalidCommand(format!(
                    "Unknown module: {}",
                    module_name
                )));
            }
            let module_def = module_def.unwrap();

            // Check if this module accepts children
            if module_def.accepts_children && app.selected_nodes.is_empty() {
                return Err(CommandError::InvalidCommand(format!(
                    "'{}' requires child modules. Select modules with 'v' first",
                    module_name
                )));
            }

            // Determine if module has parameters
            let module_has_params = !module_def.parameters.is_empty();

            // Get parameters if provided
            let params = if args.len() > 1 {
                Some(args[1..].join(" "))
            } else {
                None
            };

            // If params not provided and module has parameters, ask for them in next stage
            if params.is_none() && module_has_params {
                begin_pending_module_action(app, PendingModuleAction::Insert, module_name);
                return Ok(());
            }

            // If no params provided and module has no parameters, use empty params
            let final_params = params.or_else(|| Some(String::new()));

            app.push_undo();
            cmd_insert(app, module_name, None, final_params.as_deref()).map(|_| {
                app.update_navigation_status();
                app.set_info(&format!("Inserted: {}", module_name));
            })
        },
        "Insert a module into the AST",
        1,
        None, // Variable number of parameters (optional)
        "insert <module_name> [params]",
        vec!["insert cube", "i sphere", "insert translate 10,0,0"],
        CommandType::Module,
        true,
        true,
    ));

    // Function definition command
    registry.register(CommandDef::new(
        "function",
        vec![] as Vec<String>,
        |app, args| {
            if args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "Usage: function name(params) = expression".to_string(),
                ));
            }

            // Join all arguments to form the complete function definition
            let full_command = args.join(" ");

            app.push_undo();
            cmd_funcdef(app, &full_command)
        },
        "Define or redefine a function",
        1,
        None, // Variable number of parameters (optional)
        "function name(params) = expression",
        vec!["function myfunc()", "function add(a,b) = a + b"],
        CommandType::FunctionDefinition,
        true,
        true,
    ));

    // Module definition command
    registry.register(CommandDef::new(
        "module",
        vec![] as Vec<String>,
        |app, args| {
            if args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "Usage: module <module_name> [params]".to_string(),
                ));
            }
            let module_name = args[0];
            let params = if args.len() > 1 {
                Some(args[1..].join(" "))
            } else {
                None
            };
            app.push_undo();
            cmd_moddef(app, module_name, params.as_deref()).map(|_| {
                app.update_navigation_status();
                app.set_info(&format!("Module '{}' defined", module_name));
            })
        },
        "Define or redefine a module",
        1,
        None,
        "module <module_name> [params]",
        vec!["module mymodule", "module mybox size=10, center=false"],
        CommandType::ModuleDefinition,
        true,
        true,
    ));

    // Global variable command
    registry.register(CommandDef::new(
        "global",
        vec![] as Vec<String>,
        |app, args| {
            if args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "Usage: global <name>=<value>".to_string(),
                ));
            }
            let var_spec = args[0];
            // If there are more args, join them with space (though spec should be single token)
            let full_spec = if args.len() > 1 {
                args.join(" ")
            } else {
                var_spec.to_string()
            };
            app.push_undo();
            cmd_global(app, &full_spec).map(|_| {
                app.update_navigation_status();
                app.set_info(&format!(
                    "Global variable '{}' defined",
                    full_spec.split('=').next().unwrap_or("<invalid>")
                ));
            })
        },
        "Define or redefine a global variable",
        1,
        None,
        "global <name>=<value>",
        vec!["global pi=3.14159", "global name=\"test\""],
        CommandType::GlobalDefinition,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "yank",
        vec!["y"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "yank command takes no arguments".to_string(),
                ));
            }
            cmd_yank(app, None)
        },
        "Copy the last selected module subtree, or the current subtree when none is selected",
        0,
        Some(0),
        "yank",
        vec!["yank", "y"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "paste",
        vec!["p"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "paste command takes no arguments".to_string(),
                ));
            }
            app.push_undo();
            cmd_paste(app).map(|_| ())
        },
        "Paste a copied subtree after the current module node",
        0,
        Some(0),
        "paste",
        vec!["paste", "p"],
        CommandType::NoArg,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "remove",
        vec!["x"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "remove command takes no arguments".to_string(),
                ));
            }
            app.push_undo();
            cmd_remove(app, None)
        },
        "Remove selected module nodes and promote their children, or use the current node",
        0,
        Some(0),
        "remove",
        vec!["remove", "x"],
        CommandType::NoArg,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "set",
        vec!["param"],
        |app, args| {
            if args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "Usage: set <parameter_name>=<expression>".to_string(),
                ));
            }
            let parameter_spec = args.join(" ");
            // Parsing and target validation happen before the single undo point.
            let snapshot = app.ast.clone();
            cmd_set_parameter(app, &parameter_spec)?;
            if app.undo_stack.len() >= 100 {
                app.undo_stack.pop_front();
            }
            app.undo_stack.push_back(snapshot);
            app.redo_stack.clear();
            Ok(())
        },
        "Set a parameter on selected module nodes, or the current node",
        1,
        None,
        "set <parameter_name>=<expression>",
        vec!["set size=size", "set center=true", "set v=offset"],
        CommandType::NodeParam,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "unset",
        vec![] as Vec<String>,
        |app, args| {
            if args.len() != 1 {
                return Err(CommandError::InvalidCommand(
                    "Usage: unset <parameter_name>".to_string(),
                ));
            }
            let snapshot = app.ast.clone();
            cmd_unset_parameter(app, args[0])?;
            if app.undo_stack.len() >= 100 {
                app.undo_stack.pop_front();
            }
            app.undo_stack.push_back(snapshot);
            app.redo_stack.clear();
            Ok(())
        },
        "Remove an explicitly set parameter from selected module nodes, or the current node",
        1,
        Some(1),
        "unset <parameter_name>",
        vec!["unset size", "unset center"],
        CommandType::NodeParamUnset,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "replace",
        vec![] as Vec<String>,
        |app, args| {
            if args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "Usage: replace <module_name> [params]".to_string(),
                ));
            }
            let module_name = args[0];
            let module_def = app.library.get_module(module_name).ok_or_else(|| {
                CommandError::InvalidCommand(format!("Unknown module: {}", module_name))
            })?;
            let params = if args.len() > 1 {
                Some(args[1..].join(" "))
            } else {
                None
            };

            if params.is_none() && !module_def.parameters.is_empty() {
                let target_ids = selected_or_current_node_ids(app)?;
                if target_ids.iter().any(|target_id| {
                    target_id.starts_with("__") || find_module_node(app, target_id).is_none()
                }) {
                    return Err(CommandError::InvalidCommand(
                        "Select a module node before replacing".to_string(),
                    ));
                }
                begin_pending_module_action(
                    app,
                    PendingModuleAction::Replace { target_ids },
                    module_name,
                );
                Ok(())
            } else {
                app.push_undo();
                cmd_replace(app, None, module_name, params.as_deref()).map(|_| ())
            }
        },
        "Replace selected module subtrees, or the current subtree when none is selected",
        1,
        None,
        "replace <module_name> [params]",
        vec!["replace sphere r=5", "replace cube size=[10,10,10]"],
        CommandType::Replace,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "render",
        Vec::<&str>::new(),
        |app, args| {
            if args.is_empty() {
                cmd_render(app)
            } else {
                Err(CommandError::InvalidCommand(
                    "render command takes no arguments".to_string(),
                ))
            }
        },
        "Generate and display the current model with OpenSCAD",
        0,
        Some(0),
        "render",
        vec!["render"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "preview",
        Vec::<&str>::new(),
        |app, args| cmd_preview(app, args[0]),
        "Show source or model preview; model renders once when no preview exists",
        1,
        Some(1),
        "preview source|model|toggle",
        vec!["preview source", "preview model", "preview toggle"],
        CommandType::Preview,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "camera",
        Vec::<&str>::new(),
        cmd_camera,
        "Change model preview projection and camera",
        1,
        Some(3),
        "camera <projection|view|orbit|pan|zoom|fit|auto-rotate> ...",
        vec![
            "camera view iso",
            "camera orbit 10 -5",
            "camera zoom 0.8",
            "camera auto-rotate toggle",
        ],
        CommandType::Camera,
        false,
        true,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use App;

    #[test]
    fn test_cmd_help_generates_current_command_overview() {
        let mut app = App::new();

        cmd_help(&mut app, None).expect("general help should succeed");

        assert_eq!(app.input_mode, InputMode::Help);
        assert_eq!(app.help_doc_count, app.help_doc.len());
        assert!(app
            .help_doc
            .iter()
            .any(|line| line.contains("function name(params) = expression")));
        assert!(app
            .help_doc
            .iter()
            .any(|line| line.contains("replace <module_name> [params]")));
    }

    #[test]
    fn test_source_preview_requests_terminal_graphics_clear() {
        let mut app = App::new();
        app.enter_model_screen();
        app.model_preview.set_auto_rotate(true);

        cmd_preview(&mut app, "source").unwrap();

        assert_eq!(app.screen, crate::app::Screen::Editor);
        assert!(!app.model_preview.auto_rotate);
        assert!(app.take_terminal_clear_request());
        assert!(!app.take_terminal_clear_request());
    }

    #[test]
    fn test_model_preview_enters_independent_screen() {
        let mut app = App::new();

        cmd_preview(&mut app, "model").unwrap();

        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
        assert!(matches!(
            app.model_preview.status,
            crate::preview::ModelPreviewStatus::Generating
        ));

        app.input_mode = InputMode::ModuleEnterParams;
        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
    }

    #[test]
    fn test_preview_model_reuses_an_existing_render() {
        let mut app = App::new();
        app.model_preview.status = crate::preview::ModelPreviewStatus::Ready { triangles: 12 };

        cmd_preview(&mut app, "model").unwrap();

        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
        assert_eq!(
            app.model_preview.status,
            crate::preview::ModelPreviewStatus::Ready { triangles: 12 }
        );
    }

    #[test]
    fn test_preview_toggle_switches_existing_preview_without_rendering() {
        let mut app = App::new();
        app.model_preview.status = crate::preview::ModelPreviewStatus::Ready { triangles: 12 };

        cmd_preview(&mut app, "toggle").unwrap();
        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
        cmd_preview(&mut app, "toggle").unwrap();
        assert_eq!(app.screen, crate::app::Screen::Editor);
    }

    #[test]
    fn test_camera_auto_rotate_toggle_is_a_command_operation() {
        let mut app = App::new();

        cmd_camera(&mut app, &["auto-rotate", "toggle"]).unwrap();
        assert!(app.model_preview.auto_rotate);
        cmd_camera(&mut app, &["auto-rotate", "toggle"]).unwrap();
        assert!(!app.model_preview.auto_rotate);
    }

    #[test]
    fn test_cmd_help_shows_details_for_alias() {
        let mut app = App::new();

        cmd_help(&mut app, Some("w")).expect("alias help should succeed");

        assert!(app.help_doc.iter().any(|line| line == "Help: write"));
        assert!(app
            .help_doc
            .iter()
            .any(|line| line == "Usage: write [filename]"));
        assert!(app
            .help_doc
            .iter()
            .any(|line| line.contains("Aliases: save, w")));
    }

    #[test]
    fn test_cmd_help_rejects_unknown_command() {
        let mut app = App::new();
        let result = cmd_help(&mut app, Some("missing-command"));

        assert!(result.is_err());
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    #[test]
    fn test_cmd_yank_and_paste_clone_subtree_with_new_ids() {
        let mut app = App::new();
        let mut original = ModuleNode::new_container(
            "translate_original".to_string(),
            "translate".to_string(),
            Vec::new(),
        );
        original.children.push(ModuleNode::new_leaf(
            "cube_original".to_string(),
            "cube".to_string(),
            Vec::new(),
        ));
        app.ast_mut().modules.push(original);
        app.tree_state.borrow_mut().select(vec![
            "__modules".to_string(),
            "translate_original".to_string(),
        ]);

        cmd_yank(&mut app, None).expect("yank should succeed");
        let pasted_id = cmd_paste(&mut app).expect("paste should succeed");

        assert_eq!(app.ast.modules.len(), 2);
        let pasted = app.ast.find_node_by_id(&pasted_id).unwrap();
        assert_eq!(pasted.name, "translate");
        assert_ne!(pasted.id, "translate_original");
        assert_ne!(pasted.children[0].id, "cube_original");
    }

    #[test]
    fn test_cmd_remove_does_not_change_clipboard() {
        let mut app = App::new();
        app.ast_mut().modules.push(ModuleNode::new_leaf(
            "cube_1".to_string(),
            "cube".to_string(),
            Vec::new(),
        ));
        cmd_yank(&mut app, Some("cube_1")).unwrap();

        cmd_remove(&mut app, Some("cube_1")).expect("remove should succeed");

        assert!(app.ast.find_node_by_id("cube_1").is_none());
        assert_eq!(app.node_clipboard.as_ref().unwrap().id, "cube_1");
    }

    #[test]
    fn test_cmd_delete_prefers_selected_nodes() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf("cube_1".to_string(), "cube".to_string(), Vec::new()),
            ModuleNode::new_leaf("sphere_1".to_string(), "sphere".to_string(), Vec::new()),
            ModuleNode::new_leaf("keep_1".to_string(), "cube".to_string(), Vec::new()),
        ];
        app.selected_nodes = vec!["cube_1".to_string(), "sphere_1".to_string()];
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "keep_1".to_string()]);

        cmd_delete(&mut app, "").expect("delete should succeed");

        assert_eq!(app.ast.modules.len(), 1);
        assert_eq!(app.ast.modules[0].id, "keep_1");
        assert!(app.selected_nodes.is_empty());
    }

    #[test]
    fn test_cmd_set_parameter_updates_selected_nodes_atomically() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf(
                "cube_1".to_string(),
                "cube".to_string(),
                vec![Argument::Named {
                    name: "size".to_string(),
                    value: Expr::Integer(10),
                }],
            ),
            ModuleNode::new_leaf(
                "sphere_1".to_string(),
                "sphere".to_string(),
                vec![Argument::Named {
                    name: "r".to_string(),
                    value: Expr::Integer(5),
                }],
            ),
        ];
        app.selected_nodes = vec!["cube_1".to_string(), "sphere_1".to_string()];

        let result = cmd_set_parameter(&mut app, "size=module_size");

        assert!(result.is_err());
        assert!(matches!(
            &app.ast.modules[0].args[0],
            Argument::Named {
                value: Expr::Integer(10),
                ..
            }
        ));
    }

    #[test]
    fn test_cmd_unset_parameter_supports_positional_named_and_atomic_failure() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf(
                "cube_1".to_string(),
                "cube".to_string(),
                vec![Argument::Positional(Expr::Integer(10))],
            ),
            ModuleNode::new_leaf(
                "cube_2".to_string(),
                "cube".to_string(),
                vec![Argument::Named {
                    name: "size".to_string(),
                    value: Expr::Integer(20),
                }],
            ),
        ];
        app.selected_nodes = vec!["cube_1".to_string(), "cube_2".to_string()];

        cmd_unset_parameter(&mut app, "size").unwrap();
        assert!(app.ast.modules.iter().all(|node| node.args.is_empty()));

        app.ast_mut().modules[0].args.push(Argument::Named {
            name: "center".to_string(),
            value: Expr::Boolean(true),
        });
        let result = cmd_unset_parameter(&mut app, "center");
        assert!(result.is_err());
        assert_eq!(app.ast.modules[0].args.len(), 1);
    }

    #[test]
    fn test_cmd_set_parameter_supports_special_parameters_and_values() {
        let mut app = App::new();
        let sphere_id = cmd_insert(&mut app, "sphere", None, Some("r=10")).unwrap();
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), sphere_id.clone()]);

        cmd_set_parameter(&mut app, "$fn=32").unwrap();
        cmd_set_parameter(&mut app, "r=$fn").unwrap();

        let sphere = app.ast.find_node_by_id(&sphere_id).unwrap();
        assert!(sphere.args.iter().any(|argument| matches!(
            argument,
            Argument::Named {
                name,
                value: Expr::Integer(32)
            } if name == "$fn"
        )));
        assert!(sphere.args.iter().any(|argument| matches!(
            argument,
            Argument::Named {
                name,
                value: Expr::Identifier(identifier)
            } if name == "r" && identifier == "$fn"
        )));
    }

    #[test]
    fn test_module_body_parameter_can_reference_module_parameter() {
        let mut app = App::new();
        let cube_id = cmd_insert(&mut app, "cube", None, Some("size=10")).unwrap();
        app.selected_nodes = vec![cube_id];
        cmd_moddef(&mut app, "my_box", Some("size=20")).unwrap();
        let body_id = app.ast.module_defines[0].body[0].id.clone();
        app.tree_state.borrow_mut().select(vec![
            "__moddefs".to_string(),
            "__moddef_my_box".to_string(),
            body_id,
        ]);

        cmd_set_parameter(&mut app, "size=size").unwrap();

        assert!(app.ast.module_defines[0]
            .to_scad()
            .contains("cube(size=size);"));
    }

    #[test]
    fn test_cmd_yank_prefers_selected_node_over_current_node() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf("selected_1".to_string(), "cube".to_string(), Vec::new()),
            ModuleNode::new_leaf("current_1".to_string(), "sphere".to_string(), Vec::new()),
        ];
        app.selected_nodes = vec!["selected_1".to_string()];
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "current_1".to_string()]);

        cmd_yank(&mut app, None).expect("yank should succeed");

        assert_eq!(app.node_clipboard.as_ref().unwrap().id, "selected_1");
    }

    #[test]
    fn test_cmd_remove_prefers_all_selected_nodes_over_current_node() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf("selected_1".to_string(), "cube".to_string(), Vec::new()),
            ModuleNode::new_leaf("selected_2".to_string(), "sphere".to_string(), Vec::new()),
            ModuleNode::new_leaf("current_1".to_string(), "cube".to_string(), Vec::new()),
        ];
        app.selected_nodes = vec!["selected_1".to_string(), "selected_2".to_string()];
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "current_1".to_string()]);

        cmd_remove(&mut app, None).expect("remove should succeed");

        assert_eq!(app.ast.modules.len(), 1);
        assert_eq!(app.ast.modules[0].id, "current_1");
    }

    #[test]
    fn test_cmd_replace_prefers_all_selected_nodes_over_current_node() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf("selected_1".to_string(), "cube".to_string(), Vec::new()),
            ModuleNode::new_leaf("selected_2".to_string(), "sphere".to_string(), Vec::new()),
            ModuleNode::new_leaf("current_1".to_string(), "cube".to_string(), Vec::new()),
        ];
        app.selected_nodes = vec!["selected_1".to_string(), "selected_2".to_string()];
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "current_1".to_string()]);

        cmd_replace(&mut app, None, "cylinder", None).expect("replace should succeed");

        assert_eq!(app.ast.modules.len(), 3);
        assert_eq!(app.ast.modules[0].name, "cylinder");
        assert_eq!(app.ast.modules[1].name, "cylinder");
        assert_eq!(app.ast.modules[2].id, "current_1");
    }

    #[test]
    fn test_cmd_remove_node_from_module_definition() {
        use openscad_core::ModuleDefinition;

        let mut app = App::new();
        app.ast_mut().module_defines.push(ModuleDefinition::new(
            "custom".to_string(),
            Vec::new(),
            vec![ModuleNode::new_leaf(
                "body_cube".to_string(),
                "cube".to_string(),
                Vec::new(),
            )],
        ));

        cmd_remove(&mut app, Some("body_cube")).expect("remove should succeed");

        assert!(app.ast.module_defines[0].body.is_empty());
    }

    #[test]
    fn test_cmd_remove_promotes_children_at_same_position() {
        let mut app = App::new();
        let mut container =
            ModuleNode::new_container("group_1".to_string(), "union".to_string(), Vec::new());
        container.children = vec![
            ModuleNode::new_leaf("cube_1".to_string(), "cube".to_string(), Vec::new()),
            ModuleNode::new_leaf("sphere_1".to_string(), "sphere".to_string(), Vec::new()),
        ];
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf("before".to_string(), "cube".to_string(), Vec::new()),
            container,
            ModuleNode::new_leaf("after".to_string(), "cube".to_string(), Vec::new()),
        ];

        cmd_remove(&mut app, Some("group_1")).expect("remove should succeed");

        let ids: Vec<&str> = app
            .ast
            .modules
            .iter()
            .map(|node| node.id.as_str())
            .collect();
        assert_eq!(ids, vec!["before", "cube_1", "sphere_1", "after"]);
    }

    #[test]
    fn test_cmd_replace_uses_current_node_and_inserts_new_node() {
        let mut app = App::new();
        app.ast_mut().modules.push(ModuleNode::new_leaf(
            "shape_1".to_string(),
            "cube".to_string(),
            vec![Argument::Positional(Expr::Integer(10))],
        ));
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "shape_1".to_string()]);

        let replacement_id =
            cmd_replace(&mut app, None, "sphere", None).expect("replace should succeed");

        assert!(app.ast.find_node_by_id("shape_1").is_none());
        let replaced = app.ast.find_node_by_id(&replacement_id).unwrap();
        assert_eq!(replaced.name, "sphere");
        assert!(replaced.args.is_empty());
    }

    #[test]
    fn test_cmd_replace_deletes_source_subtree() {
        let mut app = App::new();
        let mut container =
            ModuleNode::new_container("group_1".to_string(), "union".to_string(), Vec::new());
        container.children.push(ModuleNode::new_leaf(
            "child_1".to_string(),
            "cube".to_string(),
            Vec::new(),
        ));
        app.ast_mut().modules.push(container);

        let replacement_id = cmd_replace(&mut app, Some("group_1"), "sphere", None).unwrap();

        assert!(app.ast.find_node_by_id("group_1").is_none());
        assert!(app.ast.find_node_by_id("child_1").is_none());
        assert!(app.ast.find_node_by_id(&replacement_id).is_some());
    }

    #[test]
    fn test_replace_command_enters_parameter_stage_and_applies_parameters() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = App::new();
        app.ast_mut().modules.push(ModuleNode::new_leaf(
            "shape_1".to_string(),
            "sphere".to_string(),
            Vec::new(),
        ));
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "shape_1".to_string()]);
        let handler = app.command_registry.find("replace").unwrap().handler;

        handler(&mut app, &["cube"]).expect("replace should enter parameter stage");
        assert_eq!(app.input_mode, InputMode::ModuleEnterParams);
        assert_eq!(
            app.pending_module_action,
            Some(PendingModuleAction::Replace {
                target_ids: vec!["shape_1".to_string()]
            })
        );
        assert_eq!(app.pending_module_name.as_deref(), Some("cube"));
        assert!(app.ast.find_node_by_id("shape_1").is_some());

        app.input_buffer.set_content("size=5");
        crate::input::handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);

        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.ast.find_node_by_id("shape_1").is_none());
        let replacement = app.ast.modules.first().unwrap();
        assert_eq!(replacement.name, "cube");
        assert!(matches!(
            replacement.args.first(),
            Some(Argument::Named { name, value: Expr::Integer(5) }) if name == "size"
        ));
    }

    #[test]
    fn test_parse_arguments() {
        let mgr = openscad_library::LibraryManager::new();
        let cube_def = mgr.get_module("cube").unwrap();
        // Test with simpler input that matches expected format
        let args = parse_arguments("10,10,10", &cube_def);
        // Either ok or err is fine - this is just testing the function exists
        let _ = args;
    }

    #[test]
    fn test_cmd_moddef_basic() {
        use App;

        let mut app = App::new();

        // Create a module definition without parameters
        let result = cmd_moddef(&mut app, "my_module", None);
        assert!(result.is_ok(), "cmd_moddef should succeed");

        // Check that module was added to AST
        assert_eq!(app.ast.module_defines.len(), 1);
        let module_def = &app.ast.module_defines[0];
        assert_eq!(module_def.name, "my_module");
        assert!(module_def.parameters.is_empty());
        assert!(module_def.body.is_empty());

        // Check that module was added to library manager
        let module = app.library.get_module("my_module");
        assert!(module.is_some());
        let module = module.unwrap();
        assert_eq!(module.name, "my_module");
        assert!(!module.accepts_children);
    }

    #[test]
    fn test_cmd_moddef_with_params() {
        use App;

        let mut app = App::new();

        // Create a module definition with parameters
        let result = cmd_moddef(&mut app, "my_box", Some("size=10, center=false"));
        assert!(result.is_ok(), "cmd_moddef should succeed");

        // Check that module was added to AST
        assert_eq!(app.ast.module_defines.len(), 1);
        let module_def = &app.ast.module_defines[0];
        assert_eq!(module_def.name, "my_box");
        assert_eq!(module_def.parameters.len(), 2);
        assert!(module_def.body.is_empty());

        // Check parameters
        let param1 = &module_def.parameters[0];
        assert_eq!(param1.name, "size");
        assert!(param1.default.is_some());
        let param2 = &module_def.parameters[1];
        assert_eq!(param2.name, "center");
        assert!(param2.default.is_some());

        // Check library module
        let module = app.library.get_module("my_box");
        assert!(module.is_some());
        let module = module.unwrap();
        assert_eq!(module.name, "my_box");
        assert_eq!(module.parameters.len(), 2);
        assert!(!module.accepts_children);
    }

    #[test]
    fn test_cmd_moddef_redefines_existing_module() {
        use App;

        let mut app = App::new();

        // First module definition should succeed
        let result = cmd_moddef(&mut app, "my_module", Some("size=10"));
        assert!(result.is_ok());

        // A second definition replaces the first one in place.
        let result = cmd_moddef(&mut app, "my_module", Some("height=20"));
        assert!(result.is_ok());

        // Verify the AST and completion library both contain the replacement.
        assert_eq!(app.ast.module_defines.len(), 1);
        assert_eq!(app.ast.module_defines[0].parameters[0].name, "height");
        assert_eq!(
            app.library.get_module("my_module").unwrap().parameters[0].name,
            "height"
        );
    }

    #[test]
    fn test_cmd_moddef_complex_parameters() {
        use App;

        let mut app = App::new();

        // Test complex parameter expressions
        let result = cmd_moddef(
            &mut app,
            "complex",
            Some("size=10, offset=5, name=\"test\""),
        );
        result.unwrap();

        // Check parameters were parsed
        assert_eq!(app.ast.module_defines.len(), 1);
        let module_def = &app.ast.module_defines[0];
        assert_eq!(module_def.parameters.len(), 3);

        // Check library module
        let module = app.library.get_module("complex");
        assert!(module.is_some());
        let module = module.unwrap();
        assert_eq!(module.parameters.len(), 3);
    }

    #[test]
    fn test_serialize_deserialize_with_custom_module() {
        use openscad_core::AstRoot;
        use App;

        let mut app = App::new();

        // Create a custom module
        cmd_moddef(&mut app, "my_cube", Some("size=10")).unwrap();

        // Add a module instance to the modules section
        // This tests that insert works with custom modules
        let result = cmd_insert(&mut app, "my_cube", None, Some("size=15"));
        assert!(result.is_ok(), "insert should work with custom module");

        // Serialize the AST to JSON
        let json = serde_json::to_string_pretty(&*app.ast).expect("Failed to serialize AST");

        // Deserialize back
        let deserialized: AstRoot = serde_json::from_str(&json).expect("Failed to deserialize AST");

        // Verify module definitions
        assert_eq!(deserialized.module_defines.len(), 1);
        assert_eq!(deserialized.module_defines[0].name, "my_cube");
        assert_eq!(deserialized.module_defines[0].parameters.len(), 1);

        // Verify module instances (should be empty because modules are not in module_defines)
        // Actually, modules field contains module instances, not definitions
        // The inserted module should be in modules field
        assert!(!deserialized.modules.is_empty());

        // Check that custom modules are reloaded in library manager
        // (This would be tested in integration, but we can at least ensure no panic)
    }

    #[test]
    fn test_serialize_deserialize_with_custom_module_and_children() {
        use openscad_core::{Argument, AstRoot, Expr, ModuleNode};
        use App;

        let mut app = App::new();

        // Create a custom module with parameters
        cmd_moddef(&mut app, "my_container", Some("scale=2")).unwrap();

        // Add a child module to the custom module definition body
        let child_node = ModuleNode::new_leaf(
            "cube_1".to_string(),
            "cube".to_string(),
            vec![Argument::Named {
                name: "size".to_string(),
                value: Expr::Integer(5),
            }],
        );

        // Add child to the first module definition's body
        app.ast_mut().module_defines[0].body.push(child_node);

        // Serialize the AST to JSON
        let json = serde_json::to_string_pretty(&*app.ast).expect("Failed to serialize AST");

        // Deserialize back
        let deserialized: AstRoot = serde_json::from_str(&json).expect("Failed to deserialize AST");

        // Verify module definitions
        assert_eq!(deserialized.module_defines.len(), 1);
        assert_eq!(deserialized.module_defines[0].name, "my_container");
        assert_eq!(deserialized.module_defines[0].parameters.len(), 1);

        // Verify child node in module definition body
        assert_eq!(deserialized.module_defines[0].body.len(), 1);
        assert_eq!(deserialized.module_defines[0].body[0].name, "cube");

        // Verify library manager can reload custom modules
        // (This is done in cmd_load, but we test that serialization works)
    }

    #[test]
    fn test_cmd_insert_into_modules_section() {
        use App;

        let mut app = App::new();

        // Simulate selecting the __modules section
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string()]);

        // Insert a module (cube) into the Modules section
        let result = cmd_insert(&mut app, "cube", None, Some("10,10,10"));
        assert!(result.is_ok(), "insert should succeed");

        // Check that module was added to ast.modules
        assert_eq!(app.ast.modules.len(), 1);
        let inserted = &app.ast.modules[0];
        assert_eq!(inserted.name, "cube");

        // Check that tree state is updated to select the new module
        let selected = app.tree_state.borrow().selected().last().cloned();
        assert!(selected.is_some());
        // The selected ID should be the inserted module's ID
        assert_eq!(selected.unwrap(), inserted.id);
    }

    #[test]
    fn test_cmd_insert_into_module_definition() {
        use App;

        let mut app = App::new();

        // Create a custom module definition
        cmd_moddef(&mut app, "my_module", None).unwrap();

        // Simulate selecting the module definition itself (__moddef_my_module)
        app.tree_state.borrow_mut().select(vec![
            "__moddefs".to_string(),
            "__moddef_my_module".to_string(),
        ]);

        // Insert a cube into the module definition body
        let result = cmd_insert(&mut app, "cube", None, Some("5,5,5"));
        assert!(result.is_ok(), "insert should succeed");

        // Check that module was added to module definition body, not ast.modules
        assert_eq!(app.ast.modules.len(), 0); // No module instances
        assert_eq!(app.ast.module_defines.len(), 1);
        assert_eq!(app.ast.module_defines[0].body.len(), 1);
        let inserted = &app.ast.module_defines[0].body[0];
        assert_eq!(inserted.name, "cube");

        // Check tree state selection
        let selected = app.tree_state.borrow().selected().last().cloned();
        assert!(selected.is_some());
        assert_eq!(selected.unwrap(), inserted.id);
    }

    #[test]
    fn test_cmd_boolean_op_in_modules_section() {
        use App;

        let mut app = App::new();

        // Insert two cubes into modules section
        let cube1_id = cmd_insert(&mut app, "cube", None, Some("5,5,5")).unwrap();
        let cube2_id = cmd_insert(&mut app, "cube", None, Some("10,10,10")).unwrap();

        // Select both nodes
        let selected_nodes = vec![cube1_id.clone(), cube2_id.clone()];
        app.selected_nodes = selected_nodes.clone();

        // Perform union operation
        let result = cmd_boolean_op(&mut app, "union", &selected_nodes);
        assert!(result.is_ok(), "boolean operation should succeed");

        let container_id = result.unwrap();

        // Check that container was created in modules section
        assert!(app.ast.find_node_by_id(&container_id).is_some());
        let container = app.ast.find_node_by_id(&container_id).unwrap();
        assert_eq!(container.name, "union");

        // Check that container has two children
        assert_eq!(container.children.len(), 2);

        // Check that original nodes are now children of the container
        assert_eq!(container.children.len(), 2);
        let child_ids: Vec<String> = container.children.iter().map(|c| c.id.clone()).collect();
        assert!(child_ids.contains(&cube1_id));
        assert!(child_ids.contains(&cube2_id));
        // Check that nodes are not at root level of modules section
        assert!(!app.ast.modules.iter().any(|m| m.id == cube1_id));
        assert!(!app.ast.modules.iter().any(|m| m.id == cube2_id));

        // Check that container is selected
        assert_eq!(
            app.tree_state.borrow().selected().last(),
            Some(&container_id)
        );
    }

    #[test]
    fn test_cmd_boolean_op_in_module_definition() {
        use App;

        let mut app = App::new();

        // Create a custom module definition
        cmd_moddef(&mut app, "my_module", None).unwrap();

        // Add two cubes to module definition body
        let cube1 = openscad_core::ModuleNode::new_leaf(
            "cube_1".to_string(),
            "cube".to_string(),
            vec![openscad_core::Argument::Named {
                name: "size".to_string(),
                value: openscad_core::Expr::Integer(5),
            }],
        );
        let cube2 = openscad_core::ModuleNode::new_leaf(
            "cube_2".to_string(),
            "cube".to_string(),
            vec![openscad_core::Argument::Named {
                name: "size".to_string(),
                value: openscad_core::Expr::Integer(10),
            }],
        );

        // Add cubes to module definition body
        app.ast_mut().module_defines[0].body.push(cube1.clone());
        app.ast_mut().module_defines[0].body.push(cube2.clone());

        // Select both nodes (they are in module definition body)
        let selected_nodes = vec![cube1.id.clone(), cube2.id.clone()];
        app.selected_nodes = selected_nodes.clone();

        // Perform difference operation
        let result = cmd_boolean_op(&mut app, "difference", &selected_nodes);
        assert!(
            result.is_ok(),
            "boolean operation should succeed in module definition"
        );

        let container_id = result.unwrap();

        // Check that container was created in module definition body, not modules section
        assert!(app.ast.find_node_by_id(&container_id).is_none()); // Not in modules section

        // Check that container exists in module definition body
        let mod_def = &app.ast.module_defines[0];
        let container = find_node_in_module_definition(&mod_def.body, &container_id);
        assert!(container.is_some());
        let container = container.unwrap();
        assert_eq!(container.name, "difference");

        // Check that container has two children with the original nodes
        assert_eq!(container.children.len(), 2);
        let child_ids: Vec<String> = container.children.iter().map(|c| c.id.clone()).collect();
        assert!(child_ids.contains(&cube1.id));
        assert!(child_ids.contains(&cube2.id));
        // Check that nodes are not at root level of module definition body
        assert!(!mod_def.body.iter().any(|m| m.id == cube1.id));
        assert!(!mod_def.body.iter().any(|m| m.id == cube2.id));

        // Check that container is selected
        assert_eq!(
            app.tree_state.borrow().selected().last(),
            Some(&container_id)
        );
    }

    #[test]
    fn test_cmd_boolean_op_mixed_context_error() {
        use App;

        let mut app = App::new();

        // Create a custom module definition
        cmd_moddef(&mut app, "my_module", None).unwrap();
        // Clear selection to ensure next insert goes to modules section
        app.tree_state.borrow_mut().select(Vec::new());

        // Add a cube to module definition body
        let cube1 = openscad_core::ModuleNode::new_leaf(
            "cube_1".to_string(),
            "cube".to_string(),
            vec![openscad_core::Argument::Named {
                name: "size".to_string(),
                value: openscad_core::Expr::Integer(5),
            }],
        );
        app.ast_mut().module_defines[0].body.push(cube1.clone());

        // Insert a cube into modules section
        let cube2_id = cmd_insert(&mut app, "cube", None, Some("10,10,10")).unwrap();

        // Select nodes from both contexts (mixed)
        let selected_nodes = vec![cube1.id.clone(), cube2_id.clone()];
        app.selected_nodes = selected_nodes.clone();

        // Perform union operation - should fail with mixed context error
        let result = cmd_boolean_op(&mut app, "union", &selected_nodes);
        assert!(
            result.is_err(),
            "boolean operation should fail with mixed context"
        );

        // Verify error message indicates mixed context
        let err = result.unwrap_err();
        assert!(matches!(err, CommandError::InvalidCommand(_)));
        let err_msg = match err {
            CommandError::InvalidCommand(msg) => msg,
            _ => panic!("Unexpected error type"),
        };
        assert!(err_msg.contains("mixed contexts") || err_msg.contains("different contexts"));
    }

    #[test]
    fn test_cmd_insert_container_in_module_definition() {
        use App;

        let mut app = App::new();

        // Create a custom module definition
        cmd_moddef(&mut app, "my_module", None).unwrap();

        // Add two cubes to module definition body
        let cube1 = openscad_core::ModuleNode::new_leaf(
            "cube_1".to_string(),
            "cube".to_string(),
            vec![openscad_core::Argument::Named {
                name: "size".to_string(),
                value: openscad_core::Expr::Integer(5),
            }],
        );
        let cube2 = openscad_core::ModuleNode::new_leaf(
            "cube_2".to_string(),
            "cube".to_string(),
            vec![openscad_core::Argument::Named {
                name: "size".to_string(),
                value: openscad_core::Expr::Integer(10),
            }],
        );

        // Add cubes to module definition body
        app.ast_mut().module_defines[0].body.push(cube1.clone());
        app.ast_mut().module_defines[0].body.push(cube2.clone());

        // Select both nodes (they are in module definition body)
        app.selected_nodes = vec![cube1.id.clone(), cube2.id.clone()];

        // Insert a difference container module
        let result = cmd_insert(&mut app, "difference", None, None);
        assert!(
            result.is_ok(),
            "insert difference should succeed in module definition"
        );

        let container_id = result.unwrap();

        // Check that container was created in module definition body, not modules section
        assert!(app.ast.find_node_by_id(&container_id).is_none()); // Not in modules section

        // Check that container exists in module definition body
        let mod_def = &app.ast.module_defines[0];
        let container = find_node_in_module_definition(&mod_def.body, &container_id);
        assert!(container.is_some());
        let container = container.unwrap();
        assert_eq!(container.name, "difference");

        // Check that container has two children with the original nodes
        assert_eq!(container.children.len(), 2);
        let child_ids: Vec<String> = container.children.iter().map(|c| c.id.clone()).collect();
        assert!(child_ids.contains(&cube1.id));
        assert!(child_ids.contains(&cube2.id));

        // Check that nodes are not at root level of module definition body
        assert!(!mod_def.body.iter().any(|m| m.id == cube1.id));
        assert!(!mod_def.body.iter().any(|m| m.id == cube2.id));

        // Check that container is selected (tree state should select it)
        assert_eq!(
            app.tree_state.borrow().selected().last(),
            Some(&container_id)
        );
    }

    #[test]
    fn test_cmd_moddef_with_children_module() {
        use App;

        let mut app = App::new();

        // Create a custom module definition without children module initially
        cmd_moddef(&mut app, "my_module", None).unwrap();

        // Initially, the module should not accept children
        let module = app.library.get_module("my_module").unwrap();
        assert!(!module.accepts_children);

        // Clear selection and navigate to module definition body
        app.tree_state.borrow_mut().select(vec![
            "__moddefs".to_string(),
            "__moddef_my_module".to_string(),
        ]);

        // Insert a children module into the module definition body
        let result = cmd_insert(&mut app, "children", None, None);
        assert!(
            result.is_ok(),
            "children module should be insertable into module definition"
        );

        // After inserting children module, the custom module should accept children
        // Note: The library manager should have been updated via reload_custom_modules_from_ast
        let module = app.library.get_module("my_module").unwrap();
        assert!(
            module.accepts_children,
            "module with children module should accept children"
        );
    }

    #[test]
    fn test_cmd_insert_children_outside_module_definition_fails() {
        use App;

        let mut app = App::new();

        // Try to insert children module outside module definition (in modules section)
        // Ensure no module definition is selected
        app.tree_state.borrow_mut().select(Vec::new());

        let result = cmd_insert(&mut app, "children", None, None);
        assert!(
            result.is_err(),
            "children module should not be insertable outside module definitions"
        );
        let err = result.unwrap_err();
        assert!(matches!(err, CommandError::InvalidCommand(_)));
        let err_msg = match err {
            CommandError::InvalidCommand(msg) => msg,
            _ => panic!("Unexpected error type"),
        };
        assert!(err_msg.contains("children module can only be used inside module definitions"));
    }

    #[test]
    fn test_cmd_global_basic() {
        use App;

        let mut app = App::new();

        // Test basic variable assignment
        let result = cmd_global(&mut app, "width=100");
        assert!(result.is_ok(), "cmd_global should succeed");

        // Check that variable was added to AST
        assert_eq!(app.ast.global_variables().len(), 1);
        let var = &app.ast.global_variables()[0];
        assert_eq!(var.name, "width");
        assert_eq!(var.value, openscad_core::Expr::Integer(100));
    }

    #[test]
    fn test_cmd_global_special() {
        use App;

        let mut app = App::new();

        // Test special variable assignment
        let result = cmd_global(&mut app, "$fn=50");
        assert!(
            result.is_ok(),
            "cmd_global should succeed with special variable"
        );

        // Check that special variable was added to AST
        assert_eq!(app.ast.global_variables().len(), 1);
        let var = &app.ast.global_variables()[0];
        assert_eq!(var.name, "$fn");
        assert_eq!(var.value, openscad_core::Expr::Integer(50));
    }

    #[test]
    fn test_cmd_global_with_list() {
        use App;

        let mut app = App::new();

        // Test variable with list value
        let result = cmd_global(&mut app, "size=[10,20,30]");
        assert!(result.is_ok(), "cmd_global should succeed with list value");

        // Check that variable was added to AST
        assert_eq!(app.ast.global_variables().len(), 1);
        let var = &app.ast.global_variables()[0];
        assert_eq!(var.name, "size");

        // Check that value is a list
        if let openscad_core::Expr::List(items) = &var.value {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], openscad_core::Expr::Integer(10));
            assert_eq!(items[1], openscad_core::Expr::Integer(20));
            assert_eq!(items[2], openscad_core::Expr::Integer(30));
        } else {
            panic!("Expected list expression");
        }
    }

    #[test]
    fn test_cmd_global_invalid_syntax() {
        use App;

        let mut app = App::new();

        // Test invalid syntax (no equals)
        let result = cmd_global(&mut app, "width100");
        assert!(
            result.is_err(),
            "cmd_global should fail with invalid syntax"
        );

        // Check that no variables were added to AST
        assert_eq!(app.ast.global_variables().len(), 0);
    }

    #[test]
    fn test_cmd_global_invalid_identifier() {
        use App;

        let mut app = App::new();

        // Test invalid identifier (starts with number)
        let result = cmd_global(&mut app, "123width=100");
        assert!(
            result.is_err(),
            "cmd_global should fail with invalid identifier"
        );

        // Check that no variables were added to AST
        assert_eq!(app.ast.global_variables().len(), 0);
    }

    #[test]
    fn test_cmd_global_redefines_existing_variable() {
        use App;

        let mut app = App::new();

        // Add first variable
        let result = cmd_global(&mut app, "width=100");
        assert!(result.is_ok(), "First cmd_global should succeed");

        // Redefine in place
        let result = cmd_global(&mut app, "width=200");
        assert!(result.is_ok());

        // Check that only one variable remains and its value was replaced
        assert_eq!(app.ast.global_variables().len(), 1);
        assert_eq!(
            app.ast.global_variables()[0].value,
            openscad_core::Expr::Integer(200)
        );
    }

    #[test]
    fn test_cmd_global_string_value() {
        use App;

        let mut app = App::new();

        // Test variable with string value
        let result = cmd_global(&mut app, "color=\"red\"");
        assert!(
            result.is_ok(),
            "cmd_global should succeed with string value"
        );

        // Check that variable was added to AST
        assert_eq!(app.ast.global_variables().len(), 1);
        let var = &app.ast.global_variables()[0];
        assert_eq!(var.name, "color");
        assert_eq!(var.value, openscad_core::Expr::String("red".to_string()));
    }

    #[test]
    fn test_cmd_global_float_value() {
        use App;

        let mut app = App::new();

        // Test variable with float value
        let result = cmd_global(&mut app, "precision=2.5");
        assert!(result.is_ok(), "cmd_global should succeed with float value");

        // Check that variable was added to AST
        assert_eq!(app.ast.global_variables().len(), 1);
        let var = &app.ast.global_variables()[0];
        assert_eq!(var.name, "precision");

        // Compare floats by converting to string representation
        if let openscad_core::Expr::Float(f) = var.value {
            assert!((f - 2.5).abs() < 0.001);
        } else {
            panic!("Expected float expression");
        }
    }

    #[test]
    fn test_cmd_funcdef_basic() {
        use App;

        let mut app = App::new();

        // Create a function definition with one parameter using new parentheses syntax
        let result = cmd_funcdef(&mut app, "square(x) = 10");
        assert!(result.is_ok(), "cmd_funcdef should succeed");

        // Check that function was added to AST
        assert_eq!(app.ast.function_defines.len(), 1);
        let func_def = &app.ast.function_defines[0];
        assert_eq!(func_def.name, "square");
        assert_eq!(func_def.parameters.len(), 1);
        assert_eq!(func_def.parameters[0].name, "x");
    }

    #[test]
    fn test_cmd_funcdef_no_params() {
        use App;

        let mut app = App::new();

        // Create a function definition without parameters
        let result = cmd_funcdef(&mut app, "pi_value() = 3.14159");
        assert!(result.is_ok(), "cmd_funcdef should succeed");

        // Check that function was added to AST
        assert_eq!(app.ast.function_defines.len(), 1);
        let func_def = &app.ast.function_defines[0];
        assert_eq!(func_def.name, "pi_value");
        assert_eq!(func_def.parameters.len(), 0);
        // Verify the body is the constant value
    }

    #[test]
    fn test_cmd_funcdef_multiple_params() {
        use App;

        let mut app = App::new();

        // Create a function definition with multiple parameters using new parentheses syntax
        let result = cmd_funcdef(&mut app, "add(a, b) = 15");
        assert!(result.is_ok(), "cmd_funcdef should succeed");

        // Check that function was added to AST
        assert_eq!(app.ast.function_defines.len(), 1);
        let func_def = &app.ast.function_defines[0];
        assert_eq!(func_def.name, "add");
        assert_eq!(func_def.parameters.len(), 2);
        assert_eq!(func_def.parameters[0].name, "a");
        assert_eq!(func_def.parameters[1].name, "b");
    }

    #[test]
    fn test_cmd_funcdef_redefines_existing_function() {
        use App;

        let mut app = App::new();

        // First function definition should succeed
        let result = cmd_funcdef(&mut app, "my_func(x) = 10");
        assert!(result.is_ok());

        // Second function definition with the same name replaces it in place
        let result = cmd_funcdef(&mut app, "my_func(y) = 20");
        assert!(result.is_ok());

        // Verify only one, updated function in AST and library completion metadata
        assert_eq!(app.ast.function_defines.len(), 1);
        assert_eq!(app.ast.function_defines[0].parameters[0].name, "y");
        assert_eq!(
            app.ast.function_defines[0].body,
            openscad_core::Expr::Integer(20)
        );
        assert_eq!(
            app.library.get_function("my_func").unwrap().parameters[0].name,
            "y"
        );
    }

    #[test]
    fn test_cmd_delete_global_function_and_module_definitions_without_cascading() {
        let mut app = App::new();
        cmd_global(&mut app, "size=10").unwrap();
        cmd_funcdef(&mut app, "double(x)=x*2").unwrap();
        cmd_funcdef(&mut app, "uses_double(x)=double(x)").unwrap();
        cmd_moddef(&mut app, "part", Some("size=10")).unwrap();

        cmd_delete(&mut app, "__var_size").unwrap();
        assert!(app.ast.find_global_variable("size").is_none());

        cmd_delete(&mut app, "__func_double").unwrap();
        assert!(app.ast.find_function_define("double").is_none());
        assert!(app.ast.find_function_define("uses_double").is_some());
        assert!(app.library.get_function("double").is_none());

        cmd_delete(&mut app, "__moddef_part").unwrap();
        assert!(app.ast.find_module_define("part").is_none());
        assert!(app.library.get_module("part").is_none());
    }

    #[test]
    fn test_cmd_funcdef_invalid_syntax() {
        use App;

        let mut app = App::new();

        // Test invalid expression in function body
        let result = cmd_funcdef(&mut app, "bad_func(x) = x + ");
        assert!(result.is_err());

        // Check that no functions were added to AST
        assert_eq!(app.ast.function_defines.len(), 0);
    }

    #[test]
    fn test_cmd_funcdef_invalid_name() {
        use App;

        let mut app = App::new();

        // Test invalid function name (starts with number)
        let result = cmd_funcdef(&mut app, "123func(x) = x * x");
        assert!(result.is_err());

        // Check that no functions were added to AST
        assert_eq!(app.ast.function_defines.len(), 0);
    }

    #[test]
    fn test_cmd_funcdef_with_binary_operations() {
        use App;

        let mut app = App::new();

        // Create a function with binary operations in the body
        let result = cmd_funcdef(&mut app, "add(a, b) = a + b");
        assert!(
            result.is_ok(),
            "cmd_funcdef should succeed with binary operations"
        );

        // Check that function was added to AST
        assert_eq!(app.ast.function_defines.len(), 1);
        let func_def = &app.ast.function_defines[0];
        assert_eq!(func_def.name, "add");
        assert_eq!(func_def.parameters.len(), 2);
        assert_eq!(func_def.parameters[0].name, "a");
        assert_eq!(func_def.parameters[1].name, "b");

        // Verify the body contains a binary operation
        match &func_def.body {
            openscad_core::Expr::BinOp { op, .. } => {
                assert_eq!(*op, openscad_core::BinOp::Add);
            }
            _ => panic!("Expected binary operation in function body"),
        }
    }

    #[test]
    fn test_cmd_funcdef_with_complex_expressions() {
        use App;

        let mut app = App::new();

        // Create a function with a more complex expression
        let result = cmd_funcdef(&mut app, "calc(x, y, z) = x * y + z");
        assert!(
            result.is_ok(),
            "cmd_funcdef should succeed with complex expressions"
        );

        // Check that function was added to AST
        assert_eq!(app.ast.function_defines.len(), 1);
        let func_def = &app.ast.function_defines[0];
        assert_eq!(func_def.name, "calc");
        assert_eq!(func_def.parameters.len(), 3);
        assert_eq!(func_def.parameters[0].name, "x");
        assert_eq!(func_def.parameters[1].name, "y");
        assert_eq!(func_def.parameters[2].name, "z");
    }

    #[test]
    fn test_cmd_funcdef_parentheses_format() {
        use App;

        let mut app = App::new();

        // Test the new parentheses format specifically
        let result = cmd_funcdef(&mut app, "multiply(a, b) = a * b");
        assert!(
            result.is_ok(),
            "cmd_funcdef should succeed with parentheses format"
        );

        // Check that function was added to AST with correct parameters
        assert_eq!(app.ast.function_defines.len(), 1);
        let func_def = &app.ast.function_defines[0];
        assert_eq!(func_def.name, "multiply");
        assert_eq!(func_def.parameters.len(), 2);
        assert_eq!(func_def.parameters[0].name, "a");
        assert_eq!(func_def.parameters[1].name, "b");
    }
}
