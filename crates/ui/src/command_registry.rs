//! Command registry for managing command definitions and aliases

use crate::app::App;
use crate::commands::CommandResult;
use std::collections::HashMap;
use std::ops::Range;

/// Specialized OpenSCAD editor completion mode for legacy single-leaf commands.
///
/// Resource commands use [`ArgumentSpec`] instead; this enum remains only for AST-aware
/// completion such as modules, expressions, and node parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandType {
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
    Visibility,
}

/// Command type - used for dynamic completion context analysis
/// Command handler function signature
pub type CommandHandler = fn(&mut App, &[&str]) -> CommandResult<()>;

/// One parsed command-line token and its byte range in the original input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandToken {
    pub value: String,
    pub range: Range<usize>,
}

/// Tokenized command input shared by execution and completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    pub tokens: Vec<CommandToken>,
    pub trailing_separator: bool,
    pub unterminated_quote: bool,
}

impl CommandLine {
    /// Parse shell-like quoted words without performing environment or shell expansion.
    pub fn parse(input: &str) -> Self {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Quote {
            Single,
            Double,
        }

        let mut tokens = Vec::new();
        let mut value = String::new();
        let mut start = None;
        let mut quote = None;
        let mut escaped = false;
        let mut last_end = 0;

        for (index, character) in input.char_indices() {
            let end = index + character.len_utf8();
            last_end = end;
            if escaped {
                start.get_or_insert(index);
                value.push(character);
                escaped = false;
                continue;
            }
            match (quote, character) {
                (Some(Quote::Single), '\'') => quote = None,
                (Some(Quote::Single), _) => value.push(character),
                (Some(Quote::Double), '"') => quote = None,
                (Some(Quote::Double), '\\') => escaped = true,
                (Some(Quote::Double), _) => value.push(character),
                (None, '\'') => {
                    start.get_or_insert(index);
                    quote = Some(Quote::Single);
                }
                (None, '"') => {
                    start.get_or_insert(index);
                    quote = Some(Quote::Double);
                }
                (None, '\\') => {
                    start.get_or_insert(index);
                    escaped = true;
                }
                (None, value_character) if value_character.is_whitespace() => {
                    if let Some(token_start) = start.take() {
                        tokens.push(CommandToken {
                            value: std::mem::take(&mut value),
                            range: token_start..index,
                        });
                    }
                }
                (None, _) => {
                    start.get_or_insert(index);
                    value.push(character);
                }
            }
        }

        if escaped {
            value.push('\\');
        }
        if let Some(token_start) = start {
            tokens.push(CommandToken {
                value,
                range: token_start..last_end,
            });
        }

        Self {
            tokens,
            trailing_separator: quote.is_none()
                && input.chars().last().is_some_and(char::is_whitespace),
            unterminated_quote: quote.is_some(),
        }
    }

    pub fn values(&self) -> Vec<&str> {
        self.tokens
            .iter()
            .map(|token| token.value.as_str())
            .collect()
    }
}

/// A leaf command found by longest-path matching.
pub struct ResolvedCommand<'a> {
    pub definition: &'a CommandDef,
    pub consumed_tokens: usize,
}

/// Declarative completion source for one command argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionSource {
    None,
    Literal(Vec<String>),
    Path { extensions: Vec<String> },
    ProjectSource { editable_only: bool },
    LoadedLibrary,
    LibraryRoot,
    Assembly,
    AssemblyPart { literals: Vec<String> },
    CommandPath,
}

/// Argument metadata shared by completion, validation, and help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentSpec {
    pub name: String,
    pub required: bool,
    pub variadic: bool,
    pub completion: CompletionSource,
}

impl ArgumentSpec {
    pub fn new(name: impl Into<String>, required: bool, completion: CompletionSource) -> Self {
        Self {
            name: name.into(),
            required,
            variadic: false,
            completion,
        }
    }

    pub fn variadic(mut self) -> Self {
        self.variadic = true;
        self
    }

    pub fn literal(name: impl Into<String>, required: bool, values: &[&str]) -> Self {
        Self::new(
            name,
            required,
            CompletionSource::Literal(values.iter().map(|value| (*value).to_string()).collect()),
        )
    }

    pub fn path(name: impl Into<String>, required: bool, extensions: &[&str]) -> Self {
        Self::new(
            name,
            required,
            CompletionSource::Path {
                extensions: extensions
                    .iter()
                    .map(|extension| (*extension).to_string())
                    .collect(),
            },
        )
    }
}

/// Command definition structure
pub struct CommandDef {
    /// Canonical command path, for example `model view`.
    pub name: String,
    /// Complete alternative command paths.
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
    /// Declarative argument metadata for hierarchical commands. Empty means legacy completion.
    pub arguments: Vec<ArgumentSpec>,
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
            arguments: Vec::new(),
            change_ast,
            write_to_history,
        }
    }

    pub fn with_arguments(mut self, arguments: Vec<ArgumentSpec>) -> Self {
        self.arguments = arguments;
        self
    }

    pub fn argument_bounds(&self) -> (usize, Option<usize>) {
        if self.arguments.is_empty() {
            return (self.min_args, self.max_args);
        }
        let minimum = self
            .arguments
            .iter()
            .filter(|argument| argument.required)
            .count();
        let maximum = if self
            .arguments
            .last()
            .is_some_and(|argument| argument.variadic)
        {
            None
        } else {
            Some(self.arguments.len())
        };
        (minimum, maximum)
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

    /// Resolve the longest registered command path at the start of `tokens`.
    pub fn resolve<'a>(&'a self, tokens: &[&str]) -> Option<ResolvedCommand<'a>> {
        for consumed_tokens in (1..=tokens.len()).rev() {
            let path = tokens[..consumed_tokens].join(" ");
            if let Some(definition) = self.find(&path) {
                return Some(ResolvedCommand {
                    definition,
                    consumed_tokens,
                });
            }
        }
        None
    }

    /// List canonical command path components directly below `prefix`.
    pub fn child_names(&self, prefix: &[&str]) -> Vec<String> {
        let mut children = self
            .commands
            .keys()
            .filter_map(|name| {
                let path = name.split_whitespace().collect::<Vec<_>>();
                (path.len() > prefix.len() && path[..prefix.len()] == *prefix)
                    .then(|| path[prefix.len()].to_string())
            })
            .collect::<Vec<_>>();
        children.sort();
        children.dedup();
        children
    }

    pub fn is_namespace(&self, path: &[&str]) -> bool {
        !self.child_names(path).is_empty()
    }

    pub fn commands_below(&self, prefix: &[&str]) -> Vec<&CommandDef> {
        let mut commands = self
            .commands
            .values()
            .filter(|definition| {
                let path = definition.name.split_whitespace().collect::<Vec<_>>();
                path.len() > prefix.len() && path[..prefix.len()] == *prefix
            })
            .collect::<Vec<_>>();
        commands.sort_by(|left, right| left.name.cmp(&right.name));
        commands
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

#[cfg(test)]
mod tests {
    use super::*;

    fn handler(_app: &mut App, _args: &[&str]) -> CommandResult<()> {
        Ok(())
    }

    fn command(name: &str) -> CommandDef {
        CommandDef::new(
            name,
            Vec::<String>::new(),
            handler,
            name,
            0,
            None,
            name,
            Vec::<String>::new(),
            CommandType::NoArg,
            false,
            false,
        )
    }

    #[test]
    fn command_line_preserves_ranges_and_decodes_quoted_arguments() {
        let line = CommandLine::parse("model view \"models/my part.stl\"");
        assert_eq!(line.values(), ["model", "view", "models/my part.stl"]);
        assert_eq!(
            &"model view \"models/my part.stl\""[line.tokens[2].range.clone()],
            "\"models/my part.stl\""
        );
        assert!(!line.unterminated_quote);
    }

    #[test]
    fn resolves_the_longest_registered_command_path() {
        let mut registry = CommandRegistry::new();
        registry.register(command("model view"));
        registry.register(command("model export"));

        let resolved = registry.resolve(&["model", "view", "shape.stl"]).unwrap();
        assert_eq!(resolved.definition.name, "model view");
        assert_eq!(resolved.consumed_tokens, 2);
        assert_eq!(registry.child_names(&["model"]), ["export", "view"]);
    }

    #[test]
    fn argument_bounds_count_required_arguments_after_optional_targets() {
        let definition = command("assembly parent").with_arguments(vec![
            ArgumentSpec::new("part", false, CompletionSource::None),
            ArgumentSpec::new("parent", true, CompletionSource::None),
        ]);

        assert_eq!(definition.argument_bounds(), (1, Some(2)));
    }
}
