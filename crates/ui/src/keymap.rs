//! Context-aware editor key bindings.
//!
//! Bindings resolve to command strings or command-line prefills. The input layer only applies the
//! resolved action, which keeps resource-specific overrides separate from command execution and
//! leaves a stable seam for future user configuration.

use crate::app::{App, Screen};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorContext {
    ProjectSources,
    ProjectSource(String),
    Assemblies,
    Assembly(String),
    Ast,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    Execute(String),
    BeginCommand(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolbarButton {
    pub shortcut: String,
    pub label: String,
    pub command: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Global,
    EditorAny,
    ProjectSources,
    ProjectSource,
    Assemblies,
    AssemblyNode,
    Ast,
    Other,
    Model,
    AssemblyScreen,
    Camera,
}

#[derive(Clone, Copy)]
enum Key {
    Char(char),
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
}

#[derive(Clone, Copy)]
enum Action {
    Execute(&'static str),
    Begin(&'static str),
    OpenSelection,
    SaveProject,
    RenameSource,
    CopySource,
    CutSource,
    PasteSource,
    AssemblyTransform(&'static str),
    AssemblyParent,
    AssemblyVisibility,
}

#[derive(Clone, Copy)]
enum ButtonLabel {
    Fixed(&'static str),
    Close,
    Projection,
    Axes,
    AutoRotate,
}

#[derive(Clone, Copy)]
struct Binding {
    scope: Scope,
    key: Key,
    control: bool,
    action: Action,
    button: Option<ButtonLabel>,
}

const fn binding(scope: Scope, key: Key, action: Action) -> Binding {
    Binding {
        scope,
        key,
        control: false,
        action,
        button: None,
    }
}

const fn control_binding(scope: Scope, key: Key, action: Action) -> Binding {
    Binding {
        scope,
        key,
        control: true,
        action,
        button: None,
    }
}

const fn toolbar_binding(scope: Scope, key: Key, action: Action, label: ButtonLabel) -> Binding {
    Binding {
        scope,
        key,
        control: false,
        action,
        button: Some(label),
    }
}

const BINDINGS: &[Binding] = &[
    binding(Scope::Global, Key::Char('Q'), Action::Execute("quit!")),
    binding(Scope::ProjectSource, Key::Enter, Action::OpenSelection),
    binding(Scope::ProjectSource, Key::Char('r'), Action::RenameSource),
    binding(Scope::ProjectSource, Key::Char('y'), Action::CopySource),
    binding(Scope::ProjectSource, Key::Char('d'), Action::CutSource),
    binding(Scope::ProjectSource, Key::Char('p'), Action::PasteSource),
    binding(Scope::ProjectSources, Key::Char('p'), Action::PasteSource),
    binding(Scope::ProjectSources, Key::Enter, Action::Execute("toggle")),
    binding(Scope::AssemblyNode, Key::Enter, Action::OpenSelection),
    binding(Scope::Assemblies, Key::Enter, Action::Execute("toggle")),
    binding(Scope::Ast, Key::Char('v'), Action::Execute("select")),
    binding(
        Scope::Ast,
        Key::Char(' '),
        Action::Execute("visibility toggle"),
    ),
    binding(Scope::Ast, Key::Char('y'), Action::Execute("yank")),
    binding(Scope::Ast, Key::Char('p'), Action::Execute("paste")),
    binding(Scope::Ast, Key::Char('x'), Action::Execute("remove")),
    binding(Scope::Ast, Key::Char('c'), Action::Begin("replace ")),
    binding(Scope::Ast, Key::Char('a'), Action::Begin("set ")),
    binding(Scope::Ast, Key::Char('A'), Action::Begin("unset ")),
    binding(Scope::Ast, Key::Char('i'), Action::Begin("insert ")),
    binding(Scope::Ast, Key::Char('I'), Action::Begin("insert-before ")),
    binding(Scope::Ast, Key::Char('t'), Action::Begin("translate ")),
    binding(Scope::Ast, Key::Char('r'), Action::Begin("rotate ")),
    binding(Scope::Ast, Key::Char('s'), Action::Begin("scale ")),
    binding(Scope::Ast, Key::Char('d'), Action::Execute("delete")),
    binding(Scope::Ast, Key::Enter, Action::Execute("toggle")),
    binding(Scope::Other, Key::Enter, Action::Execute("toggle")),
    binding(Scope::EditorAny, Key::Char('j'), Action::Execute("next")),
    binding(Scope::EditorAny, Key::Down, Action::Execute("next")),
    binding(Scope::EditorAny, Key::Char('k'), Action::Execute("prev")),
    binding(Scope::EditorAny, Key::Up, Action::Execute("prev")),
    binding(
        Scope::EditorAny,
        Key::Char('h'),
        Action::Execute("collapse"),
    ),
    binding(Scope::EditorAny, Key::Left, Action::Execute("collapse")),
    binding(Scope::EditorAny, Key::Char('l'), Action::Execute("expand")),
    binding(Scope::EditorAny, Key::Right, Action::Execute("expand")),
    binding(Scope::EditorAny, Key::Char('u'), Action::Execute("undo")),
    control_binding(Scope::EditorAny, Key::Char('r'), Action::Execute("redo")),
    binding(Scope::EditorAny, Key::Char('w'), Action::SaveProject),
    binding(
        Scope::EditorAny,
        Key::Char('n'),
        Action::Begin("source new "),
    ),
    binding(
        Scope::EditorAny,
        Key::Char('e'),
        Action::Begin("source import "),
    ),
    binding(
        Scope::EditorAny,
        Key::Char('o'),
        Action::Begin("project open "),
    ),
    binding(
        Scope::EditorAny,
        Key::Char('L'),
        Action::Begin("library load "),
    ),
    binding(Scope::EditorAny, Key::Char(':'), Action::Begin("")),
    binding(Scope::EditorAny, Key::Char('q'), Action::Execute("quit")),
    control_binding(Scope::EditorAny, Key::Char('c'), Action::Execute("quit")),
    binding(Scope::EditorAny, Key::Char('?'), Action::Execute("help")),
    binding(
        Scope::EditorAny,
        Key::Char('P'),
        Action::Execute("model toggle"),
    ),
    binding(
        Scope::EditorAny,
        Key::Char('R'),
        Action::Execute("model render"),
    ),
    binding(Scope::Model, Key::Escape, Action::Execute("model close")),
    binding(Scope::Model, Key::Char('q'), Action::Execute("model close")),
    toolbar_binding(
        Scope::Model,
        Key::Char('P'),
        Action::Execute("model close"),
        ButtonLabel::Close,
    ),
    binding(
        Scope::Model,
        Key::Char('R'),
        Action::Execute("model render"),
    ),
    binding(Scope::Model, Key::Char(':'), Action::Begin("")),
    binding(
        Scope::AssemblyScreen,
        Key::Escape,
        Action::Execute("assembly close"),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('q'),
        Action::Execute("assembly close"),
    ),
    toolbar_binding(
        Scope::AssemblyScreen,
        Key::Char('P'),
        Action::Execute("assembly close"),
        ButtonLabel::Fixed("Source"),
    ),
    binding(Scope::AssemblyScreen, Key::Char(':'), Action::Begin("")),
    binding(
        Scope::AssemblyScreen,
        Key::Char('n'),
        Action::Begin("assembly new "),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('a'),
        Action::Begin("assembly add "),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('t'),
        Action::AssemblyTransform("translate"),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('r'),
        Action::AssemblyTransform("rotate"),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('s'),
        Action::AssemblyTransform("scale"),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('o'),
        Action::AssemblyTransform("pivot"),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('g'),
        Action::AssemblyParent,
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('e'),
        Action::Begin("assembly export "),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('j'),
        Action::Execute("assembly select next"),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('k'),
        Action::Execute("assembly select prev"),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('u'),
        Action::Execute("assembly undo"),
    ),
    control_binding(
        Scope::AssemblyScreen,
        Key::Char('r'),
        Action::Execute("assembly redo"),
    ),
    toolbar_binding(
        Scope::AssemblyScreen,
        Key::Char('R'),
        Action::Execute("assembly render"),
        ButtonLabel::Fixed("Render"),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('v'),
        Action::Execute("assembly select toggle"),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char(' '),
        Action::AssemblyVisibility,
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('y'),
        Action::Execute("assembly copy"),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('p'),
        Action::Execute("assembly paste"),
    ),
    binding(
        Scope::AssemblyScreen,
        Key::Char('d'),
        Action::Execute("assembly remove"),
    ),
    binding(
        Scope::Camera,
        Key::Char('h'),
        Action::Execute("camera orbit -5 0"),
    ),
    binding(
        Scope::Camera,
        Key::Char('l'),
        Action::Execute("camera orbit 5 0"),
    ),
    binding(
        Scope::Camera,
        Key::Char('j'),
        Action::Execute("camera orbit 0 -5"),
    ),
    binding(
        Scope::Camera,
        Key::Char('k'),
        Action::Execute("camera orbit 0 5"),
    ),
    binding(
        Scope::Camera,
        Key::Left,
        Action::Execute("camera pan -0.05 0"),
    ),
    binding(
        Scope::Camera,
        Key::Right,
        Action::Execute("camera pan 0.05 0"),
    ),
    binding(Scope::Camera, Key::Up, Action::Execute("camera pan 0 0.05")),
    binding(
        Scope::Camera,
        Key::Down,
        Action::Execute("camera pan 0 -0.05"),
    ),
    binding(
        Scope::Camera,
        Key::Char('+'),
        Action::Execute("camera zoom 0.85"),
    ),
    binding(
        Scope::Camera,
        Key::Char('='),
        Action::Execute("camera zoom 0.85"),
    ),
    binding(
        Scope::Camera,
        Key::Char('-'),
        Action::Execute("camera zoom 1.15"),
    ),
    toolbar_binding(
        Scope::Camera,
        Key::Char('f'),
        Action::Execute("camera fit"),
        ButtonLabel::Fixed("Fit"),
    ),
    toolbar_binding(
        Scope::Camera,
        Key::Char('p'),
        Action::Execute("camera projection toggle"),
        ButtonLabel::Projection,
    ),
    toolbar_binding(
        Scope::Camera,
        Key::Char('x'),
        Action::Execute("display axes toggle"),
        ButtonLabel::Axes,
    ),
    toolbar_binding(
        Scope::Camera,
        Key::Char('1'),
        Action::Execute("camera view front"),
        ButtonLabel::Fixed("Front"),
    ),
    binding(
        Scope::Camera,
        Key::Char('2'),
        Action::Execute("camera view back"),
    ),
    binding(
        Scope::Camera,
        Key::Char('3'),
        Action::Execute("camera view left"),
    ),
    binding(
        Scope::Camera,
        Key::Char('4'),
        Action::Execute("camera view right"),
    ),
    toolbar_binding(
        Scope::Camera,
        Key::Char('5'),
        Action::Execute("camera view top"),
        ButtonLabel::Fixed("Top"),
    ),
    binding(
        Scope::Camera,
        Key::Char('6'),
        Action::Execute("camera view bottom"),
    ),
    toolbar_binding(
        Scope::Camera,
        Key::Char('7'),
        Action::Execute("camera view iso"),
        ButtonLabel::Fixed("Iso"),
    ),
    toolbar_binding(
        Scope::Camera,
        Key::Char(' '),
        Action::Execute("camera auto-rotate toggle"),
        ButtonLabel::AutoRotate,
    ),
    binding(Scope::Camera, Key::Char('?'), Action::Execute("help")),
];

pub fn editor_context(app: &App) -> EditorContext {
    let selected = app.tree_state.borrow().selected().last().cloned();
    match selected.as_deref() {
        Some("__project_sources") => EditorContext::ProjectSources,
        Some("__assemblies") => EditorContext::Assemblies,
        Some(id) if id.starts_with("__project_source_") => id
            .trim_start_matches("__project_source_")
            .parse::<usize>()
            .ok()
            .and_then(|index| app.ast.embedded_sources.get(index))
            .filter(|source| source.editable)
            .map(|source| EditorContext::ProjectSource(source.virtual_path.clone()))
            .unwrap_or(EditorContext::Other),
        Some(id) if id.starts_with("__assembly_") => id
            .trim_start_matches("__assembly_")
            .parse::<usize>()
            .ok()
            .and_then(|index| app.assemblies.get(index))
            .map(|assembly| EditorContext::Assembly(assembly.id.clone()))
            .unwrap_or(EditorContext::Other),
        Some("__modules") => EditorContext::Ast,
        Some(id)
            if !id.starts_with("__")
                || id.starts_with("__var_")
                || id.starts_with("__func_")
                || id.starts_with("__moddef_") =>
        {
            EditorContext::Ast
        }
        _ => EditorContext::Other,
    }
}

pub fn resolve_editor_action(key: KeyEvent, app: &App) -> Option<KeyAction> {
    let context = editor_context(app);
    let scope = scope(&context);
    resolve_for_scope(key, app, scope, Some(&context))
}

pub fn resolve_key_action(key: KeyEvent, app: &App) -> Option<KeyAction> {
    match app.screen {
        Screen::Editor => resolve_editor_action(key, app),
        Screen::ModelPreview => resolve_for_scope(key, app, Scope::Model, None),
        Screen::Assembly => resolve_for_scope(key, app, Scope::AssemblyScreen, None),
    }
}

fn resolve_for_scope(
    key: KeyEvent,
    app: &App,
    active_scope: Scope,
    context: Option<&EditorContext>,
) -> Option<KeyAction> {
    let binding = BINDINGS.iter().find(|binding| {
        scope_matches(binding.scope, active_scope)
            && binding.control == key.modifiers.contains(KeyModifiers::CONTROL)
            && key_matches(binding.key, key.code)
    })?;
    materialize(binding.action, context, app)
}

fn scope_matches(binding: Scope, active: Scope) -> bool {
    binding == Scope::Global
        || binding == active
        || (binding == Scope::EditorAny
            && matches!(
                active,
                Scope::ProjectSources
                    | Scope::ProjectSource
                    | Scope::Assemblies
                    | Scope::AssemblyNode
                    | Scope::Ast
                    | Scope::Other
            ))
        || (binding == Scope::Camera && matches!(active, Scope::Model | Scope::AssemblyScreen))
}

fn scope(context: &EditorContext) -> Scope {
    match context {
        EditorContext::ProjectSources => Scope::ProjectSources,
        EditorContext::ProjectSource(_) => Scope::ProjectSource,
        EditorContext::Assemblies => Scope::Assemblies,
        EditorContext::Assembly(_) => Scope::AssemblyNode,
        EditorContext::Ast => Scope::Ast,
        EditorContext::Other => Scope::Other,
    }
}

fn key_matches(binding: Key, event: KeyCode) -> bool {
    matches!(
        (binding, event),
        (Key::Char(expected), KeyCode::Char(actual)) if expected == actual
    ) || matches!(
        (binding, event),
        (Key::Up, KeyCode::Up)
            | (Key::Down, KeyCode::Down)
            | (Key::Left, KeyCode::Left)
            | (Key::Right, KeyCode::Right)
            | (Key::Enter, KeyCode::Enter)
            | (Key::Escape, KeyCode::Esc)
    )
}

fn materialize(action: Action, context: Option<&EditorContext>, app: &App) -> Option<KeyAction> {
    let command = |command: String| Some(KeyAction::Execute(command));
    match action {
        Action::Execute(command_text) => command(command_text.to_string()),
        Action::Begin(command_text) => Some(KeyAction::BeginCommand(command_text.to_string())),
        Action::SaveProject => Some(KeyAction::BeginCommand(if app.current_file.is_some() {
            "project save".to_string()
        } else {
            "project save ".to_string()
        })),
        Action::OpenSelection => match context? {
            EditorContext::ProjectSource(source) => {
                command(format!("source switch {}", quote_command_argument(source)))
            }
            EditorContext::Assembly(assembly) => command(format!(
                "assembly open {}",
                quote_command_argument(assembly)
            )),
            _ => command("toggle".to_string()),
        },
        Action::RenameSource => match context? {
            EditorContext::ProjectSource(source) => Some(KeyAction::BeginCommand(format!(
                "source rename {} ",
                quote_command_argument(source)
            ))),
            _ => None,
        },
        Action::CopySource => source_command(context?, "copy"),
        Action::CutSource => source_command(context?, "cut"),
        Action::PasteSource => command("source paste".to_string()),
        Action::AssemblyTransform(operation) => {
            let Some(part) = selected_assembly_part(app) else {
                return Some(KeyAction::Error("No assembly part is selected".to_string()));
            };
            let values = match operation {
                "translate" => part.transform.translation,
                "rotate" => part.transform.rotation_degrees,
                "scale" => part.transform.scale,
                "pivot" => part.transform.pivot,
                _ => return None,
            };
            Some(KeyAction::BeginCommand(
                if app.selected_assembly_parts.is_empty() {
                    format!(
                        "assembly {operation} {} {} {} {}",
                        part.id, values[0], values[1], values[2]
                    )
                } else {
                    format!(
                        "assembly {operation} {} {} {}",
                        values[0], values[1], values[2]
                    )
                },
            ))
        }
        Action::AssemblyParent => {
            let Some(part) = selected_assembly_part(app) else {
                return Some(KeyAction::Error("No assembly part is selected".to_string()));
            };
            Some(KeyAction::BeginCommand(format!(
                "assembly parent {} {}",
                part.id,
                part.parent.as_deref().unwrap_or("root")
            )))
        }
        Action::AssemblyVisibility => command("assembly visibility toggle".to_string()),
    }
}

pub fn toolbar_buttons(app: &App) -> Vec<ToolbarButton> {
    let active_scope = match app.screen {
        Screen::ModelPreview => Scope::Model,
        Screen::Assembly => Scope::AssemblyScreen,
        Screen::Editor => return Vec::new(),
    };
    BINDINGS
        .iter()
        .filter(|binding| {
            binding.button.is_some()
                && scope_matches(binding.scope, active_scope)
                && (app.screen == Screen::ModelPreview
                    || matches!(binding.scope, Scope::AssemblyScreen | Scope::Camera))
        })
        .filter_map(|binding| {
            let KeyAction::Execute(command) = materialize(binding.action, None, app)? else {
                return None;
            };
            let shortcut = key_label(binding.key);
            let key_action = resolve_for_scope(
                KeyEvent::new(key_code(binding.key), KeyModifiers::NONE),
                app,
                active_scope,
                None,
            );
            Some(ToolbarButton {
                shortcut: if key_action == Some(KeyAction::Execute(command.clone())) {
                    shortcut.to_string()
                } else {
                    String::new()
                },
                label: button_label(binding.button?, app),
                command,
            })
        })
        .collect()
}

pub fn screen_shortcut_help(app: &App) -> &'static str {
    match app.screen {
        Screen::Assembly => {
            "j/k Focus  v Select  Space Visibility  u/C-r Undo  t/r/s Transform  : Command"
        }
        Screen::ModelPreview => "h/j/k/l Orbit  Arrows Pan  +/- Zoom  R Render  : Command",
        Screen::Editor => "",
    }
}

fn key_label(key: Key) -> &'static str {
    match key {
        Key::Char(' ') => "Space",
        Key::Char('P') => "P",
        Key::Char('R') => "R",
        Key::Char('f') => "f",
        Key::Char('p') => "p",
        Key::Char('v') => "v",
        Key::Char('x') => "x",
        Key::Char('y') => "y",
        Key::Char('1') => "1",
        Key::Char('5') => "5",
        Key::Char('7') => "7",
        _ => "",
    }
}

fn key_code(key: Key) -> KeyCode {
    match key {
        Key::Char(character) => KeyCode::Char(character),
        Key::Up => KeyCode::Up,
        Key::Down => KeyCode::Down,
        Key::Left => KeyCode::Left,
        Key::Right => KeyCode::Right,
        Key::Enter => KeyCode::Enter,
        Key::Escape => KeyCode::Esc,
    }
}

fn button_label(label: ButtonLabel, app: &App) -> String {
    let preview = match app.screen {
        Screen::Assembly => &app.assembly_preview,
        Screen::Editor | Screen::ModelPreview => &app.model_preview,
    };
    match label {
        ButtonLabel::Fixed(label) => label.to_string(),
        ButtonLabel::Close => match app.preview_close_action {
            crate::app::PreviewCloseAction::Source => "Source".to_string(),
            crate::app::PreviewCloseAction::Quit => "Quit".to_string(),
        },
        ButtonLabel::Projection => match preview.camera.projection {
            openscad_render::Projection::Perspective { .. } => "Ortho".to_string(),
            openscad_render::Projection::Orthographic { .. } => "Persp".to_string(),
        },
        ButtonLabel::Axes => if preview.axes_visible {
            "Axes-"
        } else {
            "Axes+"
        }
        .to_string(),
        ButtonLabel::AutoRotate => if preview.auto_rotate { "Stop" } else { "Auto" }.to_string(),
    }
}

fn selected_assembly_part(app: &App) -> Option<&openscad_assembly::PartInstance> {
    let active = app.active_assembly.as_deref()?;
    let selected = app.selected_assembly_part.as_deref()?;
    app.assemblies
        .iter()
        .find(|assembly| assembly.id == active || assembly.name == active)?
        .part(selected)
}

fn source_command(context: &EditorContext, action: &str) -> Option<KeyAction> {
    match context {
        EditorContext::ProjectSource(source) => Some(KeyAction::Execute(format!(
            "source {action} {}",
            quote_command_argument(source)
        ))),
        _ => None,
    }
}

fn quote_command_argument(value: &str) -> String {
    if value
        .chars()
        .all(|character| !character.is_whitespace() && !matches!(character, '\\' | '\'' | '"'))
    {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_source_bindings_override_ast_bindings() {
        let app = App::new();
        app.tree_state.borrow_mut().select(vec![
            "__project_sources".into(),
            "__project_source_0".into(),
        ]);

        assert_eq!(
            resolve_editor_action(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE), &app),
            Some(KeyAction::BeginCommand(
                "source rename main.scad ".to_string()
            ))
        );
        assert_eq!(
            resolve_editor_action(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE), &app),
            Some(KeyAction::Execute("source cut main.scad".to_string()))
        );
    }

    #[test]
    fn unsaved_project_save_binding_requests_a_path() {
        let app = App::new();
        assert_eq!(
            resolve_editor_action(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE), &app),
            Some(KeyAction::BeginCommand("project save ".to_string()))
        );
    }

    #[test]
    fn uppercase_q_force_quits_from_every_screen() {
        let mut app = App::new();
        for screen in [Screen::Editor, Screen::ModelPreview, Screen::Assembly] {
            app.screen = screen;
            assert_eq!(
                resolve_key_action(KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT), &app),
                Some(KeyAction::Execute("quit!".to_string()))
            );
        }
    }

    #[test]
    fn module_context_keeps_the_rotate_binding() {
        let mut app = App::new();
        app.ast_mut()
            .modules
            .push(openscad_core::ModuleNode::new_leaf(
                "cube_1".into(),
                "cube".into(),
                Vec::new(),
            ));
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".into(), "cube_1".into()]);

        assert_eq!(
            resolve_editor_action(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE), &app),
            Some(KeyAction::BeginCommand("rotate ".to_string()))
        );
    }

    #[test]
    fn model_toolbar_commands_are_resolved_from_their_key_bindings() {
        let mut app = App::new();
        app.enter_model_screen();

        for button in toolbar_buttons(&app) {
            let key = match button.shortcut.as_str() {
                "Space" => KeyCode::Char(' '),
                shortcut => KeyCode::Char(shortcut.chars().next().unwrap()),
            };
            assert_eq!(
                resolve_key_action(KeyEvent::new(key, KeyModifiers::NONE), &app),
                Some(KeyAction::Execute(button.command))
            );
        }
    }

    #[test]
    fn assembly_p_remains_paste_while_projection_is_a_button_only_control() {
        let mut app = App::new();
        app.screen = Screen::Assembly;

        assert_eq!(
            resolve_key_action(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE), &app),
            Some(KeyAction::Execute("assembly paste".to_string()))
        );
        assert_eq!(
            resolve_key_action(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE), &app),
            Some(KeyAction::Execute("assembly select toggle".to_string()))
        );
        assert_eq!(
            resolve_key_action(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE), &app),
            Some(KeyAction::Execute("assembly visibility toggle".to_string()))
        );
        assert_eq!(
            resolve_key_action(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE), &app),
            Some(KeyAction::Execute("assembly undo".to_string()))
        );
        assert_eq!(
            resolve_key_action(
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
                &app
            ),
            Some(KeyAction::Execute("assembly redo".to_string()))
        );
        let buttons = toolbar_buttons(&app);
        let projection = buttons
            .iter()
            .find(|button| button.command == "camera projection toggle")
            .expect("assembly toolbar should expose projection");
        assert!(projection.shortcut.is_empty());
        let auto_rotate = buttons
            .iter()
            .find(|button| button.command == "camera auto-rotate toggle")
            .expect("assembly toolbar should expose automatic rotation");
        assert!(auto_rotate.shortcut.is_empty());
        assert!(!buttons.iter().any(|button| matches!(
            button.command.as_str(),
            "assembly visibility toggle" | "assembly copy" | "assembly paste"
        )));
    }

    #[test]
    fn assembly_camera_button_labels_follow_preview_state() {
        let mut app = App::new();
        app.screen = Screen::Assembly;
        app.assembly_preview.camera.projection = openscad_render::Projection::Orthographic {
            vertical_size: 10.0,
        };
        app.assembly_preview.axes_visible = false;
        app.assembly_preview.auto_rotate = true;

        let labels = toolbar_buttons(&app)
            .into_iter()
            .map(|button| button.label)
            .collect::<Vec<_>>();
        assert!(labels.iter().any(|label| label == "Persp"));
        assert!(labels.iter().any(|label| label == "Axes+"));
        assert!(labels.iter().any(|label| label == "Stop"));
    }
}
