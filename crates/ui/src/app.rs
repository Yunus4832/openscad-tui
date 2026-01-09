//! Application state

use openscad_core::AstRoot;
use openscad_library::LibraryManager;
use std::collections::VecDeque;
use std::cell::RefCell;
use tui_tree_widget::TreeState;

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum InputMode {
    /// Command mode - all input is command-based
    Command,
    /// Multi-stage insert - entering parameters for insert command
    InsertEnterParams,
    /// Multi-stage replace - selecting replacement module
    ReplaceSelectModule,
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

pub struct App {
    pub ast: AstRoot,
    pub library: LibraryManager,
    pub selected_nodes: Vec<String>,
    pub undo_stack: VecDeque<AstRoot>,
    pub redo_stack: VecDeque<AstRoot>,
    
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
}

impl App {
    pub fn new() -> Self {
        let mut app = Self {
            ast: AstRoot::new(),
            library: LibraryManager::new(),
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
        };
        
        // Initialize tree state: select the first module if it exists
        app.init_tree_selection();
        app
    }
    
    /// Initialize tree state with first item selected if available
    pub fn init_tree_selection(&mut self) {
        if !self.ast.modules.is_empty() {
            if let Some(first_module) = self.ast.modules.first() {
                self.tree_state.borrow_mut().select(vec![first_module.id.clone()]);
            }
        }
    }
    
    /// Restore tree state to a valid position
    /// Called after AST modifications to ensure navigation isn't lost
    pub fn restore_tree_selection(&mut self) {
        let current_selection = self.tree_state.borrow().selected().last().cloned();
        
        // Check if current selection still exists in AST
        if let Some(ref node_id) = current_selection {
            if self.ast.find_node_by_id(node_id).is_some() {
                // Current selection is still valid, keep it
                return;
            }
        }
        
        // Current selection is invalid or empty, select first module
        if !self.ast.modules.is_empty() {
            if let Some(first_module) = self.ast.modules.first() {
                self.tree_state.borrow_mut().select(vec![first_module.id.clone()]);
            }
        } else {
            // No modules at all, clear selection
            self.tree_state.borrow_mut().select(vec![]);
        }
    }
    
    /// Validate and rebuild tree state path
    /// TreeState stores a path vector. When tree structure changes, this path may become invalid.
    /// This method ensures the path is still valid or rebuilds it.
    pub fn validate_tree_state(&mut self) {
        // Extract current path within a scope to properly drop the borrow
        let current_path = self.tree_state.borrow().selected().to_vec();
        
        if current_path.is_empty() {
            // No selection, try to select first module
            if !self.ast.modules.is_empty() {
                if let Some(first_module) = self.ast.modules.first() {
                    self.tree_state.borrow_mut().select(vec![first_module.id.clone()]);
                }
            }
            return;
        }
        
        // Check if the last node in the path still exists
        if let Some(last_node_id) = current_path.last() {
            if self.ast.find_node_by_id(last_node_id).is_some() {
                // The selected node still exists, keep the path as is
                return;
            }
        }
        
        // Last node in path doesn't exist, try to select the last valid node in the path
        for i in (0..current_path.len()).rev() {
            if self.ast.find_node_by_id(&current_path[i]).is_some() {
                self.tree_state.borrow_mut().select(vec![current_path[i].clone()]);
                return;
            }
        }
        
        // No valid node in path, select first module
        if !self.ast.modules.is_empty() {
            if let Some(first_module) = self.ast.modules.first() {
                self.tree_state.borrow_mut().select(vec![first_module.id.clone()]);
            }
        } else {
            self.tree_state.borrow_mut().select(vec![]);
        }
    }
    
    #[allow(dead_code)]
    pub fn toggle_command_mode(&mut self) {
        // Legacy method - no longer used, kept for compatibility
        // All input is now command-based
    }
    
    /// Find the path to a node (from root to the node)
    /// Returns a vector of node IDs representing the path from root to target node
    pub fn find_node_path(&self, target_id: &str) -> Option<Vec<String>> {
        self.find_node_path_recursive(&self.ast.modules, target_id, &mut Vec::new())
    }
    
    fn find_node_path_recursive(
        &self,
        modules: &[openscad_core::ModuleNode],
        target_id: &str,
        current_path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        for module in modules {
            current_path.push(module.id.clone());
            
            if module.id == target_id {
                return Some(current_path.clone());
            }
            
            if let Some(path) = self.find_node_path_recursive(&module.children, target_id, current_path) {
                return Some(path);
            }
            
            current_path.pop();
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
            // Find the module name from the node ID
            let module_name = self.find_module_name(&node_id)
                .unwrap_or_else(|| node_id.clone());
            self.set_info(&format!("→ {}", module_name));
        } else {
            self.clear_message();
        }
    }
    
    /// Find module display name by node ID
    fn find_module_name(&self, node_id: &str) -> Option<String> {
        self.find_module_name_recursive(&self.ast.modules, node_id)
    }
    
    /// Recursively find module name in the AST
    fn find_module_name_recursive(
        &self,
        modules: &[openscad_core::ModuleNode],
        node_id: &str,
    ) -> Option<String> {
        for module in modules {
            if module.id == node_id {
                return Some(module.get_display_name());
            }
            if let Some(name) = self.find_module_name_recursive(&module.children, node_id) {
                return Some(name);
            }
        }
        None
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
