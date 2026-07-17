//! Command registry for managing command definitions and aliases

use crate::app::App;
use crate::commands::CommandResult;
use std::collections::HashMap;

/// Command type - used for dynamic completion context analysis
#[derive(Debug, Clone, PartialEq)]
pub enum CommandType {
    /// File command: next argument is file path (write, edit, library, export)
    File,
    /// Module command: next argument is module name (insert)
    Module,
    /// Parameter command: command itself is a module, args are module parameters (translate, rotate, scale)
    Param,
    /// No-argument command: command requires no further arguments (difference, union, etc.)
    NoArg,
    FunctionDefinition,
    ModuleDefinition,
    GlobalDefinition,
    /// Replace command: optional source node ID followed by a module name.
    Replace,
    /// Change a parameter on the selected or current module node.
    NodeParam,
    /// Remove a parameter without completing a value assignment.
    NodeParamUnset,
    Preview,
    Camera,
}

/// Command type - used for dynamic completion context analysis
/// Command handler function signature
pub type CommandHandler = fn(&mut App, &[&str]) -> CommandResult<()>;

/// Command definition structure
pub struct CommandDef {
    /// Primary command name
    pub name: String,
    /// Aliases for the command
    pub aliases: Vec<String>,
    /// Function to execute the command
    pub handler: CommandHandler,
    /// Description of what the command does
    #[allow(dead_code)]
    pub description: String,
    /// Minimum number of arguments required
    pub min_args: usize,
    /// Maximum number of arguments allowed (None for unlimited)
    pub max_args: Option<usize>,
    /// Usage example
    #[allow(dead_code)]
    pub usage: String,
    /// Detailed examples
    #[allow(dead_code)]
    pub examples: Vec<String>,
    /// Command type for dynamic completion context analysis
    pub cmd_type: CommandType,
    /// 是否修改 Ast 语法树
    pub change_ast: bool,
    /// 是否写入历史记录
    pub write_to_history: bool,
}

impl CommandDef {
    /// Create a new command definition
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: impl Into<String>,
        aliases: Vec<impl Into<String>>,
        handler: CommandHandler,
        description: impl Into<String>,
        min_args: usize,
        max_args: Option<usize>,
        usage: impl Into<String>,
        examples: Vec<impl Into<String>>,
        cmd_type: crate::command_registry::CommandType,
        change_ast: bool,
        write_to_history: bool,
    ) -> Self {
        Self {
            name: name.into(),
            aliases: aliases.into_iter().map(|s| s.into()).collect(),
            handler,
            description: description.into(),
            min_args,
            max_args,
            usage: usage.into(),
            examples: examples.into_iter().map(|s| s.into()).collect(),
            cmd_type,
            change_ast,
            write_to_history,
        }
    }

    /// Get all names for this command (including aliases)
    pub fn get_all_names(&self) -> Vec<String> {
        let mut names = vec![self.name.clone()];
        names.extend(self.aliases.clone());
        names
    }

    /// Check if a given name matches this command
    #[allow(dead_code)]
    pub fn matches_name(&self, name: &str) -> bool {
        self.name == name || self.aliases.iter().any(|alias| alias == name)
    }

    /// Check if this command is of a specific type
    #[allow(dead_code)]
    pub fn is_type(&self, cmd_type: &CommandType) -> bool {
        &self.cmd_type == cmd_type
    }
}

/// Registry for managing all commands
pub struct CommandRegistry {
    /// Map from primary name to command definition
    commands: HashMap<String, CommandDef>,
    /// Map from alias to primary name
    alias_map: HashMap<String, String>,
}

impl CommandRegistry {
    /// Create a new empty command registry
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
            alias_map: HashMap::new(),
        }
    }

    /// Register a new command
    pub fn register(&mut self, def: CommandDef) {
        let primary_name = def.name.clone();

        // Register primary name
        self.commands.insert(primary_name.clone(), def);

        // Register aliases
        for alias in self.commands.get(&primary_name).unwrap().aliases.iter() {
            self.alias_map.insert(alias.clone(), primary_name.clone());
        }
    }

    /// Find a command by name (including aliases)
    pub fn find(&self, name: &str) -> Option<&CommandDef> {
        if let Some(def) = self.commands.get(name) {
            Some(def)
        } else if let Some(primary_name) = self.alias_map.get(name) {
            self.commands.get(primary_name)
        } else {
            None
        }
    }

    /// Get all command names (including aliases) for autocomplete
    pub fn get_all_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();

        // Add all primary names and aliases
        for def in self.commands.values() {
            names.extend(def.get_all_names());
        }

        names.sort();
        names.dedup();
        names
    }

    /// Get all primary command names
    #[allow(dead_code)]
    pub fn get_primary_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.commands.keys().cloned().collect();
        names.sort();
        names
    }

    /// Check if a command exists
    #[allow(dead_code)]
    pub fn exists(&self, name: &str) -> bool {
        self.commands.contains_key(name) || self.alias_map.contains_key(name)
    }

    /// Get number of registered commands
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Check if registry is empty
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}
