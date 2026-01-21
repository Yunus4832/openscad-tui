//! OpenSCAD Library Management
//!
//! This module provides library loading and module discovery functionality.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum LibraryError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON parsing error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Library not found: {0}")]
    LibraryNotFound(String),

    #[error("Invalid library definition: {0}")]
    InvalidDefinition(String),
}

pub type Result<T> = std::result::Result<T, LibraryError>;

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

/// Library definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryDef {
    /// Library name
    pub name: String,

    /// Library description
    pub description: Option<String>,

    /// OpenSCAD file to include (relative path)
    pub file: String,

    /// All modules in this library
    pub modules: Vec<ModuleDef>,

    /// Version
    pub version: Option<String>,
}

/// Library manager that handles loading and discovering modules
pub struct LibraryManager {
    /// Built-in modules
    builtin_modules: HashMap<String, ModuleDef>,

    /// Loaded libraries
    libraries: HashMap<String, LibraryDef>,

    /// User-defined custom modules
    custom_modules: HashMap<String, ModuleDef>,
}

impl LibraryManager {
    /// Create a new library manager with built-in modules
    pub fn new() -> Self {
        let mut manager = Self {
            builtin_modules: HashMap::new(),
            libraries: HashMap::new(),
            custom_modules: HashMap::new(),
        };

        // Load standard library with fallback to embedded version
        if let Err(e) = manager.load_stdlib_with_config() {
            eprintln!("Warning: Failed to load standard library: {}", e);
        }

        manager
    }

    /// Get a module definition by name
    pub fn get_module(&self, name: &str) -> Option<ModuleDef> {
        self.custom_modules
            .get(name)
            .cloned()
            .or_else(|| self.builtin_modules.get(name).cloned())
            .or_else(|| self.get_module_from_libraries(name))
    }

    /// Get all available module names
    pub fn get_module_names(&self) -> Vec<String> {
        let mut names = std::collections::HashSet::new();

        // Add builtin modules
        names.extend(self.builtin_modules.keys().cloned());

        // Add custom modules
        names.extend(self.custom_modules.keys().cloned());

        // Add modules from loaded libraries
        for library in self.libraries.values() {
            for module in &library.modules {
                names.insert(module.name.clone());
            }
        }

        names.into_iter().collect()
    }

    /// Add a user-defined custom module
    pub fn add_custom_module(&mut self, module: ModuleDef) {
        self.custom_modules.insert(module.name.clone(), module);
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

    /// Get module source information
    /// Returns (library_name, library_file) for third-party modules
    /// Returns (None, None) for built-in modules
    pub fn get_module_source(&self, name: &str) -> (Option<String>, Option<String>) {
        // Check if it's a custom module (treated as built-in)
        if self.custom_modules.contains_key(name) {
            return (None, None);
        }

        // Check if it's a built-in module
        if self.builtin_modules.contains_key(name) {
            return (None, None);
        }

        // Check in loaded libraries
        for lib in self.libraries.values() {
            if lib.modules.iter().any(|m| m.name == name) {
                // StandardLibrary is special - it's built-in, don't generate include for it
                if lib.name == "StandardLibrary" {
                    return (None, None);
                }
                return (Some(lib.name.clone()), Some(lib.file.clone()));
            }
        }

        (None, None)
    }

    /// Get module from loaded libraries
    fn get_module_from_libraries(&self, name: &str) -> Option<ModuleDef> {
        for lib in self.libraries.values() {
            if let Some(module) = lib.modules.iter().find(|m| m.name == name) {
                return Some(module.clone());
            }
        }
        None
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

        // Add library modules, skipping any already added
        for lib in self.libraries.values() {
            for module in &lib.modules {
                if !modules.iter().any(|m| m.name == module.name) {
                    modules.push(module.clone());
                }
            }
        }

        modules.sort_by(|a, b| a.name.cmp(&b.name));
        modules
    }

    /// Load a library from a JSON string
    pub fn load_library_from_string(&mut self, json_str: &str) -> Result<()> {
        let lib_def: LibraryDef = serde_json::from_str(json_str)?;
        self.libraries.insert(lib_def.name.clone(), lib_def);
        Ok(())
    }

    /// Load a library from a JSON file
    pub fn load_library(&mut self, path: &Path) -> Result<()> {
        let contents = fs::read_to_string(path)?;
        let lib_def: LibraryDef = serde_json::from_str(&contents)?;

        self.libraries.insert(lib_def.name.clone(), lib_def);
        Ok(())
    }

    /// Get standard library config path (~/.config/openscad-tui/stdlib.json on Linux/Mac, etc.)
    pub fn get_stdlib_config_path() -> Option<std::path::PathBuf> {
        dirs::config_dir().map(|config_dir| config_dir.join("openscad-tui").join("stdlib.json"))
    }

    /// Try to load stdlib from user config directory, fallback to embedded version
    pub fn load_stdlib_with_config(&mut self) -> Result<()> {
        // Try to load from user config directory
        if let Some(config_path) = Self::get_stdlib_config_path() {
            if config_path.exists() {
                match self.load_library(&config_path) {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        // If user config exists but fails to load, log error but continue
                        eprintln!(
                            "Warning: Failed to load stdlib from {:?}: {}",
                            config_path, e
                        );
                    }
                }
            }
        }

        // Fallback to embedded stdlib.json
        const EMBEDDED_STDLIB: &str = include_str!("../../../stdlib.json");
        self.load_library_from_string(EMBEDDED_STDLIB)
    }

    /// Get a library by name
    pub fn get_library(&self, name: &str) -> Option<&LibraryDef> {
        self.libraries.get(name)
    }

    /// Get all loaded libraries
    pub fn get_all_libraries(&self) -> Vec<&LibraryDef> {
        self.libraries.values().collect()
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
}
