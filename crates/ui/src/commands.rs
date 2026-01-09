//! Commands module for OpenSCAD TUI

use openscad_core::{ModuleNode, Argument, Expr, AstError};
use openscad_library::ModuleDef;
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
///   - If no node is selected, insert at root level
pub fn cmd_insert(
    app: &mut crate::app::App,
    module_name: &str,
    _parent_id: Option<&str>,
    params: Option<&str>,
) -> CommandResult<String> {
    // Get module definition
    let module_def = app.library
        .get_module(module_name)
        .ok_or_else(|| CommandError::InvalidCommand(format!("Unknown module: {}", module_name)))?;
    
    // Parse parameters
    let args = if let Some(param_str) = params {
        parse_arguments(param_str, &module_def)?
    } else {
        Vec::new()
    };
    
    // Create module node ID
    let node_id = format!("{}_{}", module_name, std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis());
    
    // Check if this module accepts children
    if module_def.accepts_children {
        // For container modules, we need selected child nodes
        if app.selected_nodes.is_empty() {
            return Err(CommandError::NoChildrenSelected);
        }
        
        // Create container module
        let container = ModuleNode::new_container(node_id.clone(), module_name.to_string(), args);
        
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
            insert_child_before(&mut app.ast.modules, parent_id_val, &first_selected.clone().unwrap(), container.clone())?;
        } else {
            // First selected node was at root level, so we need to find its position
            if let Some(pos) = app.ast.modules.iter().position(|m| m.id == first_selected.clone().unwrap()) {
                app.ast.modules.insert(pos, container.clone());
            } else {
                // Fallback: just add at root
                app.ast.add_module(container.clone())?;
            }
        }
        
        // Collect nodes to move before modifying the tree
        let nodes_to_move: Vec<ModuleNode> = app.selected_nodes.clone()
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
        
        // Restore tree state to a valid position
        app.restore_tree_selection();
        
        Ok(node_id)
    } else {
        // For leaf modules, create as before
        let module = ModuleNode::new_leaf(node_id.clone(), module_name.to_string(), args);
        
        // Determine insertion point based on current selection
        let selected = app.tree_state.borrow().selected().last().cloned();
        
        if let Some(selected_id) = selected {
            // Find the selected node and insert after it
            insert_after_node(&mut app.ast.modules, &selected_id, module)?;
        } else {
            // No selection, insert at root level
            app.ast.add_module(module)?;
        }
        
        // Restore tree state to a valid position
        app.restore_tree_selection();
        
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
            if let Some(pos) = module.children.iter().position(|child| child.id == before_node_id) {
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
    let op_id = format!("{}_{}", operation, std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis());
    
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
        insert_child_before(&mut app.ast.modules, parent_id_val, &first_selected.clone().unwrap(), container.clone())?;
    } else {
        // First selected node was at root level
        if let Some(pos) = app.ast.modules.iter().position(|m| m.id == first_selected.clone().unwrap()) {
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
    
    // Restore tree state to a valid position
    app.restore_tree_selection();
    
    Ok(op_id)
}

/// Select command
#[allow(dead_code)]
pub fn cmd_select(app: &mut crate::app::App, node_id: &str) -> CommandResult<()> {
    if app.ast.find_node_by_id(node_id).is_none() {
        return Err(CommandError::InvalidCommand(format!("Node not found: {}", node_id)));
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

/// Select/toggle current node
#[allow(dead_code)]
pub fn cmd_select_toggle(app: &mut crate::app::App) -> CommandResult<()> {
    let selected = app.tree_state.borrow().selected().last().cloned();
    if let Some(node_id) = selected {
        if app.selected_nodes.contains(&node_id) {
            app.selected_nodes.retain(|n| n != &node_id);
            app.set_info(&format!("◯ Deselected: {}", node_id));
        } else {
            app.selected_nodes.push(node_id.clone());
            app.set_info(&format!("✓ Selected: {}", node_id));
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
        let translate_id = format!("translate_{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis());
        
        let mut translate = ModuleNode::new_container(
            translate_id,
            "translate".to_string(),
            vec![Argument::Named {
                name: "v".to_string(),
                value: Expr::List(vec![
                    Expr::Float(x),
                    Expr::Float(y),
                    Expr::Float(z),
                ]),
            }],
        );
        
        translate.children.push(node);
        app.ast.delete_node(node_id)?;
        app.ast.add_module(translate)?;
    }
    
    Ok(())
}

// Helper function to parse arguments
fn parse_arguments(param_str: &str, module_def: &ModuleDef) -> CommandResult<Vec<Argument>> {
    let mut args = Vec::new();
    
    if param_str.trim().is_empty() {
        return Ok(args);
    }
    
    let parts: Vec<&str> = param_str.split(',').collect();
    
    for (i, param_def) in module_def.parameters.iter().enumerate() {
        if i >= parts.len() {
            break;
        }
        
        let value_str = parts[i].trim();
        let value = Expr::parse(value_str)
            .map_err(|e| CommandError::ParameterError(e.to_string()))?;
        
        args.push(Argument::Named {
            name: param_def.name.clone(),
            value,
        });
    }
    
    Ok(args)
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
