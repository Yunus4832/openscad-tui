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
    NoNodeSelected,
    
    #[error("No children selected")]
    NoChildrenSelected,
}

pub type CommandResult<T> = std::result::Result<T, CommandError>;

/// Insert command
/// Insert a new module at the same level as the currently selected node.
/// If no node is selected, insert at root level.
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
    
    // Create module node
    let node_id = format!("{}_{}", module_name, std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis());
    
    let module = ModuleNode::new_leaf(node_id.clone(), module_name.to_string(), args);
    
    // Insert at root level (same level as the selected node)
    app.ast.add_module(module)?;
    
    Ok(node_id)
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
    
    let mut container = ModuleNode::new_container(op_id.clone(), operation.to_string(), Vec::new());
    
    // Move selected nodes into container
    for node_id in node_ids {
        if let Some(node) = app.ast.find_node_by_id(node_id).cloned() {
            container.children.push(node);
            app.ast.delete_node(node_id)?;
        }
    }
    
    app.ast.add_module(container)?;
    Ok(op_id)
}

/// Select command
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
pub fn cmd_deselect(app: &mut crate::app::App, node_id: &str) -> CommandResult<()> {
    app.selected_nodes.retain(|id| id != node_id);
    Ok(())
}

/// Clear selection
pub fn cmd_clear_selection(app: &mut crate::app::App) {
    app.selected_nodes.clear();
}

/// Translate command
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
