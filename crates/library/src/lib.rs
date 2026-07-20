//! OpenSCAD Library Management
//!
//! This module provides library loading and module discovery functionality.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Parameter definition for a module
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterDef {
    /// Parameter name
    pub name: String,

    /// Parameter type (integer, float, string, list, etc.)
    pub param_type: String,

    /// Default value if any
    pub default: Option<String>,

    /// Description
    pub description: Option<String>,
}

/// Module definition in a library
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleDef {
    /// Module name
    pub name: String,

    /// Module description
    pub description: Option<String>,

    /// Parameters
    pub parameters: Vec<ParameterDef>,

    /// Whether this module accepts children
    pub accepts_children: bool,
}

impl ModuleDef {
    /// Generate a parameter hint string for display
    pub fn get_param_hint(&self) -> String {
        if self.parameters.is_empty() {
            return String::new();
        }

        let params = self
            .parameters
            .iter()
            .map(|p| {
                let param_str = format!("{}: {}", p.name, p.param_type);
                if let Some(ref default) = p.default {
                    format!("{} = {}", param_str, default)
                } else {
                    param_str
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        format!("({})", params)
    }
}

/// Function definition in a library
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    /// Function name
    pub name: String,

    /// Function description
    pub description: Option<String>,

    /// Parameters
    pub parameters: Vec<ParameterDef>,

    /// Return type
    pub return_type: String,
}

impl FunctionDef {
    /// Generate a parameter hint string for display
    pub fn get_param_hint(&self) -> String {
        if self.parameters.is_empty() {
            return format!("{}() -> {}", self.name, self.return_type);
        }

        let params = self
            .parameters
            .iter()
            .map(|p| {
                let param_str = format!("{}: {}", p.name, p.param_type);
                if let Some(ref default) = p.default {
                    format!("{} = {}", param_str, default)
                } else {
                    param_str
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        format!("{}({}) -> {}", self.name, params, self.return_type)
    }
}

#[derive(Debug, Deserialize)]
struct BuiltinCatalog {
    modules: Vec<ModuleDef>,
    functions: Vec<FunctionDef>,
}

/// Library manager that handles loading and discovering modules
pub struct LibraryManager {
    /// Built-in modules
    builtin_modules: HashMap<String, ModuleDef>,

    /// Built-in functions
    builtin_functions: HashMap<String, FunctionDef>,

    /// User-defined custom modules
    custom_modules: HashMap<String, ModuleDef>,

    /// User-defined custom functions
    custom_functions: HashMap<String, FunctionDef>,
}

impl LibraryManager {
    /// Create a new library manager with built-in modules
    pub fn new() -> Self {
        let mut manager = Self {
            builtin_modules: HashMap::new(),
            builtin_functions: HashMap::new(),
            custom_modules: HashMap::new(),
            custom_functions: HashMap::new(),
        };

        manager.load_embedded_builtins();

        manager
    }

    /// Get a module definition by name
    pub fn get_module(&self, name: &str) -> Option<ModuleDef> {
        self.custom_modules
            .get(name)
            .cloned()
            .or_else(|| self.builtin_modules.get(name).cloned())
    }

    /// Get all available module names
    pub fn get_module_names(&self) -> Vec<String> {
        let mut names = std::collections::HashSet::new();

        // Add builtin modules
        names.extend(self.builtin_modules.keys().cloned());

        // Add custom modules
        names.extend(self.custom_modules.keys().cloned());

        names.into_iter().collect()
    }

    /// Add a user-defined custom module
    pub fn add_custom_module(&mut self, module: ModuleDef) {
        self.custom_modules.insert(module.name.clone(), module);
    }

    /// Get a function definition by name (优先级：自定义 > 内置 > 第三方库)
    pub fn get_function(&self, name: &str) -> Option<FunctionDef> {
        self.custom_functions
            .get(name)
            .cloned()
            .or_else(|| self.builtin_functions.get(name).cloned())
    }

    /// Get all available function names
    pub fn get_function_names(&self) -> Vec<String> {
        let mut names = std::collections::HashSet::new();

        names.extend(self.builtin_functions.keys().cloned());
        names.extend(self.custom_functions.keys().cloned());

        names.into_iter().collect()
    }

    /// Add a user-defined custom function
    pub fn add_custom_function(&mut self, function: FunctionDef) {
        self.custom_functions
            .insert(function.name.clone(), function);
    }

    /// Get all available functions (自定义 + 内置 + 第三方库)
    pub fn get_all_functions(&self) -> Vec<FunctionDef> {
        let mut functions: Vec<_> = self.custom_functions.values().cloned().collect();

        for func in self.builtin_functions.values() {
            if !functions.iter().any(|f| f.name == func.name) {
                functions.push(func.clone());
            }
        }

        functions.sort_by(|a, b| a.name.cmp(&b.name));
        functions
    }

    /// Reload custom functions from AST function definitions
    pub fn reload_custom_functions_from_ast(
        &mut self,
        function_defines: &[openscad_core::FunctionDefinition],
    ) {
        self.custom_functions.clear();

        for func_def in function_defines {
            let params: Vec<ParameterDef> = func_def
                .parameters
                .iter()
                .map(|p| ParameterDef {
                    name: p.name.clone(),
                    param_type: "any".to_string(),
                    default: None,
                    description: None,
                })
                .collect();

            let function = FunctionDef {
                name: func_def.name.clone(),
                description: Some(format!("User-defined function: {}", func_def.name)),
                parameters: params,
                return_type: "any".to_string(),
            };

            self.custom_functions
                .insert(func_def.name.clone(), function);
        }
    }

    /// Check if module body contains a children module
    fn contains_children_module(modules: &[openscad_core::ModuleNode]) -> bool {
        for module in modules {
            if module.name == "children" {
                return true;
            }
            if Self::contains_children_module(&module.children) {
                return true;
            }
        }
        false
    }

    /// Reload custom modules from AST module definitions
    pub fn reload_custom_modules_from_ast(
        &mut self,
        module_defines: &[openscad_core::ModuleDefinition],
    ) {
        self.custom_modules.clear();
        for module_def in module_defines {
            let params: Vec<ParameterDef> = module_def
                .parameters
                .iter()
                .map(|p| ParameterDef {
                    name: p.name.clone(),
                    param_type: "any".to_string(),
                    default: p.default.as_ref().map(|e| e.to_scad()),
                    description: None,
                })
                .collect();
            let accepts_children = Self::contains_children_module(&module_def.body);
            let module = ModuleDef {
                name: module_def.name.clone(),
                description: Some(format!("User-defined module: {}", module_def.name)),
                parameters: params,
                accepts_children, // Custom modules accept children if they contain a children module
            };
            self.custom_modules.insert(module_def.name.clone(), module);
        }
    }

    /// Get all available modules
    pub fn get_all_modules(&self) -> Vec<ModuleDef> {
        let mut modules: Vec<_> = self.custom_modules.values().cloned().collect();

        // Add built-in modules, skipping any already added from custom_modules
        for module in self.builtin_modules.values() {
            if !modules.iter().any(|m| m.name == module.name) {
                modules.push(module.clone());
            }
        }

        modules.sort_by(|a, b| a.name.cmp(&b.name));
        modules
    }

    fn load_embedded_builtins(&mut self) {
        const EMBEDDED_STDLIB: &str = include_str!("../../../stdlib.json");
        let catalog: BuiltinCatalog = serde_json::from_str(EMBEDDED_STDLIB)
            .expect("embedded stdlib.json must contain a valid builtin catalog");
        self.builtin_modules = catalog
            .modules
            .into_iter()
            .map(|module| (module.name.clone(), module))
            .collect();
        self.builtin_functions = catalog
            .functions
            .into_iter()
            .map(|function| (function.name.clone(), function))
            .collect();
    }
}

impl Default for LibraryManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_library_manager_builtin() {
        let manager = LibraryManager::new();
        let cube = manager.get_module("cube");
        assert!(cube.is_some());
        assert_eq!(cube.unwrap().name, "cube");
    }

    #[test]
    fn test_get_all_modules() {
        let manager = LibraryManager::new();
        let modules = manager.get_all_modules();
        assert!(!modules.is_empty());
        assert!(modules.iter().any(|m| m.name == "cube"));
        assert!(modules.iter().any(|m| m.name == "sphere"));
    }

    #[test]
    fn test_module_accepts_children() {
        let manager = LibraryManager::new();
        let cube = manager.get_module("cube").unwrap();
        assert!(!cube.accepts_children);

        let translate = manager.get_module("translate").unwrap();
        assert!(translate.accepts_children);
    }

    #[test]
    fn test_get_builtin_function() {
        let manager = LibraryManager::new();
        let sin = manager.get_function("sin");
        assert!(sin.is_some());
        assert_eq!(sin.unwrap().return_type, "number");
    }

    #[test]
    fn test_get_all_functions() {
        let manager = LibraryManager::new();
        let functions = manager.get_all_functions();
        assert!(!functions.is_empty());
        assert!(functions.iter().any(|f| f.name == "sin"));
        assert!(functions.iter().any(|f| f.name == "len"));
    }

    #[test]
    fn test_function_param_hint() {
        let manager = LibraryManager::new();
        let sin = manager.get_function("sin").unwrap();
        let hint = sin.get_param_hint();
        assert!(hint.contains("angle"));
        assert!(hint.contains("-> number"));
    }

    #[test]
    fn test_add_custom_function() {
        let mut manager = LibraryManager::new();
        let custom_func = FunctionDef {
            name: "my_func".to_string(),
            description: Some("Custom function".to_string()),
            parameters: vec![],
            return_type: "number".to_string(),
        };
        manager.add_custom_function(custom_func);

        let func = manager.get_function("my_func");
        assert!(func.is_some());
        assert_eq!(
            func.unwrap().description,
            Some("Custom function".to_string())
        );
    }

    #[test]
    fn test_custom_function_override_stdlib() {
        let mut manager = LibraryManager::new();
        let custom_sin = FunctionDef {
            name: "sin".to_string(),
            description: Some("Override sin".to_string()),
            parameters: vec![],
            return_type: "number".to_string(),
        };
        manager.add_custom_function(custom_sin);

        let sin = manager.get_function("sin");
        assert!(sin.is_some());
        assert_eq!(sin.unwrap().description, Some("Override sin".to_string()));
    }

    #[test]
    fn test_reload_custom_functions_from_ast() {
        use openscad_core::{Expr, FunctionDefinition, Parameter};

        let mut manager = LibraryManager::new();
        let func_def = FunctionDefinition::new(
            "my_func".to_string(),
            vec![Parameter::new("x".to_string())],
            Expr::Integer(0),
        );

        manager.reload_custom_functions_from_ast(&[func_def]);

        let func = manager.get_function("my_func");
        assert!(func.is_some());
        assert_eq!(func.unwrap().name, "my_func");
    }
}
