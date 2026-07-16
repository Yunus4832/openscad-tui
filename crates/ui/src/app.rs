//! Application state

use openscad_core::AstRoot;
use openscad_library::LibraryManager;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Arc;
use tui_tree_widget::TreeState;

use crate::command_registry::{CommandRegistry, CommandType};

/// Input buffer with cursor position management
#[derive(Debug, Clone)]
pub struct InputBuffer {
    buffer: String,
    cursor_pos: usize,
}

impl InputBuffer {
    /// Create a new empty input buffer
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor_pos: 0,
        }
    }

    /// Get the buffer content
    pub fn content(&self) -> &str {
        &self.buffer
    }

    /// Get the buffer content as mutable string (use with caution)
    #[allow(dead_code)]
    pub fn content_mut(&mut self) -> &mut String {
        &mut self.buffer
    }

    /// Get cursor position
    pub fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }

    /// Set cursor position (clamped to valid range)
    #[allow(dead_code)]
    pub fn set_cursor_pos(&mut self, pos: usize) {
        self.cursor_pos = pos;
        self.clamp_cursor();
    }

    /// Convert character index to byte index
    fn char_to_byte_index(&self, char_idx: usize) -> usize {
        self.buffer
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.buffer.len())
    }

    /// Insert character at cursor position
    pub fn insert_char(&mut self, ch: char) {
        let byte_pos = self.char_to_byte_index(self.cursor_pos);
        self.buffer.insert(byte_pos, ch);
        self.cursor_pos += 1;
    }

    /// Insert string at cursor position
    pub fn insert_str(&mut self, s: &str) {
        let byte_pos = self.char_to_byte_index(self.cursor_pos);
        self.buffer.insert_str(byte_pos, s);
        self.cursor_pos += s.chars().count();
    }

    /// Set buffer content (replaces entire content, moves cursor to end)
    pub fn set_content(&mut self, content: &str) {
        self.buffer = content.to_string();
        self.cursor_pos = self.buffer.len();
    }

    /// Delete character before cursor (backspace)
    pub fn delete_before_cursor(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
            self.buffer.remove(self.cursor_pos);
        }
    }

    /// Delete character at cursor (delete key)
    pub fn delete_at_cursor(&mut self) {
        if self.cursor_pos < self.buffer.len() {
            self.buffer.remove(self.cursor_pos);
        }
    }

    /// Clear the input buffer
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor_pos = 0;
    }

    /// Move cursor left
    pub fn move_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
        }
    }

    /// Move cursor right
    pub fn move_right(&mut self) {
        if self.cursor_pos < self.buffer.len() {
            self.cursor_pos += 1;
        }
    }

    /// Move cursor to start
    pub fn move_to_start(&mut self) {
        self.cursor_pos = 0;
    }

    /// Move cursor to end
    pub fn move_to_end(&mut self) {
        self.cursor_pos = self.buffer.len();
    }

    /// Ensure cursor position is within bounds
    pub fn clamp_cursor(&mut self) {
        if self.cursor_pos > self.buffer.len() {
            self.cursor_pos = self.buffer.len();
        }
    }

    /// Replace a range in the buffer
    pub fn replace_range(&mut self, start: usize, end: usize, replacement: &str) {
        self.buffer.replace_range(start..end, replacement);
        // Update cursor to end of replacement
        self.cursor_pos = start + replacement.len();
    }

    /// Get buffer length
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Check if buffer is empty
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Get a substring of the buffer
    #[allow(dead_code)]
    pub fn substring(&self, start: usize, end: usize) -> &str {
        &self.buffer[start..end]
    }
}

impl Default for InputBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// 补全候选项
#[derive(Debug, Clone)]
pub struct CompletionCandidate {
    pub content: String,
    pub candidate_type: CandidateType,
}

impl CompletionCandidate {
    pub fn new(content: String, candidate_type: CandidateType) -> Self {
        Self {
            content,
            candidate_type,
        }
    }
}

/// 补全候选项类型
#[derive(Debug, Clone, PartialEq)]
pub enum CandidateType {
    Module,
    ModuleParam,
    Function,
    Path,
    GlobalVar,
    Value,
    Command,
}

impl CandidateType {
    /// 返回候选项符号
    pub fn flag(&self) -> &'static str {
        match self {
            Self::Module => "M",
            Self::ModuleParam => "MP",
            Self::Function => "F",
            Self::Path => "PA",
            Self::GlobalVar => "G",
            Self::Value => "V",
            Self::Command => "C",
        }
    }

    /// 返回候选项分隔符
    pub fn separator(&self) -> &'static str {
        match self {
            Self::Module => " ",
            Self::ModuleParam => "=",
            Self::Function => "(",
            Self::Path => "/",
            Self::GlobalVar => ",",
            Self::Value => ",",
            Self::Command => " ",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum InputMode {
    /// Command mode - all input is command-based
    Command,
    /// Multi-stage module action - entering parameters for insert or replace.
    ModuleEnterParams,
    /// Help modal - displaying help information
    Help,
    /// Legacy modes (no longer used, kept for compatibility)
    Normal,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PendingModuleAction {
    Insert,
    Replace { target_ids: Vec<String> },
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
    },
    /// Module parameter value completion (after module parameter name)
    ModuleParamValue {
        cmd_type: CommandType,
        module_name: String,
        module_param_name: String,
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
    /// Application-local clipboard for copied module subtrees.
    pub node_clipboard: Option<openscad_core::ModuleNode>,

    // UI state - Tree navigation (using RefCell for interior mutability)
    pub tree_state: RefCell<TreeState<String>>,
    #[allow(dead_code)]
    pub tree_cursor: usize,
    #[allow(dead_code)]
    pub expanded_nodes: std::collections::HashSet<String>,

    // UI state - Input and display
    pub input_buffer: InputBuffer,
    pub input_mode: InputMode,
    /// For insert mode: whether to insert after (true) or before (false)
    #[allow(dead_code)]
    pub insert_after: bool,
    pub pending_module_action: Option<PendingModuleAction>,
    pub pending_module_name: Option<String>,
    pub preview_offset: usize,
    pub should_quit: bool,
    pub message: Option<String>,
    pub message_type: MessageType,

    // Tab completion state
    pub completion_candidates: Vec<CompletionCandidate>,
    pub completion_index: usize,
    pub completion_context: CompletionContext,
    pub completion_replacement_range: (usize, usize),
    pub completion_active: bool,

    // Command history
    pub command_history: Vec<String>,
    pub history_index: Option<usize>,
    pub history_draft: Option<String>,
    pub history_max_size: usize,

    // 文件相关
    pub current_file: Option<String>,
    pub saved: bool,

    // 帮助界面状态
    pub help_scroll_offset: usize,
    pub help_scroll_offset_max: usize,
    pub help_doc: Vec<String>,
    pub help_doc_count: usize,
    pub help_modal_width: usize,
    pub help_modal_height: usize,
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
            node_clipboard: None,
            tree_state: RefCell::new(TreeState::default()),
            tree_cursor: 0,
            expanded_nodes: std::collections::HashSet::new(),
            input_buffer: InputBuffer::new(),
            input_mode: InputMode::Normal,
            insert_after: true,
            pending_module_action: None,
            pending_module_name: None,
            preview_offset: 0,
            should_quit: false,
            message: None,
            message_type: MessageType::Info,
            completion_candidates: Vec::new(),
            completion_index: 0,
            completion_context: CompletionContext::Command,
            completion_replacement_range: (0, 0),
            completion_active: false,
            command_history: Vec::new(),
            history_index: None,
            history_draft: None,
            history_max_size: 100,
            current_file: None,
            saved: true,
            help_scroll_offset: 0,
            help_scroll_offset_max: 0,
            help_doc: Vec::new(),
            help_doc_count: 0,
            help_modal_width: 0,
            help_modal_height: 0,
        };

        // Load standard library (from config dir if exists, otherwise use embedded)
        if let Err(e) = app.library.load_stdlib_with_config() {
            eprintln!("Failed to load standard library: {}", e);
        }

        // Initialize command registry
        crate::commands::init_command_registry(&mut app.command_registry);

        // Initialize tree state: select the first module if it exists
        app.init_tree_selection();
        app.calculate_help_modal_size();
        app
    }

    pub fn set_help_doc(&mut self, help_doc: Vec<String>) {
        self.help_doc_count = help_doc.len();
        self.help_doc = help_doc;
        self.help_scroll_offset = 0;
        self.calculate_help_modal_size();
    }

    // 计算帮助窗口的高度
    pub fn calculate_help_modal_size(&mut self) {
        let (width, height) = crossterm::terminal::size().unwrap_or((80, 24));

        let help_modal_width: usize = (width as f32 * 0.8) as usize;
        let help_modal_height: usize = (height as f32 * 0.8) as usize;

        self.help_modal_width = help_modal_width;
        self.help_modal_height = help_modal_height;

        self.help_scroll_offset_max = self.help_doc_count.saturating_sub(help_modal_height - 2);
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

    /// Ensure cursor position is within bounds of input buffer
    pub fn clamp_cursor(&mut self) {
        self.input_buffer.clamp_cursor();
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

    pub fn mark_dirty(&mut self) {
        self.saved = false;
    }

    pub fn mark_saved(&mut self) {
        self.saved = true;
    }

    /// Add command to history
    pub fn add_to_history(&mut self, command: &str) {
        if !command.trim().is_empty() {
            // Avoid duplicate commands (if same as last, don't add)
            if self
                .command_history
                .last()
                .is_none_or(|last| last != command)
            {
                self.command_history.push(command.to_string());

                // Limit history size
                if self.command_history.len() > self.history_max_size {
                    self.command_history.remove(0); // Remove oldest record
                }
            }
        }

        // Reset history index
        self.history_index = None;
        self.history_draft = None;
    }

    /// Get previous command from history
    pub fn get_previous_command(&mut self, current_input: &str) -> Option<String> {
        if self.command_history.is_empty() {
            self.history_index = None;
            return None;
        }

        if self.history_index.is_none() {
            self.history_draft = Some(current_input.to_string());
        }

        let index = match self.history_index {
            Some(i) => {
                if i > 0 {
                    i - 1
                } else {
                    0
                }
            }
            None => self.command_history.len() - 1,
        };

        self.history_index = Some(index);
        Some(self.command_history[index].clone())
    }

    /// Get next command from history
    pub fn get_next_command(&mut self) -> Option<String> {
        match self.history_index {
            None => None,
            Some(i) => {
                if i < self.command_history.len() - 1 {
                    let next_index = i + 1;
                    self.history_index = Some(next_index);
                    Some(self.command_history[next_index].clone())
                } else {
                    // Return to the unfinished input captured before history navigation.
                    self.history_index = None;
                    Some(self.history_draft.take().unwrap_or_default())
                }
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_buffer_new() {
        let buf = InputBuffer::new();
        assert_eq!(buf.content(), "");
        assert_eq!(buf.cursor_pos(), 0);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_command_history_add_and_get() {
        let mut app = App::new();

        // Add some commands to history
        app.add_to_history("help");
        app.add_to_history("insert cube");
        app.add_to_history("translate [10, 0, 0]");

        // Verify history contains all commands
        assert_eq!(app.command_history.len(), 3);
        assert_eq!(app.command_history[0], "help");
        assert_eq!(app.command_history[1], "insert cube");
        assert_eq!(app.command_history[2], "translate [10, 0, 0]");

        // Test getting previous commands (going backwards in history)
        assert_eq!(
            app.get_previous_command(""),
            Some("translate [10, 0, 0]".to_string())
        );
        assert_eq!(app.history_index, Some(2));

        assert_eq!(
            app.get_previous_command(""),
            Some("insert cube".to_string())
        );
        assert_eq!(app.history_index, Some(1));

        assert_eq!(app.get_previous_command(""), Some("help".to_string()));
        assert_eq!(app.history_index, Some(0));

        // Should stay at the first command when trying to go further back
        assert_eq!(app.get_previous_command(""), Some("help".to_string()));
        assert_eq!(app.history_index, Some(0));

        // Test getting next commands (going forward in history)
        assert_eq!(app.get_next_command(), Some("insert cube".to_string()));
        assert_eq!(app.history_index, Some(1));

        assert_eq!(
            app.get_next_command(),
            Some("translate [10, 0, 0]".to_string())
        );
        assert_eq!(app.history_index, Some(2));

        // Going forward past the end should clear the index and return empty string
        assert_eq!(app.get_next_command(), Some(String::new()));
        assert_eq!(app.history_index, None);
    }

    #[test]
    fn test_command_history_restores_unfinished_input() {
        let mut app = App::new();
        app.add_to_history("insert cube");

        assert_eq!(
            app.get_previous_command("replace sph"),
            Some("insert cube".to_string())
        );
        assert_eq!(app.get_next_command(), Some("replace sph".to_string()));
        assert_eq!(app.history_index, None);
        assert_eq!(app.history_draft, None);
    }

    #[test]
    fn test_command_history_duplicates() {
        let mut app = App::new();

        // Add the same command twice
        app.add_to_history("help");
        app.add_to_history("help"); // This should not be added

        // Should only have one command in history
        assert_eq!(app.command_history.len(), 1);
        assert_eq!(app.command_history[0], "help");
    }

    #[test]
    fn test_command_history_empty() {
        let mut app = App::new();

        // Getting from empty history should return None
        assert_eq!(app.get_previous_command(""), None);
        assert_eq!(app.get_next_command(), None);
    }

    #[test]
    fn test_command_history_size_limit() {
        let mut app = App::new();
        app.history_max_size = 3; // Set small limit for testing

        // Add more commands than the limit
        app.add_to_history("cmd1");
        app.add_to_history("cmd2");
        app.add_to_history("cmd3");
        app.add_to_history("cmd4"); // This should cause the first one to be removed
        app.add_to_history("cmd5"); // This should cause more removals

        // Should only have the last 3 commands
        assert_eq!(app.command_history.len(), 3);
        assert_eq!(app.command_history[0], "cmd3");
        assert_eq!(app.command_history[1], "cmd4");
        assert_eq!(app.command_history[2], "cmd5");
    }

    #[test]
    fn test_input_buffer_insert_char() {
        let mut buf = InputBuffer::new();
        buf.insert_char('a');
        assert_eq!(buf.content(), "a");
        assert_eq!(buf.cursor_pos(), 1);
        buf.insert_char('b');
        assert_eq!(buf.content(), "ab");
        assert_eq!(buf.cursor_pos(), 2);
        buf.move_left();
        buf.insert_char('c');
        assert_eq!(buf.content(), "acb");
        assert_eq!(buf.cursor_pos(), 2);
    }

    #[test]
    fn test_input_buffer_insert_str() {
        let mut buf = InputBuffer::new();
        buf.insert_str("hello");
        assert_eq!(buf.content(), "hello");
        assert_eq!(buf.cursor_pos(), 5);
        buf.move_to_start();
        buf.insert_str("world");
        assert_eq!(buf.content(), "worldhello");
        assert_eq!(buf.cursor_pos(), 5);
    }

    #[test]
    fn test_input_buffer_set_content() {
        let mut buf = InputBuffer::new();
        buf.set_content("test");
        assert_eq!(buf.content(), "test");
        assert_eq!(buf.cursor_pos(), 4);
        buf.set_content("");
        assert_eq!(buf.content(), "");
        assert_eq!(buf.cursor_pos(), 0);
    }

    #[test]
    fn test_input_buffer_delete_before_cursor() {
        let mut buf = InputBuffer::new();
        buf.set_content("hello");
        buf.move_to_end();
        buf.delete_before_cursor();
        assert_eq!(buf.content(), "hell");
        assert_eq!(buf.cursor_pos(), 4);
        buf.move_to_start();
        buf.delete_before_cursor(); // Should do nothing at start
        assert_eq!(buf.content(), "hell");
        assert_eq!(buf.cursor_pos(), 0);
    }

    #[test]
    fn test_input_buffer_delete_at_cursor() {
        let mut buf = InputBuffer::new();
        buf.set_content("hello");
        buf.move_to_start();
        buf.delete_at_cursor();
        assert_eq!(buf.content(), "ello");
        assert_eq!(buf.cursor_pos(), 0);
        buf.move_to_end();
        buf.delete_at_cursor(); // Should do nothing at end
        assert_eq!(buf.content(), "ello");
        assert_eq!(buf.cursor_pos(), 4);
    }

    #[test]
    fn test_input_buffer_move_cursor() {
        let mut buf = InputBuffer::new();
        buf.set_content("hello");
        assert_eq!(buf.cursor_pos(), 5);
        buf.move_left();
        assert_eq!(buf.cursor_pos(), 4);
        buf.move_right();
        assert_eq!(buf.cursor_pos(), 5);
        buf.move_right(); // Should not go beyond end
        assert_eq!(buf.cursor_pos(), 5);
        buf.move_to_start();
        assert_eq!(buf.cursor_pos(), 0);
        buf.move_left(); // Should not go below 0
        assert_eq!(buf.cursor_pos(), 0);
        buf.move_to_end();
        assert_eq!(buf.cursor_pos(), 5);
    }

    #[test]
    fn test_input_buffer_clear() {
        let mut buf = InputBuffer::new();
        buf.set_content("hello");
        buf.move_left();
        buf.clear();
        assert_eq!(buf.content(), "");
        assert_eq!(buf.cursor_pos(), 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_input_buffer_replace_range() {
        let mut buf = InputBuffer::new();
        buf.set_content("hello world");
        buf.replace_range(6, 11, "there");
        assert_eq!(buf.content(), "hello there");
        assert_eq!(buf.cursor_pos(), 11);
        buf.replace_range(0, 5, "hi");
        assert_eq!(buf.content(), "hi there");
        assert_eq!(buf.cursor_pos(), 2);
    }

    #[test]
    fn test_input_buffer_substring() {
        let mut buf = InputBuffer::new();
        buf.set_content("hello world");
        assert_eq!(buf.substring(0, 5), "hello");
        assert_eq!(buf.substring(6, 11), "world");
    }

    #[test]
    fn test_input_buffer_unicode() {
        let mut buf = InputBuffer::new();
        buf.insert_str("café");
        assert_eq!(buf.content(), "café");
        assert_eq!(buf.cursor_pos(), 4); // 4 characters, not bytes
        buf.move_to_start();
        buf.move_right();
        buf.move_right();
        buf.delete_before_cursor();
        assert_eq!(buf.content(), "cfé");
        assert_eq!(buf.cursor_pos(), 1);
    }

    #[test]
    fn test_input_buffer_clamp_cursor() {
        let mut buf = InputBuffer::new();
        buf.set_content("hello");
        // Manually set cursor out of bounds
        buf.cursor_pos = 10;
        buf.clamp_cursor();
        assert_eq!(buf.cursor_pos(), 5);
        buf.cursor_pos = 3;
        buf.clamp_cursor();
        assert_eq!(buf.cursor_pos(), 3);
    }

    #[test]
    fn test_input_buffer_set_cursor_pos() {
        let mut buf = InputBuffer::new();
        buf.set_content("hello");
        buf.set_cursor_pos(2);
        assert_eq!(buf.cursor_pos(), 2);
        buf.set_cursor_pos(10); // Should clamp to 5
        assert_eq!(buf.cursor_pos(), 5);
    }
}
