//! Application state

use openscad_core::AstRoot;
use openscad_library::LibraryManager;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Arc;
use tui_tree_widget::TreeState;

use crate::command_registry::{CommandRegistry, CommandType};

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum InputMode {
    /// Command mode - all input is command-based
    Command,
    /// Multi-stage insert - entering parameters for insert command
    InsertEnterParams,
    /// Multi-stage replace - selecting replacement module
    ReplaceSelectModule,
    /// Help modal - displaying help information
    Help,
    /// Legacy modes (no longer used, kept for compatibility)
    Normal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum MessageType {
    /// Information message (normal operation feedback)
    Info,
    /// Error message (something went wrong)
    Error,
    /// Warning message (potential issue)
    Warning,
}

/// Context for tab completion
#[derive(Debug, Clone, PartialEq)]
pub enum CompletionContext {
    /// Command completion (empty input or command prefix)
    Command,
    /// Module name completion (after "insert " or "i ")
    Module,
    /// Module parameter completion (after module name)
    ModuleParam {
        cmd_type: CommandType,
        module_name: String,
        param_index: usize,
    },
    /// Module parameter value completion (after module parameter name)
    ModuleParamValue {
        cmd_type: CommandType,
        module_name: String,
        module_param_name: String,
        value_index: usize,
    },
    /// File name completion (for write/edit/library commands)
    File {
        /// Current path being completed
        current_path: String,
        /// The directory part of the path (before the last /)
        base_dir: String,
        /// The file/part being completed (after the last /)
        partial_name: String,
        /// Whether the path ends with a separator (indicating directory)
        ends_with_separator: bool,
    },
}

pub struct App {
    pub ast: Arc<AstRoot>,
    pub library: LibraryManager,
    pub command_registry: CommandRegistry,
    pub selected_nodes: Vec<String>,
    pub undo_stack: VecDeque<Arc<AstRoot>>,
    pub redo_stack: VecDeque<Arc<AstRoot>>,

    // UI state - Tree navigation (using RefCell for interior mutability)
    pub tree_state: RefCell<TreeState<String>>,
    #[allow(dead_code)]
    pub tree_cursor: usize,
    #[allow(dead_code)]
    pub expanded_nodes: std::collections::HashSet<String>,

    // UI state - Input and display
    pub input_buffer: String,
    pub input_mode: InputMode,
    /// For insert mode: whether to insert after (true) or before (false)
    #[allow(dead_code)]
    pub insert_after: bool,
    /// For insert mode: the selected module name
    pub insert_module_name: Option<String>,
    pub preview_offset: usize,
    pub should_quit: bool,
    pub message: Option<String>,
    pub message_type: MessageType,

    // Tab completion state
    pub completion_candidates: Vec<String>,
    pub completion_index: usize,
    pub completion_context: CompletionContext,
    pub completion_active: bool,
}

impl App {
    pub fn new() -> Self {
        let mut app = Self {
            ast: Arc::new(AstRoot::new()),
            library: LibraryManager::new(),
            command_registry: CommandRegistry::new(),
            selected_nodes: Vec::new(),
            undo_stack: VecDeque::with_capacity(100),
            redo_stack: VecDeque::with_capacity(100),
            tree_state: RefCell::new(TreeState::default()),
            tree_cursor: 0,
            expanded_nodes: std::collections::HashSet::new(),
            input_buffer: String::new(),
            input_mode: InputMode::Normal,
            insert_after: true,
            insert_module_name: None,
            preview_offset: 0,
            should_quit: false,
            message: None,
            message_type: MessageType::Info,
            completion_candidates: Vec::new(),
            completion_index: 0,
            completion_context: CompletionContext::Command,
            completion_active: false,
        };

        // Load standard library (from config dir if exists, otherwise use embedded)
        if let Err(e) = app.library.load_stdlib_with_config() {
            eprintln!("Failed to load standard library: {}", e);
        }

        // Initialize command registry
        crate::commands::init_command_registry(&mut app.command_registry);

        // Initialize tree state: select the first module if it exists
        app.init_tree_selection();
        app
    }

    /// Initialize tree state with first item selected if available
    pub fn init_tree_selection(&mut self) {
        // Select first available section in the tree
        if !self.ast.includes.is_empty() {
            self.tree_state
                .borrow_mut()
                .select(vec!["__includes".to_string()]);
        } else if !self.ast.uses.is_empty() {
            self.tree_state
                .borrow_mut()
                .select(vec!["__uses".to_string()]);
        } else if !self.ast.global_variables.is_empty() {
            self.tree_state
                .borrow_mut()
                .select(vec!["__globals".to_string()]);
        } else if !self.ast.function_defines.is_empty() {
            self.tree_state
                .borrow_mut()
                .select(vec!["__functions".to_string()]);
        } else if !self.ast.module_defines.is_empty() {
            self.tree_state
                .borrow_mut()
                .select(vec!["__moddefs".to_string()]);
        } else if !self.ast.modules.is_empty() {
            self.tree_state
                .borrow_mut()
                .select(vec!["__modules".to_string()]);
        }
    }

    /// Get mutable reference to AST, cloning if necessary (copy-on-write)
    pub fn ast_mut(&mut self) -> &mut AstRoot {
        Arc::make_mut(&mut self.ast)
    }

    /// Restore tree state to a valid position
    /// Called after AST modifications to ensure navigation isn't lost
    pub fn restore_tree_selection(&mut self) {
        let current_selection = self.tree_state.borrow().selected().last().cloned();

        // Check if current selection still exists
        if let Some(ref node_id) = current_selection {
            // Check section headers
            if node_id.starts_with("__") {
                if self.is_valid_section_id(node_id) {
                    return;
                }
            } else if self.ast.find_node_by_id(node_id).is_some() {
                return;
            }
        }

        // Current selection is invalid or empty, select first available section
        self.init_tree_selection();
    }

    /// Check if a section ID is still valid
    fn is_valid_section_id(&self, id: &str) -> bool {
        match id {
            "__includes" => !self.ast.includes.is_empty(),
            "__uses" => !self.ast.uses.is_empty(),
            "__globals" => !self.ast.global_variables.is_empty(),
            "__functions" => !self.ast.function_defines.is_empty(),
            "__moddefs" => !self.ast.module_defines.is_empty(),
            "__modules" => !self.ast.modules.is_empty(),
            s if s.starts_with("__include_") => {
                let idx: usize = s
                    .trim_start_matches("__include_")
                    .parse()
                    .unwrap_or(usize::MAX);
                idx < self.ast.includes.len()
            }
            s if s.starts_with("__use_") => {
                let idx: usize = s.trim_start_matches("__use_").parse().unwrap_or(usize::MAX);
                idx < self.ast.uses.len()
            }
            s if s.starts_with("__var_") => {
                let name = s
                    .trim_start_matches("__var_s_")
                    .trim_start_matches("__var_n_");
                self.ast.global_variables.iter().any(|v| v.name == name)
            }
            s if s.starts_with("__func_") => {
                let name = s.trim_start_matches("__func_");
                self.ast.function_defines.iter().any(|f| f.name == name)
            }
            s if s.starts_with("__moddef_") => {
                let name = s.trim_start_matches("__moddef_");
                self.ast.module_defines.iter().any(|m| m.name == name)
            }
            _ => false,
        }
    }

    /// Validate and rebuild tree state path
    /// TreeState stores a path vector. When tree structure changes, this path may become invalid.
    /// This method ensures the path is still valid or rebuilds it.
    pub fn validate_tree_state(&mut self) {
        // Extract current path within a scope to properly drop the borrow
        let current_path = self.tree_state.borrow().selected().to_vec();

        if current_path.is_empty() {
            // No selection, try to select first section
            self.init_tree_selection();
            return;
        }

        // Check if the last node in the path still exists
        if let Some(last_node_id) = current_path.last() {
            // Check section headers and their children
            if last_node_id.starts_with("__") {
                if self.is_valid_section_id(last_node_id) {
                    return;
                }
            } else if self.ast.find_node_by_id(last_node_id).is_some()
                || self.find_module_definition_for_node(last_node_id).is_some()
            {
                return;
            }
        }

        // Last node in path doesn't exist, try to select the last valid node in the path
        for i in (0..current_path.len()).rev() {
            let node_id = &current_path[i];
            if node_id.starts_with("__") {
                if self.is_valid_section_id(node_id) {
                    self.tree_state.borrow_mut().select(vec![node_id.clone()]);
                    return;
                }
            } else if self.ast.find_node_by_id(node_id).is_some()
                || self.find_module_definition_for_node(node_id).is_some()
            {
                self.tree_state.borrow_mut().select(vec![node_id.clone()]);
                return;
            }
        }

        // No valid node in path, select first section
        self.init_tree_selection();
    }

    #[allow(dead_code)]
    pub fn toggle_command_mode(&mut self) {
        // Legacy method - no longer used, kept for compatibility
        // All input is now command-based
    }

    /// Find the path to a node (from root to the node)
    /// Returns a vector of node IDs representing the path from root to target node
    pub fn find_node_path(&self, target_id: &str) -> Option<Vec<String>> {
        // Check if target is a section header
        if target_id.starts_with("__") {
            // Check if it's a module definition ID
            if let Some(module_name) = target_id.strip_prefix("__moddef_") {
                // Verify this module definition exists
                if self
                    .ast
                    .module_defines
                    .iter()
                    .any(|md| md.name == module_name)
                {
                    return Some(vec!["__moddefs".to_string(), target_id.to_string()]);
                }
                return None;
            }

            if self.is_valid_section_id(target_id) {
                return Some(vec![target_id.to_string()]);
            }
            return None;
        }

        // For module nodes, search within the modules section
        let mut path = vec!["__modules".to_string()];
        if let Some(mut module_path) =
            Self::find_node_path_recursive(&self.ast.modules, target_id, &mut Vec::new())
        {
            path.append(&mut module_path);
            return Some(path);
        }

        // If not found in modules, search in module definitions
        if let Some(mod_def_path) =
            Self::find_node_in_module_definitions(&self.ast.module_defines, target_id)
        {
            return Some(mod_def_path);
        }

        None
    }

    /// Check if a node is inside a module definition and return the module definition name
    pub fn find_module_definition_for_node(&self, node_id: &str) -> Option<String> {
        // First, check if the node exists in the modules section (instances)
        // If it does, it's not in a module definition (even if same ID appears in definition body)
        if self.ast.find_node_by_id(node_id).is_some() {
            return None;
        }

        // Check if node_id is a module definition itself (__moddef_{name})
        if let Some(module_name) = node_id.strip_prefix("__moddef_") {
            // Verify this module definition exists
            if self
                .ast
                .module_defines
                .iter()
                .any(|md| md.name == module_name)
            {
                return Some(module_name.to_string());
            }
        }

        for mod_def in &self.ast.module_defines {
            // Check if node is in module definition body
            if Self::find_node_in_module_body(&mod_def.body, node_id) {
                return Some(mod_def.name.clone());
            }
        }
        None
    }

    /// Helper to check if a node is in module definition body
    fn find_node_in_module_body(modules: &[openscad_core::ModuleNode], target_id: &str) -> bool {
        const MAX_DEPTH: usize = 1000;
        fn find_recursive(
            modules: &[openscad_core::ModuleNode],
            target_id: &str,
            depth: usize,
        ) -> bool {
            if depth >= MAX_DEPTH {
                return false;
            }
            for module in modules {
                if module.id == target_id {
                    return true;
                }
                if find_recursive(&module.children, target_id, depth + 1) {
                    return true;
                }
            }
            false
        }
        find_recursive(modules, target_id, 0)
    }

    fn find_node_path_recursive(
        modules: &[openscad_core::ModuleNode],
        target_id: &str,
        current_path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        const MAX_DEPTH: usize = 1000;
        fn find_recursive(
            modules: &[openscad_core::ModuleNode],
            target_id: &str,
            current_path: &mut Vec<String>,
            depth: usize,
        ) -> Option<Vec<String>> {
            if depth >= MAX_DEPTH {
                return None;
            }
            for module in modules {
                current_path.push(module.id.clone());

                if module.id == target_id {
                    return Some(current_path.clone());
                }

                if let Some(path) =
                    find_recursive(&module.children, target_id, current_path, depth + 1)
                {
                    return Some(path);
                }

                current_path.pop();
            }
            None
        }
        find_recursive(modules, target_id, current_path, 0)
    }

    /// Search for a node within module definitions
    fn find_node_in_module_definitions(
        module_defs: &[openscad_core::ModuleDefinition],
        target_id: &str,
    ) -> Option<Vec<String>> {
        for mod_def in module_defs {
            let mod_def_id = format!("__moddef_{}", mod_def.name);
            // Check if target is the module definition itself
            if mod_def_id == target_id {
                return Some(vec!["__moddefs".to_string(), mod_def_id]);
            }

            // Search in module definition body
            let mut path = vec!["__moddefs".to_string(), mod_def_id];
            if let Some(mut body_path) =
                Self::find_node_path_recursive(&mod_def.body, target_id, &mut Vec::new())
            {
                path.append(&mut body_path);
                return Some(path);
            }
        }
        None
    }

    pub fn push_undo(&mut self) {
        if self.undo_stack.len() >= 100 {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(self.ast.clone());
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop_back() {
            self.redo_stack.push_back(self.ast.clone());
            self.ast = prev;
            self.restore_tree_selection();
            self.clear_error();
        } else {
            self.set_error("Nothing to undo");
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop_back() {
            self.undo_stack.push_back(self.ast.clone());
            self.ast = next;
            self.restore_tree_selection();
            self.clear_error();
        } else {
            self.set_error("Nothing to redo");
        }
    }

    pub fn set_error(&mut self, msg: &str) {
        self.message = Some(msg.to_string());
        self.message_type = MessageType::Error;
    }

    /// Set an info message
    pub fn set_info(&mut self, msg: &str) {
        self.message = Some(msg.to_string());
        self.message_type = MessageType::Info;
    }

    /// Set a warning message
    #[allow(dead_code)]
    pub fn set_warning(&mut self, msg: &str) {
        self.message = Some(msg.to_string());
        self.message_type = MessageType::Warning;
    }

    /// Clear the current message
    pub fn clear_message(&mut self) {
        self.message = None;
        self.message_type = MessageType::Info;
    }

    pub fn clear_error(&mut self) {
        self.clear_message();
    }

    #[allow(dead_code)]
    pub fn clear_input(&mut self) {
        self.input_buffer.clear();
    }

    /// Update navigation status message based on current tree selection
    pub fn update_navigation_status(&mut self) {
        let selected = self.tree_state.borrow().selected().last().cloned();
        if let Some(node_id) = selected {
            // Handle section headers
            let display_name = if node_id.starts_with("__") {
                self.get_section_display_name(&node_id)
            } else {
                self.find_module_name(&node_id)
                    .unwrap_or_else(|| node_id.clone())
            };
            self.set_info(&format!("> {}", display_name));
        } else {
            self.clear_message();
        }
    }

    /// Get display name for section headers and their children
    fn get_section_display_name(&self, node_id: &str) -> String {
        match node_id {
            "__includes" => "[Includes]".to_string(),
            "__uses" => "[Uses]".to_string(),
            "__globals" => "[Global Variables]".to_string(),
            "__functions" => "[Functions]".to_string(),
            "__moddefs" => "[Module Definitions]".to_string(),
            "__modules" => "[Modules]".to_string(),
            s if s.starts_with("__include_") => {
                let idx: usize = s.trim_start_matches("__include_").parse().unwrap_or(0);
                self.ast
                    .includes
                    .get(idx)
                    .cloned()
                    .unwrap_or_else(|| s.to_string())
            }
            s if s.starts_with("__use_") => {
                let idx: usize = s.trim_start_matches("__use_").parse().unwrap_or(0);
                self.ast
                    .uses
                    .get(idx)
                    .cloned()
                    .unwrap_or_else(|| s.to_string())
            }
            s if s.starts_with("__var_s_") => {
                let name = s.trim_start_matches("__var_s_");
                self.ast
                    .global_variables
                    .iter()
                    .find(|v| v.name == name && v.is_special)
                    .map(|v| format!("${} = {}", v.name, v.value.to_scad()))
                    .unwrap_or_else(|| s.to_string())
            }
            s if s.starts_with("__var_n_") => {
                let name = s.trim_start_matches("__var_n_");
                self.ast
                    .global_variables
                    .iter()
                    .find(|v| v.name == name && !v.is_special)
                    .map(|v| format!("{} = {}", v.name, v.value.to_scad()))
                    .unwrap_or_else(|| s.to_string())
            }
            s if s.starts_with("__func_") => {
                let name = s.trim_start_matches("__func_");
                self.ast
                    .function_defines
                    .iter()
                    .find(|f| f.name == name)
                    .map(|f| {
                        let params = f
                            .parameters
                            .iter()
                            .map(|p| p.to_scad())
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("function {}({})", f.name, params)
                    })
                    .unwrap_or_else(|| s.to_string())
            }
            s if s.starts_with("__moddef_") => {
                let name = s.trim_start_matches("__moddef_");
                self.ast
                    .module_defines
                    .iter()
                    .find(|m| m.name == name)
                    .map(|m| {
                        let params = m
                            .parameters
                            .iter()
                            .map(|p| p.to_scad())
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("module {}({})", m.name, params)
                    })
                    .unwrap_or_else(|| s.to_string())
            }
            _ => node_id.to_string(),
        }
    }

    /// Find module display name by node ID
    fn find_module_name(&self, node_id: &str) -> Option<String> {
        Self::find_module_name_recursive(&self.ast.modules, node_id)
    }

    /// Recursively find module name in the AST
    fn find_module_name_recursive(
        modules: &[openscad_core::ModuleNode],
        node_id: &str,
    ) -> Option<String> {
        const MAX_DEPTH: usize = 1000;
        fn find_recursive(
            modules: &[openscad_core::ModuleNode],
            node_id: &str,
            depth: usize,
        ) -> Option<String> {
            if depth >= MAX_DEPTH {
                return None;
            }
            for module in modules {
                if module.id == node_id {
                    return Some(module.get_display_name());
                }
                if let Some(name) = find_recursive(&module.children, node_id, depth + 1) {
                    return Some(name);
                }
            }
            None
        }
        find_recursive(modules, node_id, 0)
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
