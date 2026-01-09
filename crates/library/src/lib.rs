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
        
        let params = self.parameters.iter()
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
}

impl LibraryManager {
    /// Create a new library manager with built-in modules
    /// Create a new library manager
    /// 
    /// Standard library modules are NOT loaded here.
    /// They are loaded later by load_stdlib_with_config() which:
    /// 1. Tries to load from user config dir (~/.config/openscad-tui/stdlib.json)
    /// 2. Falls back to embedded stdlib.json if user config doesn't exist
    pub fn new() -> Self {
        Self {
            builtin_modules: HashMap::new(),
            libraries: HashMap::new(),
        }
    }
    
    /// Initialize built-in OpenSCAD modules
    /// 
    /// DEPRECATED: This is no longer called during initialization.
    /// Standard library is loaded from stdlib.json instead via load_stdlib_with_config().
    /// Kept for reference and potential future use.
    #[allow(dead_code)]
    fn init_builtin_modules(&mut self) {
        // 3D Primitives
        self.add_builtin_module(ModuleDef {
            name: "cube".to_string(),
            description: Some("Creates a cube with specified dimensions".to_string()),
            parameters: vec![
                ParameterDef {
                    name: "size".to_string(),
                    param_type: "list".to_string(),
                    default: Some("[1, 1, 1]".to_string()),
                    description: Some("[x, y, z] dimensions".to_string()),
                },
                ParameterDef {
                    name: "center".to_string(),
                    param_type: "boolean".to_string(),
                    default: Some("false".to_string()),
                    description: Some("Center the cube at origin".to_string()),
                },
            ],
            accepts_children: false,
        });
        
        self.add_builtin_module(ModuleDef {
            name: "sphere".to_string(),
            description: Some("Creates a sphere with specified radius".to_string()),
            parameters: vec![
                ParameterDef {
                    name: "r".to_string(),
                    param_type: "float".to_string(),
                    default: Some("1".to_string()),
                    description: Some("Radius of sphere".to_string()),
                },
                ParameterDef {
                    name: "$fn".to_string(),
                    param_type: "integer".to_string(),
                    default: None,
                    description: Some("Number of fragments for rendering".to_string()),
                },
            ],
            accepts_children: false,
        });
        
        self.add_builtin_module(ModuleDef {
            name: "cylinder".to_string(),
            description: Some("Creates a cylinder".to_string()),
            parameters: vec![
                ParameterDef {
                    name: "h".to_string(),
                    param_type: "float".to_string(),
                    default: Some("1".to_string()),
                    description: Some("Height".to_string()),
                },
                ParameterDef {
                    name: "r".to_string(),
                    param_type: "float".to_string(),
                    default: Some("1".to_string()),
                    description: Some("Radius".to_string()),
                },
                ParameterDef {
                    name: "center".to_string(),
                    param_type: "boolean".to_string(),
                    default: Some("false".to_string()),
                    description: None,
                },
            ],
            accepts_children: false,
        });
        
        // 2D Primitives
        self.add_builtin_module(ModuleDef {
            name: "square".to_string(),
            description: Some("Creates a 2D square".to_string()),
            parameters: vec![
                ParameterDef {
                    name: "size".to_string(),
                    param_type: "list|float".to_string(),
                    default: Some("1".to_string()),
                    description: Some("[x, y] or single size".to_string()),
                },
                ParameterDef {
                    name: "center".to_string(),
                    param_type: "boolean".to_string(),
                    default: Some("false".to_string()),
                    description: None,
                },
            ],
            accepts_children: false,
        });
        
        self.add_builtin_module(ModuleDef {
            name: "circle".to_string(),
            description: Some("Creates a 2D circle".to_string()),
            parameters: vec![
                ParameterDef {
                    name: "r".to_string(),
                    param_type: "float".to_string(),
                    default: Some("1".to_string()),
                    description: Some("Radius".to_string()),
                },
            ],
            accepts_children: false,
        });
        
        // Transformations
        self.add_builtin_module(ModuleDef {
            name: "translate".to_string(),
            description: Some("Translates children by a vector".to_string()),
            parameters: vec![
                ParameterDef {
                    name: "v".to_string(),
                    param_type: "list".to_string(),
                    default: Some("[0, 0, 0]".to_string()),
                    description: Some("[x, y, z] translation vector".to_string()),
                },
            ],
            accepts_children: true,
        });
        
        self.add_builtin_module(ModuleDef {
            name: "rotate".to_string(),
            description: Some("Rotates children".to_string()),
            parameters: vec![
                ParameterDef {
                    name: "a".to_string(),
                    param_type: "list|float".to_string(),
                    default: Some("[0, 0, 0]".to_string()),
                    description: Some("Rotation angles in degrees".to_string()),
                },
                ParameterDef {
                    name: "v".to_string(),
                    param_type: "list".to_string(),
                    default: None,
                    description: Some("Rotation axis".to_string()),
                },
            ],
            accepts_children: true,
        });
        
        self.add_builtin_module(ModuleDef {
            name: "scale".to_string(),
            description: Some("Scales children".to_string()),
            parameters: vec![
                ParameterDef {
                    name: "v".to_string(),
                    param_type: "list|float".to_string(),
                    default: Some("[1, 1, 1]".to_string()),
                    description: Some("Scale factors".to_string()),
                },
            ],
            accepts_children: true,
        });
        
        self.add_builtin_module(ModuleDef {
            name: "mirror".to_string(),
            description: Some("Mirrors children across a plane".to_string()),
            parameters: vec![
                ParameterDef {
                    name: "v".to_string(),
                    param_type: "list".to_string(),
                    default: Some("[0, 0, 1]".to_string()),
                    description: Some("Mirror plane normal".to_string()),
                },
            ],
            accepts_children: true,
        });
        
        // Boolean Operations
        self.add_builtin_module(ModuleDef {
            name: "union".to_string(),
            description: Some("Union of children".to_string()),
            parameters: vec![],
            accepts_children: true,
        });
        
        self.add_builtin_module(ModuleDef {
            name: "difference".to_string(),
            description: Some("Difference of children (first - rest)".to_string()),
            parameters: vec![],
            accepts_children: true,
        });
        
        self.add_builtin_module(ModuleDef {
            name: "intersection".to_string(),
            description: Some("Intersection of children".to_string()),
            parameters: vec![],
            accepts_children: true,
        });
        
        // Other operations
        self.add_builtin_module(ModuleDef {
            name: "hull".to_string(),
            description: Some("Convex hull of children".to_string()),
            parameters: vec![],
            accepts_children: true,
        });
        
        self.add_builtin_module(ModuleDef {
            name: "minkowski".to_string(),
            description: Some("Minkowski sum of children".to_string()),
            parameters: vec![],
            accepts_children: true,
        });
        
        self.add_builtin_module(ModuleDef {
            name: "render".to_string(),
            description: Some("Render the subtree".to_string()),
            parameters: vec![],
            accepts_children: true,
        });
    }
    
    /// Add a built-in module
    /// 
    /// DEPRECATED: Kept for reference only, use load_library_from_string() instead.
    #[allow(dead_code)]
    fn add_builtin_module(&mut self, module: ModuleDef) {
        self.builtin_modules.insert(module.name.clone(), module);
    }
    
    /// Get a module definition by name
    pub fn get_module(&self, name: &str) -> Option<ModuleDef> {
        self.builtin_modules
            .get(name)
            .cloned()
            .or_else(|| self.get_module_from_libraries(name))
    }
    
    /// Get module source information
    /// Returns (library_name, library_file) for third-party modules
    /// Returns (None, None) for built-in modules
    pub fn get_module_source(&self, name: &str) -> (Option<String>, Option<String>) {
        // Check if it's a built-in module
        if self.builtin_modules.contains_key(name) {
            return (None, None);
        }
        
        // Check in loaded libraries
        for lib in self.libraries.values() {
            if lib.modules.iter().any(|m| m.name == name) {
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
        let mut modules: Vec<_> = self.builtin_modules.values().cloned().collect();
        
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
        dirs::config_dir().map(|config_dir| {
            config_dir.join("openscad-tui").join("stdlib.json")
        })
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
                        eprintln!("Warning: Failed to load stdlib from {:?}: {}", config_path, e);
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
