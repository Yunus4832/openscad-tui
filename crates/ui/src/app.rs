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
    InsertSelectModule,
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
    pub error_message: Option<String>,
}

impl App {
    pub fn new() -> Self {
        Self {
            ast: AstRoot::new(),
            library: LibraryManager::new(),
            selected_nodes: Vec::new(),
            undo_stack: VecDeque::with_capacity(100),
            redo_stack: VecDeque::with_capacity(100),
            tree_state: RefCell::new(TreeState::default()),
            tree_cursor: 0,
            expanded_nodes: std::collections::HashSet::new(),
            input_buffer: String::new(),
            input_mode: InputMode::Command,
            insert_after: true,
            insert_module_name: None,
            preview_offset: 0,
            should_quit: false,
            error_message: None,
        }
    }
    
    #[allow(dead_code)]
    pub fn toggle_command_mode(&mut self) {
        // Legacy method - no longer used, kept for compatibility
        // All input is now command-based
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
            self.clear_error();
        } else {
            self.set_error("Nothing to undo");
        }
    }
    
    pub fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop_back() {
            self.undo_stack.push_back(self.ast.clone());
            self.ast = next;
            self.clear_error();
        } else {
            self.set_error("Nothing to redo");
        }
    }
    
    pub fn set_error(&mut self, msg: &str) {
        self.error_message = Some(msg.to_string());
    }
    
    pub fn clear_error(&mut self) {
        self.error_message = None;
    }
    
    #[allow(dead_code)]
    pub fn clear_input(&mut self) {
        self.input_buffer.clear();
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
