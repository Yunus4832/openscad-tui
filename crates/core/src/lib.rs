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
    UnaryOp { op: UnaryOp, expr: Box<Expr> },
    /// Ternary conditional
    Ternary {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    /// Index operation
    Index { expr: Box<Expr>, index: Box<Expr> },
    /// Function call
    FunctionCall { name: String, args: Vec<Argument> },
}

impl Expr {
    /// Parse a string expression - simplified parser for parameter input
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();

        // Check if the entire expression is wrapped in parentheses
        if trimmed.starts_with('(') && trimmed.ends_with(')') {
            // Need to make sure the parentheses are balanced
            let content = &trimmed[1..trimmed.len() - 1]; // Extract content between parentheses
            if is_balanced_parentheses(trimmed) {
                return Expr::parse(content); // Recursively parse the content inside parentheses
            }
        }

        // First check for binary operations
        if let Some(bin_op_result) = parse_binary_operation(trimmed) {
            return bin_op_result;
        }

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
            return Ok(Expr::String(trimmed[1..trimmed.len() - 1].to_string()));
        }

        // Try list
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            return parse_list(&trimmed[1..trimmed.len() - 1]);
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
                let items_str = items
                    .iter()
                    .map(|e| e.to_scad())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{}]", items_str)
            }
            Expr::Range { from, to, step } => {
                if let Some(s) = step {
                    format!("[{}:{}:{}]", from.to_scad(), to.to_scad(), s.to_scad())
                } else {
                    format!("[{}:{}]", from.to_scad(), to.to_scad())
                }
            }
            Expr::BinOp { left, op, right } => {
                let current_op_prec = op.precedence();
                let wrap_if_needed = |expr: &Expr| -> String {
                    match expr {
                        Expr::BinOp {op: sub_op, ..} if sub_op.precedence() < current_op_prec => {
                            format!("({})", expr.to_scad())
                        },
                        _ => expr.to_scad(),
                    }
                };
                format!(
                    "{} {} {}",
                    wrap_if_needed(left),
                    op.to_string(),
                    wrap_if_needed(right)
                )
            }
            Expr::UnaryOp { op, expr } => {
                format!("{}{}", op.to_string(), expr.to_scad())
            }
            Expr::Ternary {
                condition,
                then_expr,
                else_expr,
            } => {
                format!(
                    "{} ? {} : {}",
                    condition.to_scad(),
                    then_expr.to_scad(),
                    else_expr.to_scad()
                )
            }
            Expr::Index { expr, index } => {
                format!("{}[{}]", expr.to_scad(), index.to_scad())
            }
            Expr::FunctionCall { name, args } => {
                let args_str = args
                    .iter()
                    .map(|a| match a {
                        Argument::Positional(e) => e.to_scad(),
                        Argument::Named { name: n, value: v } => format!("{}={}", n, v.to_scad()),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", name, args_str)
            }
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

    /// Get the precedence level of the operator (higher number means higher precedence)
    pub fn precedence(self) -> u8 {
        match self {
            BinOp::Power => 5,                                    // Exponentiation (highest)
            BinOp::Mul | BinOp::Div | BinOp::Mod => 4, // Multiplication, division, modulo
            BinOp::Add | BinOp::Sub => 3,              // Addition, subtraction
            BinOp::Lt | BinOp::Gt | BinOp::Lte | BinOp::Gte => 2, // Comparison
            BinOp::Eq | BinOp::Neq => 1,               // Equality
            BinOp::And => 0,                           // Logical AND
            BinOp::Or => 0,                            // Logical OR (lowest)
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

/// A parameter in a function or module definition
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Parameter {
    /// Parameter name
    pub name: String,
    /// Optional default value
    pub default: Option<Expr>,
}

impl Parameter {
    /// Create a new parameter with a name
    pub fn new(name: String) -> Self {
        Self {
            name,
            default: None,
        }
    }

    /// Create a new parameter with a default value
    pub fn with_default(name: String, default: Expr) -> Self {
        Self {
            name,
            default: Some(default),
        }
    }

    /// Generate OpenSCAD code for this parameter
    pub fn to_scad(&self) -> String {
        if let Some(ref default) = self.default {
            format!("{}={}", self.name, default.to_scad())
        } else {
            self.name.clone()
        }
    }
}

/// An assignment statement (name = value)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Assignment {
    /// Variable name
    pub name: String,
    /// Value expression
    pub value: Expr,
}

impl Assignment {
    /// Create a new assignment
    pub fn new(name: String, value: Expr) -> Self {
        Self { name, value }
    }

    /// Generate OpenSCAD code for this assignment
    pub fn to_scad(&self) -> String {
        format!("{} = {}", self.name, self.value.to_scad())
    }
}

/// A global variable declaration in OpenSCAD
///
/// Global variables in OpenSCAD are declared at the top level with an assignment like:
/// `$my_var = 10;` or `my_var = [1, 2, 3];`
///
/// The `is_special` flag indicates whether it's a special variable (starts with $)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GlobalVariable {
    /// Variable name (without $ prefix for special variables)
    pub name: String,
    /// Variable value
    pub value: Expr,
    /// Whether this is a special variable (prefixed with $)
    pub is_special: bool,
}

impl GlobalVariable {
    /// Create a new global variable
    pub fn new(name: String, value: Expr) -> Self {
        Self {
            name,
            value,
            is_special: false,
        }
    }

    /// Create a new special global variable (prefixed with $)
    pub fn new_special(name: String, value: Expr) -> Self {
        Self {
            name,
            value,
            is_special: true,
        }
    }

    /// Generate OpenSCAD code for this global variable
    pub fn to_scad(&self) -> String {
        let var_name = if self.is_special {
            format!("${}", self.name)
        } else {
            self.name.clone()
        };
        format!("{} = {};", var_name, self.value.to_scad())
    }
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

    /// Source library (None = built-in, Some = third-party library name)
    pub source_library: Option<String>,
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
            source_library: None,
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
            source_library: None,
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
            let args_str = self
                .args
                .iter()
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
            let args = self
                .args
                .iter()
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

/// A function definition in OpenSCAD
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// Function name
    pub name: String,
    /// Function parameters
    pub parameters: Vec<Parameter>,
    /// Function body expression
    pub body: Expr,
}

impl FunctionDefinition {
    /// Create a new function definition
    pub fn new(name: String, parameters: Vec<Parameter>, body: Expr) -> Self {
        Self {
            name,
            parameters,
            body,
        }
    }

    /// Generate OpenSCAD code for this function
    pub fn to_scad(&self) -> String {
        let params_str = if self.parameters.is_empty() {
            String::new()
        } else {
            self.parameters
                .iter()
                .map(|p| p.to_scad())
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "function {}({}) = {};",
            self.name,
            params_str,
            self.body.to_scad()
        )
    }
}

/// A module definition in OpenSCAD
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleDefinition {
    /// Module name
    pub name: String,
    /// Module parameters
    pub parameters: Vec<Parameter>,
    /// Module body (statements/children)
    pub body: Vec<ModuleNode>,
}

impl ModuleDefinition {
    /// Create a new module definition
    pub fn new(name: String, parameters: Vec<Parameter>, body: Vec<ModuleNode>) -> Self {
        Self {
            name,
            parameters,
            body,
        }
    }

    /// Generate OpenSCAD code for this module
    pub fn to_scad(&self) -> String {
        let params_str = if self.parameters.is_empty() {
            String::new()
        } else {
            self.parameters
                .iter()
                .map(|p| p.to_scad())
                .collect::<Vec<_>>()
                .join(", ")
        };

        let mut result = format!("module {}({}) {{\n", self.name, params_str);
        for stmt in &self.body {
            result.push_str(&stmt.to_scad(4));
            result.push('\n');
        }
        result.push_str("}\n");
        result
    }
}

/// The root AST structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstRoot {
    /// Global variable declarations
    pub global_variables: Vec<GlobalVariable>,

    /// Module definitions
    pub module_defines: Vec<ModuleDefinition>,

    /// Function definitions
    pub function_defines: Vec<FunctionDefinition>,

    /// All top-level module instantiations
    pub modules: Vec<ModuleNode>,

    /// Included libraries
    pub includes: Vec<String>,

    /// Used libraries
    pub uses: Vec<String>,

    /// Loaded library files (JSON files that were used to load modules)
    pub loaded_libraries: Vec<String>,
}

impl AstRoot {
    /// Create a new empty AST
    pub fn new() -> Self {
        Self {
            global_variables: Vec::new(),
            module_defines: Vec::new(),
            function_defines: Vec::new(),
            modules: Vec::new(),
            includes: Vec::new(),
            uses: Vec::new(),
            loaded_libraries: Vec::new(),
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
        let mut stack: Vec<&'a ModuleNode> = modules.iter().collect();
        while let Some(module) = stack.pop() {
            if module.id == id {
                return Some(module);
            }
            stack.extend(module.children.iter());
        }
        None
    }

    /// Find a mutable node by ID
    pub fn find_node_mut(&mut self, id: &str) -> Option<&mut ModuleNode> {
        Self::find_in_vec_mut(&mut self.modules, id)
    }

    fn find_in_vec_mut<'a>(modules: &'a mut [ModuleNode], id: &str) -> Option<&'a mut ModuleNode> {
        const MAX_DEPTH: usize = 1000;
        fn find_recursive<'a>(
            modules: &'a mut [ModuleNode],
            id: &str,
            depth: usize,
        ) -> Option<&'a mut ModuleNode> {
            if depth >= MAX_DEPTH {
                return None;
            }
            for module in modules {
                if module.id == id {
                    return Some(module);
                }
                if let Some(found) = find_recursive(&mut module.children, id, depth + 1) {
                    return Some(found);
                }
            }
            None
        }
        find_recursive(modules, id, 0)
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
        const MAX_DEPTH: usize = 1000;
        fn insert_recursive(
            modules: &mut [ModuleNode],
            parent_id: &str,
            child: &ModuleNode,
            depth: usize,
        ) -> bool {
            if depth >= MAX_DEPTH {
                return false;
            }
            for module in modules {
                if module.id == parent_id {
                    module.children.push(child.clone());
                    return true;
                }
                if insert_recursive(&mut module.children, parent_id, child, depth + 1) {
                    return true;
                }
            }
            false
        }
        insert_recursive(modules, parent_id, &child, 0)
    }

    /// Delete a node and all its children
    pub fn delete_node(&mut self, id: &str) -> Result<()> {
        Self::delete_node_in_vec(&mut self.modules, id);
        Ok(())
    }

    fn delete_node_in_vec(modules: &mut Vec<ModuleNode>, id: &str) -> bool {
        const MAX_DEPTH: usize = 1000;
        fn delete_recursive(modules: &mut Vec<ModuleNode>, id: &str, depth: usize) -> bool {
            if depth >= MAX_DEPTH {
                return false;
            }
            for i in (0..modules.len()).rev() {
                if modules[i].id == id {
                    modules.remove(i);
                    return true;
                }
            }

            for module in modules {
                if delete_recursive(&mut module.children, id, depth + 1) {
                    return true;
                }
            }

            false
        }
        delete_recursive(modules, id, 0)
    }

    /// Add a function definition
    pub fn add_function_define(&mut self, func_def: FunctionDefinition) -> Result<()> {
        // Check for duplicate function names
        if self
            .function_defines
            .iter()
            .any(|f| f.name == func_def.name)
        {
            return Err(AstError::DuplicateIdentifier(func_def.name.clone()));
        }
        self.function_defines.push(func_def);
        Ok(())
    }

    /// Add a module definition
    pub fn add_module_define(&mut self, module_def: ModuleDefinition) -> Result<()> {
        // Check for duplicate module names
        if self
            .module_defines
            .iter()
            .any(|m| m.name == module_def.name)
        {
            return Err(AstError::DuplicateIdentifier(module_def.name.clone()));
        }
        self.module_defines.push(module_def);
        Ok(())
    }

    /// Find a function definition by name
    pub fn find_function_define(&self, name: &str) -> Option<&FunctionDefinition> {
        self.function_defines.iter().find(|f| f.name == name)
    }

    /// Find a module definition by name
    pub fn find_module_define(&self, name: &str) -> Option<&ModuleDefinition> {
        self.module_defines.iter().find(|m| m.name == name)
    }

    /// Add a global variable
    pub fn add_global_variable(&mut self, var: GlobalVariable) -> Result<()> {
        // Check for duplicate variable names
        if self
            .global_variables
            .iter()
            .any(|v| v.name == var.name && v.is_special == var.is_special)
        {
            return Err(AstError::DuplicateIdentifier(var.name.clone()));
        }
        self.global_variables.push(var);
        Ok(())
    }

    /// Remove a global variable by name
    pub fn remove_global_variable(&mut self, name: &str, is_special: bool) -> Result<()> {
        if let Some(pos) = self
            .global_variables
            .iter()
            .position(|v| v.name == name && v.is_special == is_special)
        {
            self.global_variables.remove(pos);
            Ok(())
        } else {
            Err(AstError::NodeNotFound(name.to_string()))
        }
    }

    /// Find a global variable by name
    pub fn find_global_variable(&self, name: &str, is_special: bool) -> Option<&GlobalVariable> {
        self.global_variables
            .iter()
            .find(|v| v.name == name && v.is_special == is_special)
    }

    /// Find a mutable global variable by name
    pub fn find_global_variable_mut(
        &mut self,
        name: &str,
        is_special: bool,
    ) -> Option<&mut GlobalVariable> {
        self.global_variables
            .iter_mut()
            .find(|v| v.name == name && v.is_special == is_special)
    }

    /// Update a global variable's value
    pub fn update_global_variable(
        &mut self,
        name: &str,
        is_special: bool,
        new_value: Expr,
    ) -> Result<()> {
        if let Some(var) = self.find_global_variable_mut(name, is_special) {
            var.value = new_value;
            Ok(())
        } else {
            Err(AstError::NodeNotFound(name.to_string()))
        }
    }

    /// Get all global variables
    pub fn global_variables(&self) -> &[GlobalVariable] {
        &self.global_variables
    }

    /// Check if a global variable exists
    pub fn has_global_variable(&self, name: &str, is_special: bool) -> bool {
        self.global_variables
            .iter()
            .any(|v| v.name == name && v.is_special == is_special)
    }

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

        // Add global variables
        for var in &self.global_variables {
            result.push_str(&var.to_scad());
            result.push('\n');
        }

        if !self.global_variables.is_empty() {
            result.push('\n');
        }

        // Add function definitions
        for func_def in &self.function_defines {
            result.push_str(&func_def.to_scad());
            result.push('\n');
        }

        if !self.function_defines.is_empty() {
            result.push('\n');
        }

        // Add module definitions
        for module_def in &self.module_defines {
            result.push_str(&module_def.to_scad());
            result.push('\n');
        }

        if !self.module_defines.is_empty() {
            result.push('\n');
        }

        // Add module instantiations
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

/// Helper function to parse binary operations in expressions using precedence-based parsing
fn parse_binary_operation(input: &str) -> Option<Result<Expr>> {
    // Tokenize and parse using precedence climbing algorithm
    let trimmed = input.trim();

    // First, find all operators that are not inside parentheses, brackets, or quotes
    let mut ops_found = Vec::new();

    let mut paren_depth = 0;
    let mut bracket_depth = 0;
    let mut brace_depth = 0;
    let mut in_string = false;
    let mut escape_next = false;

    let chars: Vec<(usize, char)> = trimmed.char_indices().collect();

    #[allow(clippy::needless_range_loop)]
    for i in 0..chars.len() {
        let (pos, ch) = chars[i];

        if escape_next {
            escape_next = false;
            continue;
        }

        match ch {
            '\\' => escape_next = true,
            '"' => in_string = !in_string,
            '(' if !in_string => paren_depth += 1,
            ')' if !in_string => {
                paren_depth -= 1;
                if paren_depth < 0 {
                    // Unmatched parenthesis
                    return None;
                }
            }
            '[' if !in_string => bracket_depth += 1,
            ']' if !in_string => bracket_depth -= 1,
            '{' if !in_string => brace_depth += 1,
            '}' if !in_string => brace_depth -= 1,
            _ => {}
        }

        // Only look for operators when not inside brackets/strings and at outermost level
        if !in_string && paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 {
            // Check if any operator starts at this position
            let remaining = &trimmed[pos..];

            // Sort operators by length descending to match longest operators first (e.g., "==" before "=")
            let mut sorted_ops = vec![
                "||", "&&", "==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "/", "%", "^",
            ];
            sorted_ops.sort_by_key(|b| std::cmp::Reverse(b.len()));

            for op_str in &sorted_ops {
                if remaining.starts_with(op_str) {
                    // Found an operator at this position
                    ops_found.push((pos, *op_str));
                    break; // Only record the longest matching operator
                }
            }
        }
    }

    // If no operators found at top level, return None
    if ops_found.is_empty() {
        return None;
    }

    // Find the operator with the LOWEST precedence (since lower precedence binds weaker and cuts at the highest level)
    let mut lowest_prec_op = "";
    let mut lowest_prec_pos = 0;
    let mut lowest_prec = u8::MAX; // Start with max to find minimum

    for (pos, op_str) in ops_found {
        let op = match op_str {
            "+" => BinOp::Add,
            "-" => BinOp::Sub,
            "*" => BinOp::Mul,
            "/" => BinOp::Div,
            "%" => BinOp::Mod,
            "^" => BinOp::Power,
            "||" => BinOp::Or,
            "&&" => BinOp::And,
            "==" => BinOp::Eq,
            "!=" => BinOp::Neq,
            "<=" => BinOp::Lte,
            ">=" => BinOp::Gte,
            "<" => BinOp::Lt,
            ">" => BinOp::Gt,
            _ => continue, // Unknown operator
        };

        let prec = op.precedence();
        if prec < lowest_prec {
            lowest_prec = prec;
            lowest_prec_op = op_str;
            lowest_prec_pos = pos;
        }
    }

    // If no valid operator found, return None
    if lowest_prec_op.is_empty() {
        return None;
    }

    // Split the expression at the lowest precedence operator
    let left_str = &trimmed[..lowest_prec_pos].trim();
    let right_str = &trimmed[lowest_prec_pos + lowest_prec_op.len()..].trim();

    if left_str.is_empty() || right_str.is_empty() {
        // This might be a unary operator or invalid expression
        return None;
    }

    // Parse left and right sides recursively
    let left_result = Expr::parse(left_str);
    let right_result = Expr::parse(right_str);

    match (left_result, right_result) {
        (Ok(left_expr), Ok(right_expr)) => {
            let op = match lowest_prec_op {
                "+" => BinOp::Add,
                "-" => BinOp::Sub,
                "*" => BinOp::Mul,
                "/" => BinOp::Div,
                "%" => BinOp::Mod,
                "^" => BinOp::Power,
                "||" => BinOp::Or,
                "&&" => BinOp::And,
                "==" => BinOp::Eq,
                "!=" => BinOp::Neq,
                "<=" => BinOp::Lte,
                ">=" => BinOp::Gte,
                "<" => BinOp::Lt,
                ">" => BinOp::Gt,
                _ => unreachable!(), // Should have been caught earlier
            };

            Some(Ok(Expr::BinOp {
                left: Box::new(left_expr),
                op,
                right: Box::new(right_expr),
            }))
        }
        (Err(e), _) => Some(Err(e)),
        (_, Err(e)) => Some(Err(e)),
    }
}

/// Check if parentheses in an expression are balanced
fn is_balanced_parentheses(expr: &str) -> bool {
    let mut paren_count = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for ch in expr.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match ch {
            '\\' => escape_next = true,
            '"' => in_string = !in_string,
            '(' if !in_string => paren_count += 1,
            ')' if !in_string => {
                paren_count -= 1;
                if paren_count < 0 {
                    // Closing parenthesis without matching opening one
                    return false;
                }
            }
            _ => {}
        }
    }

    // Balanced parentheses should have a count of 0
    paren_count == 0
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
        let module = ModuleNode::new_leaf("cube1".to_string(), "cube".to_string(), vec![]);

        ast.add_module(module).unwrap();
        assert_eq!(ast.modules.len(), 1);
    }

    #[test]
    fn test_parameter_to_scad() {
        let param = Parameter::new("size".to_string());
        assert_eq!(param.to_scad(), "size");

        let param_with_default =
            Parameter::with_default("center".to_string(), Expr::Boolean(false));
        assert_eq!(param_with_default.to_scad(), "center=false");
    }

    #[test]
    fn test_assignment_to_scad() {
        let assignment = Assignment::new("x".to_string(), Expr::Integer(10));
        assert_eq!(assignment.to_scad(), "x = 10");
    }

    #[test]
    fn test_global_variable_regular() {
        let var = GlobalVariable::new("width".to_string(), Expr::Integer(100));
        assert_eq!(var.to_scad(), "width = 100;");
        assert!(!var.is_special);
    }

    #[test]
    fn test_global_variable_special() {
        let var = GlobalVariable::new_special("fn".to_string(), Expr::Integer(50));
        assert_eq!(var.to_scad(), "$fn = 50;");
        assert!(var.is_special);
    }

    #[test]
    fn test_global_variable_complex_expression() {
        let var = GlobalVariable::new(
            "size".to_string(),
            Expr::List(vec![
                Expr::Integer(10),
                Expr::Integer(20),
                Expr::Integer(30),
            ]),
        );
        assert!(var.to_scad().contains("size = [10, 20, 30];"));
    }

    #[test]
    fn test_function_definition() {
        let func = FunctionDefinition::new(
            "add".to_string(),
            vec![
                Parameter::new("a".to_string()),
                Parameter::new("b".to_string()),
            ],
            Expr::BinOp {
                left: Box::new(Expr::Identifier("a".to_string())),
                op: BinOp::Add,
                right: Box::new(Expr::Identifier("b".to_string())),
            },
        );

        let scad = func.to_scad();
        assert!(scad.contains("function add"));
        assert!(scad.contains("a,"));
        assert!(scad.contains("b)"));
        assert!(scad.contains("a + b"));
    }

    #[test]
    fn test_module_definition() {
        let module_def = ModuleDefinition::new(
            "my_cube".to_string(),
            vec![Parameter::with_default(
                "size".to_string(),
                Expr::Integer(10),
            )],
            vec![ModuleNode::new_leaf(
                "cube1".to_string(),
                "cube".to_string(),
                vec![],
            )],
        );

        let scad = module_def.to_scad();
        assert!(scad.contains("module my_cube"));
        assert!(scad.contains("size=10"));
        assert!(scad.contains("cube"));
    }

    #[test]
    fn test_ast_root_with_definitions() {
        let mut ast = AstRoot::new();

        let func_def = FunctionDefinition::new(
            "double".to_string(),
            vec![Parameter::new("x".to_string())],
            Expr::BinOp {
                left: Box::new(Expr::Identifier("x".to_string())),
                op: BinOp::Mul,
                right: Box::new(Expr::Integer(2)),
            },
        );

        ast.add_function_define(func_def).unwrap();
        assert_eq!(ast.function_defines.len(), 1);
        assert!(ast.find_function_define("double").is_some());

        let module_def = ModuleDefinition::new("test_module".to_string(), vec![], vec![]);

        ast.add_module_define(module_def).unwrap();
        assert_eq!(ast.module_defines.len(), 1);
        assert!(ast.find_module_define("test_module").is_some());
    }

    #[test]
    fn test_ast_root_complete_code_generation() {
        let mut ast = AstRoot::new();

        // Add includes
        ast.includes.push("lib.scad".to_string());

        // Add function definition
        ast.add_function_define(FunctionDefinition::new(
            "get_size".to_string(),
            vec![],
            Expr::Integer(10),
        ))
        .unwrap();

        // Add module definition
        ast.add_module_define(ModuleDefinition::new(
            "my_shape".to_string(),
            vec![],
            vec![ModuleNode::new_leaf(
                "m1".to_string(),
                "cube".to_string(),
                vec![],
            )],
        ))
        .unwrap();

        // Add module instantiation
        ast.add_module(ModuleNode::new_leaf(
            "instance1".to_string(),
            "my_shape".to_string(),
            vec![],
        ))
        .unwrap();

        let scad = ast.to_scad();
        assert!(scad.contains("include <lib.scad>"));
        assert!(scad.contains("function get_size"));
        assert!(scad.contains("module my_shape"));
        assert!(scad.contains("my_shape()"));
    }

    #[test]
    fn test_ast_root_with_global_variables() {
        let mut ast = AstRoot::new();

        // Add regular global variable
        ast.add_global_variable(GlobalVariable::new("width".to_string(), Expr::Integer(100)))
            .unwrap();

        // Add special global variable
        ast.add_global_variable(GlobalVariable::new_special(
            "fn".to_string(),
            Expr::Integer(50),
        ))
        .unwrap();

        assert_eq!(ast.global_variables.len(), 2);
        assert!(ast.find_global_variable("width", false).is_some());
        assert!(ast.find_global_variable("fn", true).is_some());
        assert!(ast.has_global_variable("width", false));
        assert!(!ast.has_global_variable("width", true));
    }

    #[test]
    fn test_global_variable_duplicate_check() {
        let mut ast = AstRoot::new();

        ast.add_global_variable(GlobalVariable::new("size".to_string(), Expr::Integer(10)))
            .unwrap();

        // Try to add duplicate
        let result =
            ast.add_global_variable(GlobalVariable::new("size".to_string(), Expr::Integer(20)));

        assert!(result.is_err());
        assert_eq!(ast.global_variables.len(), 1);
    }

    #[test]
    fn test_global_variable_update() {
        let mut ast = AstRoot::new();

        ast.add_global_variable(GlobalVariable::new("size".to_string(), Expr::Integer(10)))
            .unwrap();

        // Update the variable
        ast.update_global_variable("size", false, Expr::Integer(20))
            .unwrap();

        let var = ast.find_global_variable("size", false).unwrap();
        assert_eq!(var.value, Expr::Integer(20));
    }

    #[test]
    fn test_global_variable_remove() {
        let mut ast = AstRoot::new();

        ast.add_global_variable(GlobalVariable::new("size".to_string(), Expr::Integer(10)))
            .unwrap();

        assert_eq!(ast.global_variables.len(), 1);

        // Remove the variable
        ast.remove_global_variable("size", false).unwrap();
        assert_eq!(ast.global_variables.len(), 0);
    }

    #[test]
    fn test_ast_root_with_all_components() {
        let mut ast = AstRoot::new();

        // Add includes
        ast.includes.push("lib.scad".to_string());

        // Add global variable
        ast.add_global_variable(GlobalVariable::new_special(
            "fn".to_string(),
            Expr::Integer(32),
        ))
        .unwrap();

        // Add function definition
        ast.add_function_define(FunctionDefinition::new(
            "double".to_string(),
            vec![Parameter::new("x".to_string())],
            Expr::BinOp {
                left: Box::new(Expr::Identifier("x".to_string())),
                op: BinOp::Mul,
                right: Box::new(Expr::Integer(2)),
            },
        ))
        .unwrap();

        // Add module definition
        ast.add_module_define(ModuleDefinition::new(
            "cube_holder".to_string(),
            vec![],
            vec![ModuleNode::new_leaf(
                "c1".to_string(),
                "cube".to_string(),
                vec![],
            )],
        ))
        .unwrap();

        // Add module instantiation
        ast.add_module(ModuleNode::new_leaf(
            "m1".to_string(),
            "cube_holder".to_string(),
            vec![],
        ))
        .unwrap();

        let scad = ast.to_scad();

        // Check order: includes -> global vars -> functions -> modules -> instantiations
        let includes_pos = scad.find("include").unwrap();
        let vars_pos = scad.find("$fn").unwrap();
        let func_pos = scad.find("function double").unwrap();
        let module_pos = scad.find("module cube_holder").unwrap();
        let inst_pos = scad.find("cube_holder()").unwrap();

        assert!(includes_pos < vars_pos);
        assert!(vars_pos < func_pos);
        assert!(func_pos < module_pos);
        assert!(module_pos < inst_pos);
    }

    #[test]
    fn test_expr_parse_parentheses_simple() {
        let result = Expr::parse("(a+b)").unwrap();
        let expected = Expr::BinOp {
            left: Box::new(Expr::Identifier("a".to_string())),
            op: BinOp::Add,
            right: Box::new(Expr::Identifier("b".to_string())),
        };
        assert_eq!(result, expected);
    }

    #[test]
    fn test_expr_parse_nested_parentheses() {
        let result = Expr::parse("((a+b)*c)").unwrap();
        let expected = Expr::BinOp {
            left: Box::new(Expr::BinOp {
                left: Box::new(Expr::Identifier("a".to_string())),
                op: BinOp::Add,
                right: Box::new(Expr::Identifier("b".to_string())),
            }),
            op: BinOp::Mul,
            right: Box::new(Expr::Identifier("c".to_string())),
        };
        assert_eq!(result, expected);
    }

    #[test]
    fn test_is_balanced_parentheses() {
        assert!(is_balanced_parentheses("(a+b)"));
        assert!(is_balanced_parentheses("((a+b)*c)"));
        assert!(is_balanced_parentheses("(((a)))"));
        assert!(is_balanced_parentheses("a+b")); // No parentheses is balanced
        assert!(!is_balanced_parentheses("((a+b)")); // Unbalanced
        assert!(!is_balanced_parentheses("(a+b))")); // Unbalanced
    }

    #[test]
    fn test_expr_to_scad_with_parentheses_preservation() {
        // Test that binary operations always get parentheses now (redundant but safe)
        let expr = Expr::BinOp {
            left: Box::new(Expr::Identifier("a".to_string())),
            op: BinOp::Add,
            right: Box::new(Expr::Identifier("b".to_string())),
        };
        assert_eq!(expr.to_scad(), "(a + b)");

        // Test that nested operations get parentheses
        let nested_expr = Expr::BinOp {
            left: Box::new(Expr::BinOp {
                left: Box::new(Expr::Identifier("a".to_string())),
                op: BinOp::Add,
                right: Box::new(Expr::Identifier("b".to_string())),
            }),
            op: BinOp::Mul,
            right: Box::new(Expr::Identifier("c".to_string())),
        };
        assert_eq!(nested_expr.to_scad(), "((a + b) * c)");

        // Test the reverse: addition with multiplication operands
        let nested_expr2 = Expr::BinOp {
            left: Box::new(Expr::BinOp {
                left: Box::new(Expr::Identifier("a".to_string())),
                op: BinOp::Mul,
                right: Box::new(Expr::Identifier("b".to_string())),
            }),
            op: BinOp::Add,
            right: Box::new(Expr::Identifier("c".to_string())),
        };
        assert_eq!(nested_expr2.to_scad(), "((a * b) + c)");

        // Test division with addition
        let div_expr = Expr::BinOp {
            left: Box::new(Expr::BinOp {
                left: Box::new(Expr::Identifier("a".to_string())),
                op: BinOp::Add,
                right: Box::new(Expr::Identifier("b".to_string())),
            }),
            op: BinOp::Div,
            right: Box::new(Expr::Integer(2)),
        };
        assert_eq!(div_expr.to_scad(), "((a + b) / 2)");

        // Test power operation
        let pow_expr = Expr::BinOp {
            left: Box::new(Expr::Identifier("a".to_string())),
            op: BinOp::Power,
            right: Box::new(Expr::BinOp {
                left: Box::new(Expr::Identifier("b".to_string())),
                op: BinOp::Add,
                right: Box::new(Expr::Identifier("c".to_string())),
            }),
        };
        assert_eq!(pow_expr.to_scad(), "(a ^ (b + c))");
    }

    #[test]
    fn test_operator_precedence_values() {
        // Verify that precedence values are assigned correctly
        assert!(BinOp::Power.precedence() > BinOp::Mul.precedence());
        assert!(BinOp::Mul.precedence() > BinOp::Add.precedence());
        assert!(BinOp::Add.precedence() > BinOp::Gt.precedence());
        assert!(BinOp::Gt.precedence() > BinOp::Eq.precedence());
        assert!(BinOp::Eq.precedence() > BinOp::And.precedence());
        assert_eq!(BinOp::And.precedence(), BinOp::Or.precedence()); // Same precedence
    }

    #[test]
    fn test_operator_precedence_parsing() {
        // Test that multiplication/division has higher precedence than addition/subtraction
        let expr = Expr::parse("a + b * c").unwrap();
        // Should be parsed as a + (b * c), not (a + b) * c
        match expr {
            Expr::BinOp {
                left,
                op: BinOp::Add,
                right,
            } => {
                // Left should be 'a', right should be 'b * c'
                match right.as_ref() {
                    Expr::BinOp { op: BinOp::Mul, .. } => {
                        // Correct: b * c is grouped together
                        assert!(matches!(left.as_ref(), Expr::Identifier(_)));
                    }
                    _ => panic!("Expected multiplication to have higher precedence"),
                }
            }
            _ => panic!("Expected addition with multiplication on right side"),
        }

        // Test division vs addition
        let expr = Expr::parse("a + b / c").unwrap();
        match expr {
            Expr::BinOp {
                left,
                op: BinOp::Add,
                right,
            } => {
                match right.as_ref() {
                    Expr::BinOp { op: BinOp::Div, .. } => {
                        // Correct: b / c is grouped together
                        assert!(matches!(left.as_ref(), Expr::Identifier(_)));
                    }
                    _ => panic!("Expected division to have higher precedence than addition"),
                }
            }
            _ => panic!("Expected addition with division on right side"),
        }
    }
}
