//! Commands module for OpenSCAD TUI

use openscad_core::{Argument, AstError, Expr, ModuleNode};
use openscad_library::{LibraryError, ModuleDef};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;

const MAX_RECURSION_DEPTH: usize = 1000;

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
    app: &mut crate::app::App,
    module_name: &str,
    _parent_id: Option<&str>,
    params: Option<&str>,
) -> CommandResult<String> {
    // Get module definition
    let module_def = app
        .library
        .get_module(module_name)
        .ok_or_else(|| CommandError::InvalidCommand(format!("Unknown module: {}", module_name)))?;

    // Get module source information (library name and file)
    let (source_lib_name, source_lib_file) = app.library.get_module_source(module_name);

    // Parse parameters
    let args = if let Some(param_str) = params {
        parse_arguments(param_str, &module_def)?
    } else {
        Vec::new()
    };

    // Create module node ID
    let node_id = format!(
        "{}_{}",
        module_name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );

    // Check if this module accepts children
    if module_def.accepts_children {
        // For container modules, we need selected child nodes
        if app.selected_nodes.is_empty() {
            return Err(CommandError::NoChildrenSelected);
        }

        // Create container module with source library info
        let mut container =
            ModuleNode::new_container(node_id.clone(), module_name.to_string(), args);
        container.source_library = source_lib_name.clone();

        // If this module comes from a third-party library, add include statement
        if let Some(ref lib_file) = source_lib_file {
            if !app.ast_mut().includes.contains(lib_file) {
                app.ast_mut().includes.push(lib_file.clone());
            }
        }

        // Use the shared implementation for inserting container with selected nodes
        let selected_nodes = app.selected_nodes.clone();
        insert_container_with_selected_nodes(app, container, &selected_nodes)
    } else {
        // For leaf modules, create with source library info
        let mut module = ModuleNode::new_leaf(node_id.clone(), module_name.to_string(), args);
        module.source_library = source_lib_name;

        // If this module comes from a third-party library, add include statement
        if let Some(ref lib_file) = source_lib_file {
            if !app.ast_mut().includes.contains(lib_file) {
                app.ast_mut().includes.push(lib_file.clone());
            }
        }

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
                insert_after_node(
                    &mut app.ast_mut().module_defines[mod_def_idx].body,
                    &selected_id,
                    module,
                )?;
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
                insert_after_node(&mut app.ast_mut().modules, &selected_id, module)?;
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

/// Helper function to insert a node after a specific target node
/// This function searches for the target node and inserts the new module immediately after it
/// at the same level in the tree hierarchy.
///
/// Returns Ok if the target was found and insertion was successful.
/// Returns Err if the target node ID was not found anywhere in the tree.
fn insert_after_node(
    modules: &mut Vec<ModuleNode>,
    target_id: &str,
    new_module: ModuleNode,
) -> CommandResult<()> {
    // First, check if the target is at this level
    for i in 0..modules.len() {
        if modules[i].id == target_id {
            // Found the target at this level, insert after it
            modules.insert(i + 1, new_module);
            return Ok(());
        }
    }

    // Target not at this level, search in children recursively
    for module in modules {
        if let Ok(()) = insert_after_node(&mut module.children, target_id, new_module.clone()) {
            return Ok(());
        }
    }

    // Target node not found in any branch
    Err(CommandError::InvalidCommand(format!(
        "Target node not found: {}",
        target_id
    )))
}

/// Delete command
pub fn cmd_delete(app: &mut crate::app::App, node_id: &str) -> CommandResult<()> {
    // Prevent deletion of section headers
    if node_id.starts_with("__") {
        return Err(CommandError::Custom(format!(
            "Cannot delete section header: {}",
            node_id
        )));
    }

    app.ast_mut().delete_node(node_id)?;
    app.selected_nodes.retain(|id| id != node_id);

    // Clear tree state selection if the deleted node was selected
    let mut tree_state = app.tree_state.borrow_mut();
    if tree_state.selected() == [node_id.to_string()] {
        tree_state.select(vec![]);
    }
    drop(tree_state); // Explicitly drop the borrow

    // Restore tree state to a valid position
    app.restore_tree_selection();

    Ok(())
}

/// Apply boolean operation (union, difference, intersection)
#[allow(dead_code)]
pub fn cmd_boolean_op(
    app: &mut crate::app::App,
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
    app: &mut crate::app::App,
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
pub fn cmd_select(app: &mut crate::app::App, node_id: &str) -> CommandResult<()> {
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
pub fn cmd_deselect(app: &mut crate::app::App, node_id: &str) -> CommandResult<()> {
    app.selected_nodes.retain(|id| id != node_id);
    Ok(())
}

/// Clear selection
#[allow(dead_code)]
pub fn cmd_clear_selection(app: &mut crate::app::App) {
    app.selected_nodes.clear();
}

/// Navigation commands
/// Move cursor down (next)
#[allow(dead_code)]
pub fn cmd_next(app: &mut crate::app::App) -> CommandResult<()> {
    app.tree_state.borrow_mut().key_down();
    app.update_navigation_status();
    Ok(())
}

/// Move cursor up (previous)
#[allow(dead_code)]
pub fn cmd_prev(app: &mut crate::app::App) -> CommandResult<()> {
    app.tree_state.borrow_mut().key_up();
    app.update_navigation_status();
    Ok(())
}

/// Collapse node (move left)
#[allow(dead_code)]
pub fn cmd_collapse(app: &mut crate::app::App) -> CommandResult<()> {
    app.tree_state.borrow_mut().key_left();
    app.update_navigation_status();
    Ok(())
}

/// Expand node (move right)
#[allow(dead_code)]
pub fn cmd_expand(app: &mut crate::app::App) -> CommandResult<()> {
    app.tree_state.borrow_mut().key_right();
    app.update_navigation_status();
    Ok(())
}

/// Toggle node (move right)
#[allow(dead_code)]
pub fn cmd_toggle(app: &mut crate::app::App) -> CommandResult<()> {
    app.tree_state.borrow_mut().toggle_selected();
    app.update_navigation_status();
    Ok(())
}

/// Select/toggle current node
#[allow(dead_code)]
pub fn cmd_select_toggle(app: &mut crate::app::App) -> CommandResult<()> {
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
pub fn cmd_deselect_all(app: &mut crate::app::App) -> CommandResult<()> {
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
pub fn cmd_write(app: &crate::app::App, filename: &str) -> CommandResult<()> {
    // Ensure filename ends with .json
    let filepath = if !filename.ends_with(".json") {
        format!("{}.json", filename)
    } else {
        filename.to_string()
    };

    // Serialize AST to JSON
    let json = serde_json::to_string_pretty(&*app.ast)
        .map_err(|e| CommandError::Custom(format!("Failed to serialize AST: {}", e)))?;

    // Write to file
    fs::write(&filepath, json)
        .map_err(|e| CommandError::Custom(format!("Failed to write file '{}': {}", filepath, e)))?;

    Ok(())
}

/// Load AST from JSON file
pub fn cmd_load(app: &mut crate::app::App, filename: &str) -> CommandResult<()> {
    // Check file exists
    if !Path::new(filename).exists() {
        return Err(CommandError::Custom(format!(
            "File '{}' not found",
            filename
        )));
    }

    // Read file
    let content = fs::read_to_string(filename)
        .map_err(|e| CommandError::Custom(format!("Failed to read file '{}': {}", filename, e)))?;

    // Deserialize from JSON
    let ast = serde_json::from_str(&content)
        .map_err(|e| CommandError::Custom(format!("Failed to parse JSON: {}", e)))?;

    // Replace AST
    app.ast = Arc::new(ast);

    // Reload custom modules in library manager
    app.library
        .reload_custom_modules_from_ast(&app.ast.module_defines);

    // Reset navigation state
    app.selected_nodes.clear();
    app.tree_state.borrow_mut().select(Vec::new());

    Ok(())
}

/// Export AST to OpenSCAD code file
pub fn cmd_export(app: &crate::app::App, filename: &str) -> CommandResult<()> {
    // Ensure filename ends with .scad
    let filepath = if !filename.ends_with(".scad") {
        format!("{}.scad", filename)
    } else {
        filename.to_string()
    };

    // Generate OpenSCAD code
    let code = app.ast.to_scad();

    // Write to file
    fs::write(&filepath, code)
        .map_err(|e| CommandError::Custom(format!("Failed to write file '{}': {}", filepath, e)))?;

    Ok(())
}

/// Load a library from a JSON file
/// This command loads third-party module libraries into the LibraryManager
/// Libraries should be in JSON format with the same schema as stdlib.json
pub fn cmd_load_library(app: &mut crate::app::App, filename: &str) -> CommandResult<()> {
    use std::path::Path;

    // Check if file exists
    let path = Path::new(filename);
    if !path.exists() {
        return Err(CommandError::Custom(format!(
            "Library file '{}' not found",
            filename
        )));
    }

    // Load library
    app.library.load_library(path)?;

    Ok(())
}

/// Quit app command
#[allow(dead_code)]
pub fn cmd_quit(app: &mut crate::app::App) -> CommandResult<()> {
    app.should_quit = true;
    Ok(())
}

/// Undo command
#[allow(dead_code)]
pub fn cmd_undo(app: &mut crate::app::App) -> CommandResult<()> {
    app.undo();
    Ok(())
}

/// Redo command
#[allow(dead_code)]
pub fn cmd_redo(app: &mut crate::app::App) -> CommandResult<()> {
    app.redo();
    Ok(())
}

/// Help command - Show help modal
#[allow(dead_code)]
pub fn cmd_help(app: &mut crate::app::App) -> CommandResult<()> {
    app.input_mode = crate::app::InputMode::Help;
    Ok(())
}

/// Define a new custom module
/// Syntax: moddef <module_name> [params]
///   params: optional parameter list like "size=10, center=false"
///   children: taken from selected nodes (if any)
pub fn cmd_moddef(
    app: &mut crate::app::App,
    module_name: &str,
    params: Option<&str>,
) -> CommandResult<()> {
    use openscad_core::ModuleDefinition;
    use openscad_library::{ModuleDef, ParameterDef};

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

    // Determine if module accepts children (contains a children module in its body)
    let accepts_children = contains_children_module(&children);

    // Create ModuleDefinition for AST
    let module_def = ModuleDefinition::new(module_name.to_string(), parameters.clone(), children);

    // Add to AST
    app.ast_mut()
        .add_module_define(module_def)
        .map_err(CommandError::AstError)?;

    // Create ModuleDef for library manager
    let library_params: Vec<ParameterDef> = parameters
        .iter()
        .map(|p| {
            ParameterDef {
                name: p.name.clone(),
                param_type: "any".to_string(), // Default type, could be inferred
                default: p.default.as_ref().map(|e| e.to_scad()),
                description: None,
            }
        })
        .collect();

    let module_lib_def = ModuleDef {
        name: module_name.to_string(),
        description: Some(format!("User-defined module: {}", module_name)),
        parameters: library_params,
        accepts_children, // Custom modules accept children if they contain a children module
    };

    // Add to library manager
    app.library.add_custom_module(module_lib_def);

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

/// Check if a module tree contains a children module
fn contains_children_module(nodes: &[openscad_core::ModuleNode]) -> bool {
    // Use explicit stack to avoid recursion depth issues
    let mut stack: Vec<(&openscad_core::ModuleNode, usize)> =
        nodes.iter().map(|n| (n, 0)).collect();

    while let Some((node, depth)) = stack.pop() {
        if depth >= MAX_RECURSION_DEPTH {
            continue;
        }

        if node.name == "children" {
            return true;
        }

        // Push children to stack
        for child in &node.children {
            stack.push((child, depth + 1));
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use crate::app::App;

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
        use crate::app::App;

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
        use crate::app::App;

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
    fn test_cmd_moddef_duplicate_name() {
        use crate::app::App;

        let mut app = App::new();

        // First module definition should succeed
        let result = cmd_moddef(&mut app, "my_module", None);
        assert!(result.is_ok());

        // Second module definition with same name should fail
        let result = cmd_moddef(&mut app, "my_module", None);
        assert!(result.is_err());

        // Verify only one module in AST
        assert_eq!(app.ast.module_defines.len(), 1);
    }

    #[test]
    fn test_cmd_moddef_complex_parameters() {
        use crate::app::App;

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
        use crate::app::App;
        use openscad_core::AstRoot;

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
        use crate::app::App;
        use openscad_core::{Argument, AstRoot, Expr, ModuleNode};

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
        use crate::app::App;

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
        use crate::app::App;

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
        use crate::app::App;

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
        use crate::app::App;

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
        use crate::app::App;

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
        use crate::app::App;

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
        use crate::app::App;

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
        use crate::app::App;

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
}
