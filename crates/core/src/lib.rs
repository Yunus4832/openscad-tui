//! OpenSCAD AST (Abstract Syntax Tree) implementation
//! 
//! This module provides the core API for building and manipulating OpenSCAD code structures.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur during AST operations
#[derive(Error, Debug)]
pub enum AstError {
    #[error("Invalid identifier: {0}")]
    InvalidIdentifier(String),
    
    #[error("Duplicate identifier: {0}")]
    DuplicateIdentifier(String),
    
    #[error("Node not found: {0}")]
    NodeNotFound(String),
    
    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),
    
    #[error("Serialization error: {0}")]
    SerializationError(String),
}

pub type Result<T> = std::result::Result<T, AstError>;

/// Represents an OpenSCAD expression
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Expr {
    /// Boolean literal
    Boolean(bool),
    /// Integer literal
    Integer(i64),
    /// Float literal
    Float(f64),
    /// String literal
    String(String),
    /// Undefined value
    Undef,
    /// Variable/identifier lookup
    Identifier(String),
    /// List expression [a, b, c]
    List(Vec<Expr>),
    /// Range expression [from:to] or [from:to:step]
    Range {
        from: Box<Expr>,
        to: Box<Expr>,
        step: Option<Box<Expr>>,
    },
    /// Binary operation
    BinOp {
        left: Box<Expr>,
        op: BinOp,
        right: Box<Expr>,
    },
    /// Unary operation
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    /// Ternary conditional
    Ternary {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    /// Index operation
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
    },
    /// Function call
    FunctionCall {
        name: String,
        args: Vec<Argument>,
    },
}

impl Expr {
    /// Parse a string expression - simplified parser for parameter input
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        
        // Try boolean
        if trimmed == "true" {
            return Ok(Expr::Boolean(true));
        }
        if trimmed == "false" {
            return Ok(Expr::Boolean(false));
        }
        if trimmed == "undef" {
            return Ok(Expr::Undef);
        }
        
        // Try number
        if let Ok(i) = trimmed.parse::<i64>() {
            return Ok(Expr::Integer(i));
        }
        if let Ok(f) = trimmed.parse::<f64>() {
            return Ok(Expr::Float(f));
        }
        
        // Try string
        if trimmed.starts_with('"') && trimmed.ends_with('"') {
            return Ok(Expr::String(trimmed[1..trimmed.len()-1].to_string()));
        }
        
        // Try list
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            return parse_list(&trimmed[1..trimmed.len()-1]);
        }
        
        // Try identifier
        if is_valid_identifier(trimmed) {
            return Ok(Expr::Identifier(trimmed.to_string()));
        }
        
        Err(AstError::InvalidParameter(format!(
            "Cannot parse expression: {}",
            trimmed
        )))
    }
    
    /// Generate OpenSCAD code for this expression
    pub fn to_scad(&self) -> String {
        match self {
            Expr::Boolean(b) => b.to_string(),
            Expr::Integer(i) => i.to_string(),
            Expr::Float(f) => f.to_string(),
            Expr::String(s) => format!("\"{}\"", s),
            Expr::Undef => "undef".to_string(),
            Expr::Identifier(name) => name.clone(),
            Expr::List(items) => {
                let items_str = items.iter()
                    .map(|e| e.to_scad())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{}]", items_str)
            },
            Expr::Range { from, to, step } => {
                if let Some(s) = step {
                    format!("[{}:{}:{}]", from.to_scad(), to.to_scad(), s.to_scad())
                } else {
                    format!("[{}:{}]", from.to_scad(), to.to_scad())
                }
            },
            Expr::BinOp { left, op, right } => {
                format!("{} {} {}", left.to_scad(), op.to_string(), right.to_scad())
            },
            Expr::UnaryOp { op, expr } => {
                format!("{}{}", op.to_string(), expr.to_scad())
            },
            Expr::Ternary { condition, then_expr, else_expr } => {
                format!("{} ? {} : {}",
                    condition.to_scad(),
                    then_expr.to_scad(),
                    else_expr.to_scad())
            },
            Expr::Index { expr, index } => {
                format!("{}[{}]", expr.to_scad(), index.to_scad())
            },
            Expr::FunctionCall { name, args } => {
                let args_str = args.iter()
                    .map(|a| match a {
                        Argument::Positional(e) => e.to_scad(),
                        Argument::Named { name: n, value: v } => format!("{}={}", n, v.to_scad()),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", name, args_str)
            },
        }
    }
}

/// Binary operators
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Power,
    Gt,
    Gte,
    Lt,
    Lte,
    Eq,
    Neq,
    And,
    Or,
}

impl BinOp {
    pub fn to_string(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Power => "^",
            BinOp::Gt => ">",
            BinOp::Gte => ">=",
            BinOp::Lt => "<",
            BinOp::Lte => "<=",
            BinOp::Eq => "==",
            BinOp::Neq => "!=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }
}

/// Unary operators
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum UnaryOp {
    Not,
    Plus,
    Minus,
}

impl UnaryOp {
    pub fn to_string(self) -> &'static str {
        match self {
            UnaryOp::Not => "!",
            UnaryOp::Plus => "+",
            UnaryOp::Minus => "-",
        }
    }
}
/// Function and module arguments
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Argument {
    Positional(Expr),
    Named { name: String, value: Expr },
}

/// A module node in the AST
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleNode {
    /// Unique identifier for this node
    pub id: String,
    
    /// Module name (cube, sphere, translate, etc.)
    pub name: String,
    
    /// Module arguments
    pub args: Vec<Argument>,
    
    /// Child modules
    pub children: Vec<ModuleNode>,
    
    /// Metadata for display
    pub display_name: Option<String>,
}

impl ModuleNode {
    /// Create a new leaf module (no children)
    pub fn new_leaf(id: String, name: String, args: Vec<Argument>) -> Self {
        Self {
            id,
            name,
            args,
            children: Vec::new(),
            display_name: None,
        }
    }
    
    /// Create a new module that can have children
    pub fn new_container(id: String, name: String, args: Vec<Argument>) -> Self {
        Self {
            id,
            name,
            args,
            children: Vec::new(),
            display_name: None,
        }
    }
    
    /// Set display name for UI rendering
    pub fn with_display_name(mut self, display_name: String) -> Self {
        self.display_name = Some(display_name);
        self
    }
    
    /// Get the display name, fallback to module name with args
    pub fn get_display_name(&self) -> String {
        if let Some(ref name) = self.display_name {
            return name.clone();
        }
        
        if self.args.is_empty() {
            self.name.clone()
        } else {
            let args_str = self.args.iter()
                .map(|a| match a {
                    Argument::Positional(e) => e.to_scad(),
                    Argument::Named { name: n, value: v } => format!("{}={}", n, v.to_scad()),
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({})", self.name, args_str)
        }
    }
    
    /// Generate OpenSCAD code
    pub fn to_scad(&self, indent: usize) -> String {
        let indent_str = " ".repeat(indent);
        let args_str = if self.args.is_empty() {
            "()".to_string()
        } else {
            let args = self.args.iter()
                .map(|a| match a {
                    Argument::Positional(e) => e.to_scad(),
                    Argument::Named { name: n, value: v } => format!("{}={}", n, v.to_scad()),
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("({})", args)
        };
        
        if self.children.is_empty() {
            format!("{}{}{};", indent_str, self.name, args_str)
        } else {
            let mut result = format!("{}{}{} {{\n", indent_str, self.name, args_str);
            for child in &self.children {
                result.push_str(&child.to_scad(indent + 4));
                result.push('\n');
            }
            result.push_str(&format!("{}}}", indent_str));
            result
        }
    }
}

/// The root AST structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstRoot {
    /// All top-level modules
    pub modules: Vec<ModuleNode>,
    
    /// Included libraries
    pub includes: Vec<String>,
    
    /// Used libraries
    pub uses: Vec<String>,
}

impl AstRoot {
    /// Create a new empty AST
    pub fn new() -> Self {
        Self {
            modules: Vec::new(),
            includes: Vec::new(),
            uses: Vec::new(),
        }
    }
    
    /// Add a module to the root level
    pub fn add_module(&mut self, module: ModuleNode) -> Result<()> {
        // Check for duplicate identifiers
        if self.find_node_by_id(&module.id).is_some() {
            return Err(AstError::DuplicateIdentifier(module.id.clone()));
        }
        self.modules.push(module);
        Ok(())
    }
    
    /// Find a node by ID
    pub fn find_node_by_id(&self, id: &str) -> Option<&ModuleNode> {
        Self::find_in_vec(&self.modules, id)
    }
    
    fn find_in_vec<'a>(modules: &'a [ModuleNode], id: &str) -> Option<&'a ModuleNode> {
        for module in modules {
            if module.id == id {
                return Some(module);
            }
            if let Some(found) = Self::find_in_vec(&module.children, id) {
                return Some(found);
            }
        }
        None
    }
    
    /// Find a mutable node by ID
    pub fn find_node_mut(&mut self, id: &str) -> Option<&mut ModuleNode> {
        Self::find_in_vec_mut(&mut self.modules, id)
    }
    
    fn find_in_vec_mut<'a>(modules: &'a mut [ModuleNode], id: &str) -> Option<&'a mut ModuleNode> {
        for module in modules {
            if module.id == id {
                return Some(module);
            }
            if let Some(found) = Self::find_in_vec_mut(&mut module.children, id) {
                return Some(found);
            }
        }
        None
    }
    
    /// Insert a child module into a parent module
    pub fn insert_child(&mut self, parent_id: &str, child: ModuleNode) -> Result<()> {
        // Check for duplicate identifiers first
        if self.find_node_by_id(&child.id).is_some() {
            return Err(AstError::DuplicateIdentifier(child.id.clone()));
        }
        
        // Find parent and insert
        if Self::insert_child_in_vec(&mut self.modules, parent_id, child) {
            Ok(())
        } else {
            Err(AstError::NodeNotFound(parent_id.to_string()))
        }
    }
    
    fn insert_child_in_vec(modules: &mut [ModuleNode], parent_id: &str, child: ModuleNode) -> bool {
        for module in modules {
            if module.id == parent_id {
                module.children.push(child);
                return true;
            }
            if Self::insert_child_in_vec(&mut module.children, parent_id, child.clone()) {
                return true;
            }
        }
        false
    }
    
    /// Delete a node and all its children
    pub fn delete_node(&mut self, id: &str) -> Result<()> {
        Self::delete_node_in_vec(&mut self.modules, id);
        Ok(())
    }
    
    fn delete_node_in_vec(modules: &mut Vec<ModuleNode>, id: &str) -> bool {
        for i in (0..modules.len()).rev() {
            if modules[i].id == id {
                modules.remove(i);
                return true;
            }
        }
        
        for module in modules {
            if Self::delete_node_in_vec(&mut module.children, id) {
                return true;
            }
        }
        
        false
    }
    
    /// Generate complete OpenSCAD code
    pub fn to_scad(&self) -> String {
        let mut result = String::new();
        
        // Add includes
        for include in &self.includes {
            result.push_str(&format!("include <{}>;\n", include));
        }
        
        if !self.includes.is_empty() {
            result.push('\n');
        }
        
        // Add uses
        for use_lib in &self.uses {
            result.push_str(&format!("use <{}>;\n", use_lib));
        }
        
        if !self.uses.is_empty() {
            result.push('\n');
        }
        
        // Add modules
        for module in &self.modules {
            result.push_str(&module.to_scad(0));
            result.push('\n');
        }
        
        result
    }
}

impl Default for AstRoot {
    fn default() -> Self {
        Self::new()
    }
}

// Helper functions for parsing

fn parse_list(content: &str) -> Result<Expr> {
    if content.trim().is_empty() {
        return Ok(Expr::List(Vec::new()));
    }
    
    let items: Vec<&str> = content.split(',').collect();
    let mut exprs = Vec::new();
    
    for item in items {
        exprs.push(Expr::parse(item.trim())?);
    }
    
    Ok(Expr::List(exprs))
}

fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    
    let first = s.chars().next().unwrap();
    if !first.is_alphabetic() && first != '_' {
        return false;
    }
    
    s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_expr_parse_integer() {
        assert_eq!(Expr::parse("42").unwrap(), Expr::Integer(42));
    }
    
    #[test]
    fn test_expr_parse_boolean() {
        assert_eq!(Expr::parse("true").unwrap(), Expr::Boolean(true));
        assert_eq!(Expr::parse("false").unwrap(), Expr::Boolean(false));
    }
    
    #[test]
    fn test_expr_to_scad() {
        let expr = Expr::Integer(42);
        assert_eq!(expr.to_scad(), "42");
    }
    
    #[test]
    fn test_module_node_to_scad() {
        let module = ModuleNode::new_leaf(
            "cube1".to_string(),
            "cube".to_string(),
            vec![Argument::Named {
                name: "size".to_string(),
                value: Expr::List(vec![
                    Expr::Integer(10),
                    Expr::Integer(10),
                    Expr::Integer(10),
                ]),
            }],
        );
        
        let scad = module.to_scad(0);
        assert!(scad.contains("cube"));
        assert!(scad.contains("size"));
    }
    
    #[test]
    fn test_ast_root_basic() {
        let mut ast = AstRoot::new();
        let module = ModuleNode::new_leaf(
            "cube1".to_string(),
            "cube".to_string(),
            vec![],
        );
        
        ast.add_module(module).unwrap();
        assert_eq!(ast.modules.len(), 1);
    }
}
