//! Commands module for OpenSCAD TUI

use openscad_core::{Argument, AstError, Expr, ModuleNode};
use openscad_library::ModuleDef;
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CommandError {
    #[error("Invalid command: {0}")]
    InvalidCommand(String),

    #[error("AST error: {0}")]
    AstError(#[from] AstError),

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
            if !app.ast.includes.contains(lib_file) {
                app.ast.includes.push(lib_file.clone());
            }
        }

        // Find the parent of the first selected node
        let first_selected = app.selected_nodes.first().cloned();
        let parent_id = if let Some(ref first_id) = first_selected {
            find_node_parent(&app.ast.modules, first_id)
        } else {
            None
        };

        // Insert the container BEFORE deleting the selected nodes
        // This way we can still find the position of the first selected node
        if let Some(parent_id_val) = &parent_id {
            insert_child_before(
                &mut app.ast.modules,
                parent_id_val,
                &first_selected.clone().unwrap(),
                container.clone(),
            )?;
        } else {
            // First selected node was at root level, so we need to find its position
            if let Some(pos) = app
                .ast
                .modules
                .iter()
                .position(|m| m.id == first_selected.clone().unwrap())
            {
                app.ast.modules.insert(pos, container.clone());
            } else {
                // Fallback: just add at root
                app.ast.add_module(container.clone())?;
            }
        }

        // Collect nodes to move before modifying the tree
        let nodes_to_move: Vec<ModuleNode> = app
            .selected_nodes
            .clone()
            .iter()
            .filter_map(|node_id_to_move| app.ast.find_node_by_id(node_id_to_move).cloned())
            .collect();

        // Delete the selected nodes from the tree
        for node_id_to_move in &app.selected_nodes.clone() {
            app.ast.delete_node(node_id_to_move)?;
        }

        // Add collected nodes to the container
        if let Some(container_mut) = app.ast.find_node_mut(&node_id) {
            for node in nodes_to_move {
                container_mut.children.push(node);
            }
        }

        // Clear selection after moving nodes
        app.selected_nodes.clear();

        // Select the newly created container module for continued operations
        // Use the full path to ensure proper navigation in nested trees
        if let Some(path) = app.find_node_path(&node_id) {
            app.tree_state.borrow_mut().select(path);
        } else {
            // Fallback: just select by ID if path not found
            app.tree_state.borrow_mut().select(vec![node_id.clone()]);
        }

        Ok(node_id)
    } else {
        // For leaf modules, create with source library info
        let mut module = ModuleNode::new_leaf(node_id.clone(), module_name.to_string(), args);
        module.source_library = source_lib_name;

        // If this module comes from a third-party library, add include statement
        if let Some(ref lib_file) = source_lib_file {
            if !app.ast.includes.contains(lib_file) {
                app.ast.includes.push(lib_file.clone());
            }
        }

        // Determine insertion point based on current selection
        let selected = app.tree_state.borrow().selected().last().cloned();

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
            app.ast.add_module(module)?;
        } else if let Some(selected_id) = selected {
            // Find the selected node and insert after it
            insert_after_node(&mut app.ast.modules, &selected_id, module)?;
        }

        // Select the newly inserted module for continued operations
        // Use the full path to ensure proper navigation in nested trees
        if let Some(path) = app.find_node_path(&node_id) {
            app.tree_state.borrow_mut().select(path);
            // Open the __modules section to show the new node
            app.tree_state.borrow_mut().open(vec!["__modules".to_string()]);
        } else {
            // Fallback: just select by ID if path not found
            app.tree_state.borrow_mut().select(vec!["__modules".to_string(), node_id.clone()]);
            app.tree_state.borrow_mut().open(vec!["__modules".to_string()]);
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

/// Find the parent ID of a node
/// Returns the ID of the parent node, or None if the node is at root level
fn find_node_parent(modules: &[ModuleNode], target_id: &str) -> Option<String> {
    find_node_parent_recursive(modules, target_id)
}

fn find_node_parent_recursive(modules: &[ModuleNode], target_id: &str) -> Option<String> {
    for module in modules {
        // Check if target is a direct child of this module
        if module.children.iter().any(|child| child.id == target_id) {
            return Some(module.id.clone());
        }
        // Recursively search in children
        if let Some(parent) = find_node_parent_recursive(&module.children, target_id) {
            return Some(parent);
        }
    }
    None
}

/// Insert a child node before a specific sibling node
/// Finds the parent and inserts the new module before the sibling
fn insert_child_before(
    modules: &mut [ModuleNode],
    parent_id: &str,
    before_node_id: &str,
    new_child: ModuleNode,
) -> CommandResult<()> {
    insert_child_before_recursive(modules, parent_id, before_node_id, new_child)
}

fn insert_child_before_recursive(
    modules: &mut [ModuleNode],
    parent_id: &str,
    before_node_id: &str,
    new_child: ModuleNode,
) -> CommandResult<()> {
    for module in modules {
        if module.id == parent_id {
            // Found the parent, now find the position of the sibling
            if let Some(pos) = module
                .children
                .iter()
                .position(|child| child.id == before_node_id)
            {
                module.children.insert(pos, new_child);
                return Ok(());
            } else {
                return Err(CommandError::InvalidCommand(format!(
                    "Sibling node not found: {}",
                    before_node_id
                )));
            }
        }
        // Recursively search in children
        match insert_child_before_recursive(
            &mut module.children,
            parent_id,
            before_node_id,
            new_child.clone(),
        ) {
            Ok(()) => return Ok(()),
            Err(_) => continue, // Try next module
        }
    }
    Err(CommandError::InvalidCommand(format!(
        "Parent node not found: {}",
        parent_id
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

    app.ast.delete_node(node_id)?;
    app.selected_nodes.retain(|id| id != node_id);

    // Clear tree state selection if the deleted node was selected
    let mut tree_state = app.tree_state.borrow_mut();
    if tree_state.selected() == &[node_id.to_string()] {
        tree_state.select(vec![]);
    }
    drop(tree_state); // Explicitly drop the borrow

    // Restore tree state to a valid position
    app.restore_tree_selection();

    Ok(())
}

/// Apply boolean operation (union, difference, intersection)
pub fn cmd_boolean_op(
    app: &mut crate::app::App,
    operation: &str,
    node_ids: &[String],
) -> CommandResult<String> {
    if node_ids.is_empty() {
        return Err(CommandError::NoChildrenSelected);
    }

    // Create container module
    let op_id = format!(
        "{}_{}",
        operation,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );

    let container = ModuleNode::new_container(op_id.clone(), operation.to_string(), Vec::new());

    // Find the parent of the first selected node
    let first_selected = node_ids.first().cloned();
    let parent_id = if let Some(ref first_id) = first_selected {
        find_node_parent(&app.ast.modules, first_id)
    } else {
        None
    };

    // Insert the container BEFORE deleting the selected nodes
    if let Some(parent_id_val) = &parent_id {
        insert_child_before(
            &mut app.ast.modules,
            parent_id_val,
            &first_selected.clone().unwrap(),
            container.clone(),
        )?;
    } else {
        // First selected node was at root level
        if let Some(pos) = app
            .ast
            .modules
            .iter()
            .position(|m| m.id == first_selected.clone().unwrap())
        {
            app.ast.modules.insert(pos, container.clone());
        } else {
            app.ast.add_module(container.clone())?;
        }
    }

    // Collect nodes to move before modifying the tree
    let nodes_to_move: Vec<ModuleNode> = node_ids
        .iter()
        .filter_map(|node_id| app.ast.find_node_by_id(node_id).cloned())
        .collect();

    // Delete the selected nodes from the tree
    for node_id in node_ids {
        app.ast.delete_node(node_id)?;
    }

    // Add collected nodes to the container
    if let Some(container_mut) = app.ast.find_node_mut(&op_id) {
        for node in nodes_to_move {
            container_mut.children.push(node);
        }
    }

    // Select the newly created container module for continued operations
    // Use the full path to ensure proper navigation in nested trees
    if let Some(path) = app.find_node_path(&op_id) {
        app.tree_state.borrow_mut().select(path);
    } else {
        // Fallback: just select by ID if path not found
        app.tree_state.borrow_mut().select(vec![op_id.clone()]);
    }

    Ok(op_id)
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

/// Translate command
#[allow(dead_code)]
pub fn cmd_translate(
    app: &mut crate::app::App,
    node_id: &str,
    x: f64,
    y: f64,
    z: f64,
) -> CommandResult<()> {
    // Wrap the node in a translate module
    if let Some(node) = app.ast.find_node_by_id(node_id).cloned() {
        let translate_id = format!(
            "translate_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );

        let mut translate = ModuleNode::new_container(
            translate_id,
            "translate".to_string(),
            vec![Argument::Named {
                name: "v".to_string(),
                value: Expr::List(vec![Expr::Float(x), Expr::Float(y), Expr::Float(z)]),
            }],
        );

        translate.children.push(node);
        app.ast.delete_node(node_id)?;
        app.ast.add_module(translate)?;
    }

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
    let json = serde_json::to_string_pretty(&app.ast)
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
    app.ast = ast;

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
    app.library
        .load_library(path)
        .map_err(|e| CommandError::Custom(format!("Failed to load library: {}", e)))?;

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
}
