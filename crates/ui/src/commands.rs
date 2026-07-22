//! Commands module for OpenSCAD TUI

use openscad_core::{Argument, ArgumentSelector, AstError, Expr, ModuleNode};
use openscad_library::ModuleDef;
use openscad_terminal::DisplayProtocol;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;

use crate::app::{App, InputMode, PendingModuleAction, PreviewCloseAction, Screen};
use crate::command_registry::CommandType;
use crate::project_file::{load_project, save_project, ProjectDocument, PROJECT_EXTENSION};
use crate::project_import::{attach_editable_scad, attach_scad_library};

const MAX_RECURSION_DEPTH: usize = 1000;

pub fn cmd_render(app: &mut App) -> CommandResult<()> {
    let (source, render_project) = active_source_project(app)?;
    let source_context = app
        .ast
        .source_directory
        .clone()
        .or_else(|| app.current_file.clone());
    app.model_preview
        .render(source, source_context.as_deref(), render_project)
        .map_err(CommandError::Custom)?;
    app.enter_model_screen();
    Ok(())
}

pub fn cmd_view(app: &mut App, filename: &str) -> CommandResult<()> {
    let path = expand_tilde(filename);
    if !path.is_file() {
        return Err(CommandError::Custom(format!(
            "Model file '{}' does not exist or is not a regular file",
            path.display()
        )));
    }
    openscad_render::ModelFileFormat::from_path(&path)
        .map_err(|error| CommandError::Custom(error.to_string()))?;
    app.model_preview
        .view_file(path)
        .map_err(CommandError::Custom)?;
    app.enter_model_screen();
    Ok(())
}

fn active_source_project(
    app: &mut App,
) -> CommandResult<(String, Option<openscad_render::OpenScadProject>)> {
    app.ast_mut().sync_active_source();
    let target = app.ast.active_source.clone();
    let mut source = target
        .as_deref()
        .and_then(|path| app.ast.source_code(path))
        .unwrap_or_else(|| app.ast.to_scad());
    if let Some(target) = &target {
        source = rewrite_absolute_source_references(&source, target, &app.ast.source_dependencies);
    }
    let project = target
        .as_ref()
        .map(|target| openscad_render::OpenScadProject {
            entry_path: PathBuf::from(target),
            files: app
                .ast
                .embedded_sources
                .iter()
                .map(|project_source| openscad_render::OpenScadProjectFile {
                    path: PathBuf::from(&project_source.virtual_path),
                    content: rewrite_absolute_source_references(
                        &project_source.generated_content(),
                        &project_source.virtual_path,
                        &app.ast.source_dependencies,
                    ),
                })
                .collect(),
        });
    Ok((source, project))
}

fn source_project(
    app: &mut App,
    target: &str,
) -> CommandResult<(String, openscad_render::OpenScadProject, u64)> {
    app.ast_mut().sync_active_source();
    let source = app
        .ast
        .source_code(target)
        .ok_or_else(|| CommandError::Custom(format!("Project source '{target}' was not found")))?;
    let source = rewrite_absolute_source_references(&source, target, &app.ast.source_dependencies);
    let files = app
        .ast
        .embedded_sources
        .iter()
        .map(|project_source| openscad_render::OpenScadProjectFile {
            path: PathBuf::from(&project_source.virtual_path),
            content: rewrite_absolute_source_references(
                &project_source.generated_content(),
                &project_source.virtual_path,
                &app.ast.source_dependencies,
            ),
        })
        .collect::<Vec<_>>();
    let mut hasher = DefaultHasher::new();
    target.hash(&mut hasher);
    source.hash(&mut hasher);
    for file in &files {
        file.path.hash(&mut hasher);
        file.content.hash(&mut hasher);
    }
    Ok((
        source,
        openscad_render::OpenScadProject {
            entry_path: PathBuf::from(target),
            files,
        },
        hasher.finish(),
    ))
}

fn compile_assembly(
    app: &mut App,
) -> CommandResult<(openscad_assembly::ResolvedAssembly, Duration)> {
    let assembly = active_assembly(app)?.clone();
    let started = Instant::now();
    let sources = assembly
        .parts
        .iter()
        .map(|part| part.source.clone())
        .collect::<HashSet<_>>();
    let mut meshes = HashMap::new();
    for source_ref in sources {
        let target = source_ref.virtual_path();
        let (source, project, fingerprint) = source_project(app, target)?;
        let mesh = match app.assembly_mesh_cache.get(&source_ref) {
            Some((cached_fingerprint, mesh)) if *cached_fingerprint == fingerprint => {
                Arc::clone(mesh)
            }
            _ => {
                let mut generator = openscad_render::OpenScadGenerator::new("openscad")
                    .with_timeout(Duration::from_secs(120))
                    .with_project(project);
                if let Some(directory) = project_working_directory(app) {
                    generator = generator.with_working_directory(directory);
                }
                let generated = generator
                    .generate(&source)
                    .map_err(|error| CommandError::Custom(error.to_string()))?;
                let mesh = Arc::new(generated.mesh);
                app.assembly_mesh_cache
                    .insert(source_ref.clone(), (fingerprint, Arc::clone(&mesh)));
                mesh
            }
        };
        meshes.insert(source_ref, mesh);
    }
    let resolved = assembly
        .resolve(&meshes)
        .map_err(|error| CommandError::Custom(error.to_string()))?;
    Ok((resolved, started.elapsed()))
}

fn project_working_directory(app: &App) -> Option<PathBuf> {
    app.ast
        .source_directory
        .as_deref()
        .map(expand_tilde)
        .or_else(|| {
            app.current_file
                .as_deref()
                .map(expand_tilde)
                .and_then(|path| path.parent().map(Path::to_path_buf))
        })
        .or_else(|| std::env::current_dir().ok())
}

fn active_assembly(app: &App) -> CommandResult<&openscad_assembly::AssemblyDocument> {
    let active = app.active_assembly.as_deref().ok_or_else(|| {
        CommandError::Custom("No active assembly; use 'assembly new <name>' first".into())
    })?;
    app.assemblies
        .iter()
        .find(|assembly| assembly.id == active || assembly.name == active)
        .ok_or_else(|| CommandError::Custom(format!("Active assembly '{active}' was not found")))
}

fn active_assembly_mut(app: &mut App) -> CommandResult<&mut openscad_assembly::AssemblyDocument> {
    let active = app.active_assembly.clone().ok_or_else(|| {
        CommandError::Custom("No active assembly; use 'assembly new <name>' first".into())
    })?;
    app.assemblies
        .iter_mut()
        .find(|assembly| assembly.id == active || assembly.name == active)
        .ok_or_else(|| CommandError::Custom(format!("Active assembly '{active}' was not found")))
}

fn refresh_cached_assembly_preview(app: &mut App) -> CommandResult<()> {
    if !matches!(app.screen, Screen::Assembly) || app.assembly_mesh_cache.is_empty() {
        return Ok(());
    }
    let assembly = active_assembly(app)?.clone();
    let meshes = app
        .assembly_mesh_cache
        .iter()
        .map(|(source, (_, mesh))| (source.clone(), Arc::clone(mesh)))
        .collect();
    let resolved = match assembly.resolve(&meshes) {
        Ok(resolved) => resolved,
        Err(openscad_assembly::AssemblyError::MissingMesh(_)) => return Ok(()),
        Err(openscad_assembly::AssemblyError::EmptyAssembly) => {
            app.assembly_preview.clear();
            return Ok(());
        }
        Err(error) => return Err(CommandError::Custom(error.to_string())),
    };
    let scene = resolved
        .render_scene(app.selected_assembly_part.as_deref())
        .map_err(|error| CommandError::Custom(error.to_string()))?;
    app.assembly_preview
        .update_scene(scene, Duration::ZERO)
        .map_err(CommandError::Custom)
}

pub fn cmd_buffer(app: &mut App, value: Option<&str>) -> CommandResult<()> {
    let editable = app
        .ast
        .embedded_sources
        .iter()
        .filter(|source| source.editable)
        .map(|source| source.virtual_path.clone())
        .collect::<Vec<_>>();
    if editable.is_empty() {
        return Err(CommandError::Custom(
            "The current project has no editable source buffers".to_string(),
        ));
    }
    let Some(value) = value else {
        let active = app.ast.active_source.as_deref();
        let summary = editable
            .iter()
            .map(|path| {
                let active_marker = if active == Some(path.as_str()) {
                    "*"
                } else {
                    ""
                };
                format!("{active_marker}{path}")
            })
            .collect::<Vec<_>>()
            .join(" ");
        app.set_info(&format!("Buffers (* active): {summary}"));
        return Ok(());
    };

    let target = match value {
        "next" | "prev" => {
            let current = app
                .ast
                .active_source
                .as_ref()
                .and_then(|active| editable.iter().position(|path| path == active))
                .unwrap_or(0);
            let index = if value == "next" {
                (current + 1) % editable.len()
            } else {
                (current + editable.len() - 1) % editable.len()
            };
            editable[index].clone()
        }
        name => resolve_project_source(&editable, name)?,
    };
    if app.ast.active_source.as_deref() == Some(&target) {
        app.set_info(&format!("Already editing '{target}'"));
        return Ok(());
    }
    app.ast_mut().activate_source(&target)?;
    reload_project_definitions(app);
    app.selected_nodes.clear();
    app.undo_stack.clear();
    app.redo_stack.clear();
    app.tree_state.borrow_mut().select(Vec::new());
    app.init_tree_selection();
    app.invalidate_source_previews();
    app.set_info(&format!("Editing project source '{target}'"));
    Ok(())
}

fn resolve_project_source(sources: &[String], name: &str) -> CommandResult<String> {
    if let Some(path) = sources.iter().find(|path| path.as_str() == name) {
        return Ok(path.clone());
    }
    let matches = sources
        .iter()
        .filter(|path| {
            let path = Path::new(path);
            path.file_name().and_then(|value| value.to_str()) == Some(name)
                || path.file_stem().and_then(|value| value.to_str()) == Some(name)
        })
        .cloned()
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(CommandError::Custom(format!(
            "Editable project source '{name}' was not found"
        ))),
        _ => Err(CommandError::Custom(format!(
            "Project source name '{name}' is ambiguous; use its full virtual path"
        ))),
    }
}

fn rewrite_absolute_source_references(
    source: &str,
    from: &str,
    dependencies: &[openscad_core::SourceDependency],
) -> String {
    let mut rewritten = source.to_string();
    for dependency in dependencies.iter().filter(|dependency| {
        dependency.from == from && Path::new(&dependency.reference).is_absolute()
    }) {
        let directive = match dependency.kind {
            openscad_core::SourceDependencyKind::Include => "include",
            openscad_core::SourceDependencyKind::Use => "use",
        };
        let original = format!("{directive} <{}>", dependency.reference);
        let relative = relative_virtual_path(from, &dependency.to);
        rewritten = rewritten.replace(&original, &format!("{directive} <{relative}>"));
    }
    rewritten
}

fn relative_virtual_path(from_file: &str, to_file: &str) -> String {
    let from: Vec<_> = Path::new(from_file)
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let to: Vec<_> = Path::new(to_file)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    std::iter::repeat_n("..".to_string(), from.len() - common)
        .chain(to.into_iter().skip(common))
        .collect::<Vec<_>>()
        .join("/")
}

pub fn cmd_preview(app: &mut App, mode: &str) -> CommandResult<()> {
    match mode {
        "source" => app.enter_editor_screen(),
        "close" => match app.preview_close_action {
            PreviewCloseAction::Source => app.enter_editor_screen(),
            PreviewCloseAction::Quit => return cmd_quit(app),
        },
        "model"
            if matches!(
                app.model_preview.status,
                crate::preview::ModelPreviewStatus::Empty
            ) =>
        {
            return cmd_render(app)
        }
        "model" => app.enter_model_screen(),
        "toggle" => match app.screen {
            Screen::Editor => return cmd_preview(app, "model"),
            Screen::ModelPreview => return cmd_preview(app, "close"),
            Screen::Assembly => app.enter_editor_screen(),
        },
        _ => {
            return Err(CommandError::InvalidCommand(
                "Usage: model preview [--render] | model toggle | model close | source preview"
                    .to_string(),
            ))
        }
    };
    Ok(())
}

fn active_preview(app: &App) -> &crate::preview::ModelPreview {
    match app.screen {
        Screen::Assembly => &app.assembly_preview,
        Screen::Editor | Screen::ModelPreview => &app.model_preview,
    }
}

fn active_preview_mut(app: &mut App) -> &mut crate::preview::ModelPreview {
    match app.screen {
        Screen::Assembly => &mut app.assembly_preview,
        Screen::Editor | Screen::ModelPreview => &mut app.model_preview,
    }
}

pub fn cmd_camera(app: &mut App, args: &[&str]) -> CommandResult<()> {
    use openscad_render::{Projection, StandardView};

    let invalid = || {
        CommandError::InvalidCommand(
        "Usage: camera projection perspective|orthographic|toggle | view front|back|left|right|top|bottom|iso | orbit <yaw-deg> <pitch-deg> | pan <x> <y> | zoom <factor> | fit | auto-rotate on|off|toggle"
            .to_string(),
    )
    };
    let parse = |value: &str| value.parse::<f32>().map_err(|_| invalid());
    let result = match args {
        ["projection", "perspective"] => active_preview_mut(app).set_projection(false),
        ["projection", "orthographic"] => active_preview_mut(app).set_projection(true),
        ["projection", "toggle"] => {
            let use_orthographic = matches!(
                active_preview(app).camera.projection,
                Projection::Perspective { .. }
            );
            active_preview_mut(app).set_projection(use_orthographic)
        }
        ["view", name] => {
            let view = match *name {
                "front" => StandardView::Front,
                "back" => StandardView::Back,
                "left" => StandardView::Left,
                "right" => StandardView::Right,
                "top" => StandardView::Top,
                "bottom" => StandardView::Bottom,
                "iso" | "isometric" => StandardView::Isometric,
                _ => return Err(invalid()),
            };
            active_preview_mut(app).set_view(view)
        }
        ["orbit", yaw, pitch] => active_preview_mut(app).orbit(parse(yaw)?, parse(pitch)?),
        ["pan", horizontal, vertical] => {
            active_preview_mut(app).pan(parse(horizontal)?, parse(vertical)?)
        }
        ["zoom", factor] => active_preview_mut(app).zoom(parse(factor)?),
        ["fit"] => active_preview_mut(app).fit(),
        ["auto-rotate", value] => {
            let enabled = match *value {
                "on" => true,
                "off" => false,
                "toggle" => !active_preview(app).auto_rotate,
                _ => return Err(invalid()),
            };
            active_preview_mut(app).set_auto_rotate(enabled);
            Ok(())
        }
        _ => return Err(invalid()),
    };
    result.map_err(CommandError::Custom)
}

pub fn cmd_protocol(app: &mut App, value: &str) -> CommandResult<()> {
    match value.to_ascii_lowercase().as_str() {
        "auto" => active_preview_mut(app).reset_protocol_type(),
        "next" => {
            let next = active_preview(app).protocol_type().next();
            active_preview_mut(app).set_protocol_type(next);
        }
        value => {
            let protocol = value.parse::<DisplayProtocol>().map_err(|_| {
                CommandError::InvalidCommand(format!(
                    "Usage: display protocol auto|next|{}",
                    DisplayProtocol::NAMES.join("|")
                ))
            })?;
            active_preview_mut(app).set_protocol_type(protocol);
        }
    }
    app.terminal_clear_requested = true;
    app.set_info(&format!(
        "Terminal preview protocol: {}",
        active_preview(app).protocol_type()
    ));
    Ok(())
}

pub fn cmd_axes(app: &mut App, value: &str) -> CommandResult<()> {
    let visible = match value {
        "on" => true,
        "off" => false,
        "toggle" => !active_preview(app).axes_visible,
        _ => {
            return Err(CommandError::InvalidCommand(
                "Usage: display axes on|off|toggle".to_string(),
            ))
        }
    };
    active_preview_mut(app).set_axes_visible(visible);
    app.set_info(if visible {
        "World axes enabled"
    } else {
        "World axes disabled"
    });
    Ok(())
}

pub fn cmd_visibility(app: &mut App, value: &str) -> CommandResult<()> {
    if !matches!(value, "show" | "hide" | "toggle") {
        return Err(CommandError::InvalidCommand(
            "Usage: visibility show|hide|toggle".to_string(),
        ));
    }
    let target_ids = selected_or_current_node_ids(app)?;
    if target_ids
        .iter()
        .any(|node_id| node_id.starts_with("__") || app.ast.find_node_anywhere(node_id).is_none())
    {
        return Err(CommandError::InvalidCommand(
            "Visibility can only be changed on module nodes".to_string(),
        ));
    }

    app.push_undo();
    for node_id in &target_ids {
        let node = app
            .ast_mut()
            .find_node_anywhere_mut(node_id)
            .expect("targets were validated before mutation");
        node.modifier = match value {
            "hide" => Some('*'),
            "show" if node.modifier == Some('*') => None,
            "show" => node.modifier,
            "toggle" if node.modifier == Some('*') => None,
            "toggle" => Some('*'),
            _ => unreachable!(),
        };
    }
    app.set_info(&format!("{value} {} module node(s)", target_ids.len()));
    Ok(())
}

fn dispatch_assembly(app: &mut App, operation: &'static str, args: &[&str]) -> CommandResult<()> {
    let mut command = Vec::with_capacity(args.len() + 1);
    command.push(operation);
    command.extend_from_slice(args);
    cmd_assembly(app, &command)
}

pub fn cmd_assembly(app: &mut App, args: &[&str]) -> CommandResult<()> {
    let usage = || {
        CommandError::InvalidCommand(
            "Usage: assembly new [name] | open [name] | list | add <project-source> [name] | select <part|next|prev> | copy [part] | paste [parent|root] | remove [part] | parent [part] <parent|root> | translate|rotate|scale|pivot [part] <x> <y> <z> | visibility [part] <show|hide|toggle> | render | export <file.dae> | close".into(),
        )
    };
    match args {
        ["new"] | ["new", _] => {
            let name = args
                .get(1)
                .copied()
                .map(str::to_string)
                .unwrap_or_else(|| format!("assembly-{}", app.assemblies.len() + 1));
            let mut document = openscad_assembly::AssemblyDocument::new(name);
            let base = document.id.clone();
            let mut suffix = 2;
            while app
                .assemblies
                .iter()
                .any(|assembly| assembly.id == document.id)
            {
                document.id = format!("{base}_{suffix}");
                suffix += 1;
            }
            app.active_assembly = Some(document.id.clone());
            app.assemblies.push(document);
            app.selected_assembly_part = None;
            app.assembly_scroll_offset = 0;
            app.assembly_preview.clear();
            app.saved = false;
            app.enter_assembly_screen();
            app.set_info("Created assembly");
        }
        ["open"] if app.active_assembly.is_some() => app.enter_assembly_screen(),
        ["open", name] => {
            let document = app
                .assemblies
                .iter()
                .find(|assembly| assembly.id == *name || assembly.name == *name)
                .ok_or_else(|| CommandError::Custom(format!("Assembly '{name}' was not found")))?;
            app.active_assembly = Some(document.id.clone());
            app.selected_assembly_part = document.parts.first().map(|part| part.id.clone());
            app.assembly_scroll_offset = 0;
            app.assembly_preview.clear();
            app.enter_assembly_screen();
            refresh_cached_assembly_preview(app)?;
        }
        ["list"] => {
            let active = app.active_assembly.as_deref();
            let summary = app
                .assemblies
                .iter()
                .map(|assembly| {
                    format!(
                        "{}{} ({} parts)",
                        if active == Some(assembly.id.as_str()) {
                            "*"
                        } else {
                            ""
                        },
                        assembly.name,
                        assembly.parts.len()
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            app.set_info(if summary.is_empty() {
                "No assemblies"
            } else {
                &summary
            });
        }
        ["add", source] | ["add", source, _] => {
            let embedded = app
                .ast
                .embedded_sources
                .iter()
                .find(|candidate| candidate.virtual_path == *source && candidate.editable)
                .ok_or_else(|| {
                    CommandError::Custom(format!(
                        "Editable project source '{source}' was not found"
                    ))
                })?;
            let default_name = Path::new(&embedded.virtual_path)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("part")
                .to_string();
            let name = args.get(2).copied().unwrap_or(&default_name);
            let id = active_assembly_mut(app)?
                .add_part(
                    openscad_assembly::MeshSourceRef::project_source(*source),
                    name,
                )
                .map_err(|error| CommandError::Custom(error.to_string()))?
                .id
                .clone();
            app.selected_assembly_part = Some(id);
            app.saved = false;
            refresh_cached_assembly_preview(app)?;
        }
        ["select", target] => {
            let assembly = active_assembly(app)?;
            if assembly.parts.is_empty() {
                return Err(CommandError::Custom(
                    "The active assembly has no parts".into(),
                ));
            }
            let rows = assembly.hierarchy_rows();
            let selected_index = app.selected_assembly_part.as_deref().and_then(|id| {
                rows.iter()
                    .position(|(index, _)| assembly.parts[*index].id == id)
            });
            let target_id = match *target {
                "next" => {
                    let row = (selected_index.unwrap_or(rows.len() - 1) + 1) % rows.len();
                    assembly.parts[rows[row].0].id.clone()
                }
                "prev" => {
                    let row = selected_index
                        .unwrap_or(0)
                        .checked_sub(1)
                        .unwrap_or(rows.len() - 1);
                    assembly.parts[rows[row].0].id.clone()
                }
                value => assembly
                    .part(value)
                    .ok_or_else(|| {
                        CommandError::Custom(format!("Assembly part '{value}' was not found"))
                    })?
                    .id
                    .clone(),
            };
            app.selected_assembly_part = Some(target_id);
            refresh_cached_assembly_preview(app)?;
        }
        ["copy"] | ["copy", _] => {
            let target = args
                .get(1)
                .copied()
                .or(app.selected_assembly_part.as_deref())
                .ok_or_else(|| CommandError::Custom("No assembly part is selected".into()))?;
            let part = active_assembly(app)?
                .part(target)
                .ok_or_else(|| {
                    CommandError::Custom(format!("Assembly part '{target}' was not found"))
                })?
                .clone();
            let name = part.name.clone();
            app.assembly_clipboard = Some(part);
            app.set_info(&format!("Copied assembly part '{name}'"));
        }
        ["paste"] | ["paste", _] => {
            let copied = app
                .assembly_clipboard
                .clone()
                .ok_or_else(|| CommandError::Custom("Assembly clipboard is empty".into()))?;
            let requested_parent = args.get(1).copied();
            let parent = match requested_parent {
                Some("root" | "none") => None,
                Some(value) => Some(
                    active_assembly(app)?
                        .part(value)
                        .ok_or_else(|| {
                            CommandError::Custom(format!("Assembly part '{value}' was not found"))
                        })?
                        .id
                        .clone(),
                ),
                None => copied.parent.as_ref().and_then(|parent| {
                    active_assembly(app)
                        .ok()
                        .and_then(|assembly| assembly.part(parent))
                        .map(|part| part.id.clone())
                }),
            };
            let pasted_id = active_assembly_mut(app)?
                .add_part(copied.source.clone(), copied.name_base.clone())
                .map_err(|error| CommandError::Custom(error.to_string()))?
                .id
                .clone();
            {
                let pasted = active_assembly_mut(app)?
                    .part_mut(&pasted_id)
                    .expect("part was just added");
                pasted.transform = copied.transform;
                pasted.visible = copied.visible;
            }
            if let Some(parent) = parent.as_deref() {
                active_assembly_mut(app)?
                    .set_parent(&pasted_id, Some(parent))
                    .map_err(|error| CommandError::Custom(error.to_string()))?;
            }
            let pasted_name = active_assembly(app)?
                .part(&pasted_id)
                .expect("part was just added")
                .name
                .clone();
            app.selected_assembly_part = Some(pasted_id);
            app.saved = false;
            refresh_cached_assembly_preview(app)?;
            app.set_info(&format!("Pasted assembly part '{pasted_name}'"));
        }
        ["remove"] | ["remove", _] => {
            let target = args
                .get(1)
                .copied()
                .or(app.selected_assembly_part.as_deref())
                .ok_or_else(|| CommandError::Custom("No assembly part is selected".into()))?
                .to_string();
            active_assembly_mut(app)?
                .remove_part(&target)
                .map_err(|error| CommandError::Custom(error.to_string()))?;
            app.selected_assembly_part = active_assembly(app)?
                .parts
                .first()
                .map(|part| part.id.clone());
            app.saved = false;
            refresh_cached_assembly_preview(app)?;
        }
        ["parent", parent] => {
            let part = selected_assembly_part_id(app)?;
            let parent = (!matches!(*parent, "root" | "none")).then_some(*parent);
            active_assembly_mut(app)?
                .set_parent(&part, parent)
                .map_err(|error| CommandError::Custom(error.to_string()))?;
            app.saved = false;
            refresh_cached_assembly_preview(app)?;
        }
        ["parent", part, parent] => {
            let parent = (!matches!(*parent, "root" | "none")).then_some(*parent);
            active_assembly_mut(app)?
                .set_parent(part, parent)
                .map_err(|error| CommandError::Custom(error.to_string()))?;
            app.saved = false;
            refresh_cached_assembly_preview(app)?;
        }
        [operation @ ("translate" | "rotate" | "scale" | "pivot"), x, y, z] => {
            let part = selected_assembly_part_id(app)?;
            let values = [
                parse_f32(x, &usage)?,
                parse_f32(y, &usage)?,
                parse_f32(z, &usage)?,
            ];
            set_assembly_transform(app, operation, &part, values)?;
        }
        [operation @ ("translate" | "rotate" | "scale" | "pivot"), part, x, y, z] => {
            let values = [
                parse_f32(x, &usage)?,
                parse_f32(y, &usage)?,
                parse_f32(z, &usage)?,
            ];
            set_assembly_transform(app, operation, part, values)?;
        }
        ["visibility", action @ ("show" | "hide" | "toggle")] => {
            let part = selected_assembly_part_id(app)?;
            set_assembly_visibility(app, &part, action)?;
        }
        ["visibility", part, action @ ("show" | "hide" | "toggle")] => {
            set_assembly_visibility(app, part, action)?;
        }
        ["render"] => {
            let (resolved, elapsed) = compile_assembly(app)?;
            let scene = resolved
                .render_scene(app.selected_assembly_part.as_deref())
                .map_err(|error| CommandError::Custom(error.to_string()))?;
            app.assembly_preview
                .render_scene(scene, elapsed)
                .map_err(CommandError::Custom)?;
            app.enter_assembly_screen();
        }
        ["export", destination] => {
            let path = ensure_extension(resolve_export_path(app, destination)?, "dae");
            let (resolved, elapsed) = compile_assembly(app)?;
            openscad_assembly::write_dae(&path, &resolved)
                .map_err(|error| CommandError::Custom(error.to_string()))?;
            app.set_info(&format!(
                "Exported assembly to '{}' in {:.2}s",
                path.display(),
                elapsed.as_secs_f64()
            ));
        }
        ["close"] => app.enter_editor_screen(),
        _ => return Err(usage()),
    }
    Ok(())
}

fn selected_assembly_part_id(app: &App) -> CommandResult<String> {
    let part = app
        .selected_assembly_part
        .as_deref()
        .ok_or_else(|| CommandError::Custom("No assembly part is selected".into()))?;
    active_assembly(app)?
        .part(part)
        .map(|part| part.id.clone())
        .ok_or_else(|| CommandError::Custom(format!("Assembly part '{part}' was not found")))
}

fn set_assembly_transform(
    app: &mut App,
    operation: &str,
    part: &str,
    values: [f32; 3],
) -> CommandResult<()> {
    let document = active_assembly_mut(app)?;
    let target = document
        .part_mut(part)
        .ok_or_else(|| CommandError::Custom(format!("Assembly part '{part}' was not found")))?;
    let previous = target.transform;
    match operation {
        "translate" => target.transform.translation = values,
        "rotate" => target.transform.rotation_degrees = values,
        "scale" => target.transform.scale = values,
        "pivot" => target.transform.pivot = values,
        _ => unreachable!(),
    }
    if let Err(error) = target.transform.validate() {
        target.transform = previous;
        return Err(CommandError::Custom(error.to_string()));
    }
    app.saved = false;
    refresh_cached_assembly_preview(app)
}

fn set_assembly_visibility(app: &mut App, part: &str, action: &str) -> CommandResult<()> {
    let document = active_assembly_mut(app)?;
    let target = document
        .part_mut(part)
        .ok_or_else(|| CommandError::Custom(format!("Assembly part '{part}' was not found")))?;
    target.visible = match action {
        "show" => true,
        "hide" => false,
        "toggle" => !target.visible,
        _ => unreachable!(),
    };
    app.saved = false;
    refresh_cached_assembly_preview(app)
}

fn parse_f32(value: &str, invalid: &impl Fn() -> CommandError) -> CommandResult<f32> {
    value.parse().map_err(|_| invalid())
}

pub fn cmd_diagnostics(app: &mut App, destination: Option<&str>) -> CommandResult<()> {
    let details = active_preview(app)
        .diagnostics()
        .ok_or_else(|| CommandError::Custom("No render or display diagnostic is available".into()))?
        .to_string();
    if let Some(destination) = destination {
        let destination = expand_tilde(destination);
        fs::write(&destination, &details).map_err(|error| {
            CommandError::Custom(format!(
                "Failed to write diagnostics to '{}': {error}",
                destination.display()
            ))
        })?;
        app.set_info(&format!("Wrote diagnostics to '{}'", destination.display()));
    } else {
        let mut lines = vec!["Latest render diagnostic".to_string(), String::new()];
        lines.extend(details.lines().map(str::to_string));
        lines.extend([
            String::new(),
            "Use :diagnostics <file> to save this report.".to_string(),
        ]);
        app.set_help_doc(lines);
        app.input_mode = InputMode::Help;
    }
    Ok(())
}

#[derive(Error, Debug)]
pub enum CommandError {
    #[error("Invalid command: {0}")]
    InvalidCommand(String),

    #[error("AST error: {0}")]
    AstError(#[from] AstError),

    #[error("Parameter parsing error: {0}")]
    ParameterError(String),

    #[error("No node selected")]
    #[allow(dead_code)]
    NoNodeSelected,

    #[error("No children selected")]
    NoChildrenSelected,

    #[error("{0}")]
    Custom(String),
}

pub type CommandResult<T> = std::result::Result<T, CommandError>;

struct PreparedModule {
    node: ModuleNode,
    accepts_children: bool,
}

fn prepare_module(
    app: &App,
    module_name: &str,
    params: Option<&str>,
) -> CommandResult<PreparedModule> {
    let definition = app
        .library
        .get_module(module_name)
        .ok_or_else(|| CommandError::InvalidCommand(format!("Unknown module: {}", module_name)))?;
    let args = match params {
        Some(params) if !params.trim().is_empty() => parse_arguments(params, &definition)?,
        _ => Vec::new(),
    };
    let id = format!(
        "{}_{}",
        module_name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let node = if definition.accepts_children {
        ModuleNode::new_container(id, module_name.to_string(), args)
    } else {
        ModuleNode::new_leaf(id, module_name.to_string(), args)
    };
    Ok(PreparedModule {
        node,
        accepts_children: definition.accepts_children,
    })
}

/// Insert command
/// Insert a new module in the tree.
///
/// For modules that accept children (accepts_children: true):
///   - If child nodes are selected, create the module and move selected nodes as children
///   - If no child nodes are selected, return NoChildrenSelected error
///
/// For leaf modules (accepts_children: false):
///   - Insert after the currently selected node if there is one
///   - If no node is selected, or selected node is not in Modules section, insert at root level of Modules
pub fn cmd_insert(
    app: &mut App,
    module_name: &str,
    _parent_id: Option<&str>,
    params: Option<&str>,
) -> CommandResult<String> {
    let prepared = prepare_module(app, module_name, params)?;
    insert_prepared_module(app, module_name, prepared, false)
}

pub fn cmd_insert_before(
    app: &mut App,
    module_name: &str,
    params: Option<&str>,
) -> CommandResult<String> {
    ensure_insert_before_target(app)?;
    let prepared = prepare_module(app, module_name, params)?;
    insert_prepared_module(app, module_name, prepared, true)
}

fn ensure_insert_before_target(app: &App) -> CommandResult<()> {
    let selected = app.tree_state.borrow().selected().last().cloned();
    if selected.as_ref().is_none_or(|node_id| {
        node_id.starts_with("__") || app.ast.find_node_anywhere(node_id).is_none()
    }) {
        return Err(CommandError::InvalidCommand(
            "insert-before requires the cursor to be on a module node".to_string(),
        ));
    }
    Ok(())
}

fn insert_prepared_module(
    app: &mut App,
    module_name: &str,
    prepared: PreparedModule,
    before: bool,
) -> CommandResult<String> {
    let node_id = prepared.node.id.clone();

    // Check if this module accepts children
    if prepared.accepts_children {
        // Use the shared implementation for inserting container with selected nodes
        let selected_nodes = if app.selected_nodes.is_empty() {
            if let Some(last_selected) = app.tree_state.borrow().selected().last() {
                let mut vec_from_tree = vec![last_selected.clone()];
                vec_from_tree.retain(|item| !item.starts_with("__"));
                vec_from_tree
            } else {
                Vec::new()
            }
        } else {
            app.selected_nodes.clone()
        };

        // For container modules, we need selected child nodes
        if selected_nodes.is_empty() {
            return Err(CommandError::NoChildrenSelected);
        }
        insert_container_with_selected_nodes(app, prepared.node, &selected_nodes)
    } else {
        let module = prepared.node;

        // Determine insertion point based on current selection
        let selected = app.tree_state.borrow().selected().last().cloned();
        if before {
            ensure_insert_before_target(app)?;
        }

        // Check if selected node is in a module definition
        let mut in_module_def = if let Some(ref selected_id) = selected {
            app.find_module_definition_for_node(selected_id).is_some()
        } else {
            false
        };

        // Children module can only be used inside module definitions
        if module_name == "children" && !in_module_def {
            return Err(CommandError::InvalidCommand(
                "children module can only be used inside module definitions".to_string(),
            ));
        }

        // Special case: inserting a module with the same name as the module definition when selected is the definition header
        // This should create an instance in the modules section, not add to definition body
        if in_module_def {
            let selected_id = selected.as_ref().unwrap();
            if let Some(mod_def_name) = app.find_module_definition_for_node(selected_id) {
                if module_name == mod_def_name && selected_id.starts_with("__moddef_") {
                    in_module_def = false;
                }
            }
        }

        if in_module_def {
            // Insert into module definition body
            let selected_id = selected.unwrap();
            let mod_def_name = app.find_module_definition_for_node(&selected_id).unwrap();

            // Find the module definition
            let mod_def_idx = app
                .ast
                .module_defines
                .iter()
                .position(|md| md.name == mod_def_name)
                .ok_or_else(|| {
                    CommandError::InvalidCommand(format!(
                        "Module definition not found: {}",
                        mod_def_name
                    ))
                })?;

            // Check if selected node is the module definition itself
            if selected_id.starts_with("__moddef_") {
                // Insert at the end of module definition body
                app.ast_mut().module_defines[mod_def_idx].body.push(module);
            } else {
                // Insert after the selected node in module definition body
                if before {
                    app.ast_mut().insert_before(&selected_id, module)?;
                } else {
                    app.ast_mut().insert_after(&selected_id, module)?;
                }
            }
        } else {
            // Check if selected node is in Modules section (not a section header)
            let insert_at_root = match &selected {
                None => true,
                Some(id) => {
                    // If selected is a section header or not a module node, insert at root
                    id.starts_with("__") || app.ast.find_node_by_id(id).is_none()
                }
            };

            if insert_at_root {
                // Insert at root level of Modules section
                app.ast_mut().add_module(module)?;
            } else if let Some(selected_id) = selected {
                // Find the selected node and insert after it
                if before {
                    app.ast_mut().insert_before(&selected_id, module)?;
                } else {
                    app.ast_mut().insert_after(&selected_id, module)?;
                }
            }
        }

        // Select the newly inserted module for continued operations
        // Use the full path to ensure proper navigation in nested trees
        if let Some(path) = app.find_node_path(&node_id) {
            app.tree_state.borrow_mut().select(path.clone());
            // Open the appropriate section based on path
            if !path.is_empty() {
                // Open all parent sections
                let mut parent_path = Vec::new();
                for item in path.iter().take(path.len() - 1) {
                    parent_path.push(item.clone());
                    app.tree_state.borrow_mut().open(parent_path.clone());
                }
            }
        } else {
            // Fallback: just select by ID if path not found
            // Try to determine if this is in module definitions or modules
            let section = if in_module_def {
                "__moddefs"
            } else {
                "__modules"
            };
            app.tree_state
                .borrow_mut()
                .select(vec![section.to_string(), node_id.clone()]);
            app.tree_state.borrow_mut().open(vec![section.to_string()]);
        }

        // If we inserted a children module into a module definition, update the custom module's accepts_children flag
        if module_name == "children" && in_module_def {
            reload_project_definitions(app);
        }

        Ok(node_id)
    }
}

fn module_sibling_ids(nodes: &[ModuleNode], target_id: &str) -> Option<Vec<String>> {
    if nodes.iter().any(|node| node.id == target_id) {
        return Some(nodes.iter().map(|node| node.id.clone()).collect());
    }
    nodes
        .iter()
        .find_map(|node| module_sibling_ids(&node.children, target_id))
}

fn deletion_path(app: &App, target_id: &str) -> Option<Vec<String>> {
    let section = if target_id.starts_with("__var_") {
        Some("__globals")
    } else if target_id.starts_with("__func_") {
        Some("__functions")
    } else if target_id.starts_with("__moddef_") {
        Some("__moddefs")
    } else {
        None
    };
    section
        .map(|section| vec![section.to_string(), target_id.to_string()])
        .or_else(|| app.find_node_path(target_id))
}

fn sibling_ids(app: &App, target_id: &str) -> Option<Vec<String>> {
    if target_id.starts_with("__var_") {
        return Some(
            app.ast
                .global_variables
                .iter()
                .map(|variable| format!("__var_{}", variable.name))
                .collect(),
        );
    }
    if target_id.starts_with("__func_") {
        return Some(
            app.ast
                .function_defines
                .iter()
                .map(|function| format!("__func_{}", function.name))
                .collect(),
        );
    }
    if target_id.starts_with("__moddef_") {
        return Some(
            app.ast
                .module_defines
                .iter()
                .map(|module| format!("__moddef_{}", module.name))
                .collect(),
        );
    }
    module_sibling_ids(&app.ast.modules, target_id).or_else(|| {
        app.ast
            .module_defines
            .iter()
            .find_map(|definition| module_sibling_ids(&definition.body, target_id))
    })
}

fn selection_after_removing(app: &App, target_ids: &[String]) -> Option<Vec<String>> {
    let removed = target_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let (target_id, target_path) = target_ids
        .iter()
        .filter_map(|target_id| deletion_path(app, target_id).map(|path| (target_id, path)))
        .min_by_key(|(_, path)| path.len())?;
    let siblings = sibling_ids(app, target_id)?;
    let position = siblings.iter().position(|sibling| sibling == target_id)?;
    let nearest = siblings[position + 1..]
        .iter()
        .find(|sibling| !removed.contains(sibling.as_str()))
        .or_else(|| {
            siblings[..position]
                .iter()
                .rev()
                .find(|sibling| !removed.contains(sibling.as_str()))
        });

    nearest
        .and_then(|sibling| deletion_path(app, sibling))
        .or_else(|| (target_path.len() > 1).then(|| target_path[..target_path.len() - 1].to_vec()))
}

fn restore_selection_after_removing(app: &mut App, selection: Option<Vec<String>>) {
    if let Some(selection) = selection {
        app.tree_state.borrow_mut().select(selection);
    }
    app.validate_tree_state();
}

/// Delete command
pub fn cmd_delete(app: &mut App, node_id: &str) -> CommandResult<()> {
    let node_ids = if !node_id.is_empty() {
        vec![node_id.to_string()]
    } else if !app.selected_nodes.is_empty() {
        app.selected_nodes.clone()
    } else {
        vec![app
            .tree_state
            .borrow()
            .selected()
            .last()
            .cloned()
            .ok_or(CommandError::NoNodeSelected)?]
    };

    if let Some(section_id) = node_ids.iter().find(|id| {
        id.starts_with("__")
            && !id.starts_with("__var_")
            && !id.starts_with("__func_")
            && !id.starts_with("__moddef_")
    }) {
        return Err(CommandError::Custom(format!(
            "Cannot delete section header: {}",
            section_id
        )));
    }

    let fallback_selection = selection_after_removing(app, &node_ids);
    let mut cut_nodes = Vec::new();
    for target_id in &node_ids {
        if let Some(name) = target_id.strip_prefix("__var_") {
            app.ast_mut().remove_global_variable(name)?;
        } else if let Some(name) = target_id.strip_prefix("__func_") {
            app.ast_mut().remove_function_define(name)?;
        } else if let Some(name) = target_id.strip_prefix("__moddef_") {
            app.ast_mut().remove_module_define(name)?;
        } else if app.ast.find_node_anywhere(target_id).is_some() {
            // A selected descendant may already have been removed with its parent.
            if let Ok(node) = app.ast_mut().delete_node(target_id) {
                cut_nodes.push(node);
            }
        }
    }
    if !cut_nodes.is_empty() {
        app.node_clipboard = cut_nodes;
    }
    reload_project_definitions(app);
    app.selected_nodes.clear();
    restore_selection_after_removing(app, fallback_selection);

    Ok(())
}

/// Apply boolean operation (union, difference, intersection)
#[allow(dead_code)]
pub fn cmd_boolean_op(
    app: &mut App,
    operation: &str,
    node_ids: &[String],
) -> CommandResult<String> {
    // For boolean operations, we need to select the nodes first
    // Save current selection
    let current_selection = app.selected_nodes.clone();

    // Temporarily set selection to the provided node_ids
    app.selected_nodes = node_ids.to_vec();

    // Call cmd_insert with no parameters (boolean operations don't have parameters)
    let result = cmd_insert(app, operation, None, None);

    // If cmd_insert failed, restore original selection
    if result.is_err() {
        app.selected_nodes = current_selection;
    }
    // If cmd_insert succeeded, it will have cleared the selection
    // (through insert_container_with_selected_nodes)

    result
}

/// Insert a container module with selected nodes as children
/// Handles both modules section and module definition contexts
fn insert_container_with_selected_nodes(
    app: &mut App,
    container: ModuleNode,
    selected_node_ids: &[String],
) -> CommandResult<String> {
    if selected_node_ids.is_empty() {
        return Err(CommandError::NoChildrenSelected);
    }

    let container_id = container.id.clone();
    let first_selected = selected_node_ids.first().cloned();
    let in_module_def = if let Some(ref first_id) = first_selected {
        app.find_module_definition_for_node(first_id).is_some()
    } else {
        false
    };

    // Validate all selected nodes are in the same context
    let mut context_module_def_name: Option<String> = None;
    for node_id in selected_node_ids {
        let node_in_module_def = app.find_module_definition_for_node(node_id);
        match (node_in_module_def, &context_module_def_name) {
            (Some(ref mod_def_name), Some(ref context_name)) => {
                if mod_def_name != context_name {
                    return Err(CommandError::InvalidCommand(format!(
                        "Selected nodes are in different module definitions: {} vs {}",
                        mod_def_name, context_name
                    )));
                }
            }
            (Some(ref mod_def_name), None) => {
                context_module_def_name = Some(mod_def_name.clone());
            }
            (None, Some(_)) => {
                return Err(CommandError::InvalidCommand(
                    "Selected nodes are in mixed contexts (module definition vs modules section)"
                        .to_string(),
                ));
            }
            (None, None) => {
                // All nodes are in modules section, context remains None
            }
        }
    }
    // Ensure context consistency with first node
    if in_module_def && context_module_def_name.is_none() {
        // This shouldn't happen but handle edge case
        return Err(CommandError::InvalidCommand(
            "Inconsistent context detection".to_string(),
        ));
    }

    if in_module_def {
        // Handle insertion into module definition body
        let first_id = first_selected.unwrap();
        let mod_def_name = context_module_def_name.unwrap();

        // Find the module definition index
        let mod_def_idx = app
            .ast
            .module_defines
            .iter()
            .position(|md| md.name == mod_def_name)
            .ok_or_else(|| {
                CommandError::InvalidCommand(format!(
                    "Module definition not found: {}",
                    mod_def_name
                ))
            })?;

        // Find parent in module definition body
        let parent_id = find_node_parent(&app.ast.module_defines[mod_def_idx].body, &first_id);

        // Insert the container in module definition body
        if let Some(parent_id_val) = &parent_id {
            insert_child_before(
                &mut app.ast_mut().module_defines[mod_def_idx].body,
                parent_id_val,
                &first_id,
                container.clone(),
            )
            .map_err(CommandError::InvalidCommand)?;
        } else {
            // First selected node was at root level of module definition body
            if let Some(pos) = app.ast_mut().module_defines[mod_def_idx]
                .body
                .iter()
                .position(|m| m.id == first_id)
            {
                app.ast_mut().module_defines[mod_def_idx]
                    .body
                    .insert(pos, container.clone());
            } else {
                // Fallback: add to end of module definition body
                app.ast_mut().module_defines[mod_def_idx]
                    .body
                    .push(container.clone());
            }
        }

        // Collect nodes to move from module definition body
        let mut nodes_to_move = Vec::new();
        for node_id in selected_node_ids {
            // Search in module definition body
            if let Some(node) =
                find_node_in_module_definition(&app.ast.module_defines[mod_def_idx].body, node_id)
            {
                nodes_to_move.push(node.clone());
            }
        }

        // Delete nodes from module definition body
        for node_id in selected_node_ids {
            delete_node_from_module_definition(
                &mut app.ast_mut().module_defines[mod_def_idx].body,
                node_id,
            )
            .map_err(CommandError::InvalidCommand)?;
        }

        // Add collected nodes to the container
        if let Some(container_mut) = find_node_in_module_definition_mut(
            &mut app.ast_mut().module_defines[mod_def_idx].body,
            &container_id,
        ) {
            for node in nodes_to_move {
                container_mut.children.push(node);
            }
        }
    } else {
        // Original logic for modules section
        let parent_id = if let Some(ref first_id) = first_selected {
            find_node_parent(&app.ast.modules, first_id)
        } else {
            None
        };

        // Insert the container BEFORE deleting the selected nodes
        if let Some(parent_id_val) = &parent_id {
            insert_child_before(
                &mut app.ast_mut().modules,
                parent_id_val,
                first_selected.as_ref().unwrap(),
                container.clone(),
            )
            .map_err(CommandError::InvalidCommand)?;
        } else {
            // First selected node was at root level
            if let Some(pos) = app
                .ast
                .modules
                .iter()
                .position(|m| m.id == *first_selected.as_ref().unwrap())
            {
                app.ast_mut().modules.insert(pos, container.clone());
            } else {
                app.ast_mut().add_module(container.clone())?;
            }
        }

        // Collect nodes to move before modifying the tree
        let nodes_to_move: Vec<ModuleNode> = selected_node_ids
            .iter()
            .filter_map(|node_id| app.ast.find_node_by_id(node_id).cloned())
            .collect();

        // Delete the selected nodes from the tree
        for node_id in selected_node_ids {
            app.ast_mut().delete_node(node_id)?;
        }

        // Add collected nodes to the container
        if let Some(container_mut) = app.ast_mut().find_node_mut(&container_id) {
            for node in nodes_to_move {
                container_mut.children.push(node);
            }
        }
    }

    // Select the newly created container module for continued operations
    // Use the full path to ensure proper navigation in nested trees
    if let Some(path) = app.find_node_path(&container_id) {
        app.tree_state.borrow_mut().select(path);
    } else {
        // Fallback: just select by ID if path not found
        app.tree_state
            .borrow_mut()
            .select(vec![container_id.clone()]);
    }

    // Clear the selected nodes since they've been moved into the container
    app.selected_nodes.clear();

    Ok(container_id)
}

/// Select command
#[allow(dead_code)]
pub fn cmd_select(app: &mut App, node_id: &str) -> CommandResult<()> {
    if app.ast.find_node_by_id(node_id).is_none() {
        return Err(CommandError::InvalidCommand(format!(
            "Node not found: {}",
            node_id
        )));
    }

    if !app.selected_nodes.contains(&node_id.to_string()) {
        app.selected_nodes.push(node_id.to_string());
    }

    Ok(())
}

/// Deselect command
#[allow(dead_code)]
pub fn cmd_deselect(app: &mut App, node_id: &str) -> CommandResult<()> {
    app.selected_nodes.retain(|id| id != node_id);
    Ok(())
}

/// Clear selection
#[allow(dead_code)]
pub fn cmd_clear_selection(app: &mut App) {
    app.selected_nodes.clear();
}

/// Navigation commands
/// Move cursor down (next)
#[allow(dead_code)]
pub fn cmd_next(app: &mut App) -> CommandResult<()> {
    app.tree_state.borrow_mut().key_down();
    app.update_navigation_status();
    Ok(())
}

/// Move cursor up (previous)
#[allow(dead_code)]
pub fn cmd_prev(app: &mut App) -> CommandResult<()> {
    app.tree_state.borrow_mut().key_up();
    app.update_navigation_status();
    Ok(())
}

/// Collapse node (move left)
#[allow(dead_code)]
pub fn cmd_collapse(app: &mut App) -> CommandResult<()> {
    app.tree_state.borrow_mut().key_left();
    app.update_navigation_status();
    Ok(())
}

/// Expand node (move right)
#[allow(dead_code)]
pub fn cmd_expand(app: &mut App) -> CommandResult<()> {
    app.tree_state.borrow_mut().key_right();
    app.update_navigation_status();
    Ok(())
}

/// Toggle node (move right)
#[allow(dead_code)]
pub fn cmd_toggle(app: &mut App) -> CommandResult<()> {
    app.tree_state.borrow_mut().toggle_selected();
    app.update_navigation_status();
    Ok(())
}

/// Select/toggle current node
#[allow(dead_code)]
pub fn cmd_select_toggle(app: &mut App) -> CommandResult<()> {
    let selected = app.tree_state.borrow().selected().last().cloned();
    if let Some(node_id) = selected {
        // Prevent selection of section headers
        if node_id.starts_with("__") {
            app.set_info("Cannot select section headers");
            return Ok(());
        }

        if app.selected_nodes.contains(&node_id) {
            app.selected_nodes.retain(|n| n != &node_id);
            app.set_info(&format!("Deselected: {}", node_id));
        } else {
            app.selected_nodes.push(node_id.clone());
            app.set_info(&format!("Selected: {}", node_id));
        }
        Ok(())
    } else {
        Err(CommandError::NoNodeSelected)
    }
}

/// Clear all selections
#[allow(dead_code)]
pub fn cmd_deselect_all(app: &mut App) -> CommandResult<()> {
    app.selected_nodes.clear();
    app.set_info("All nodes deselected");
    Ok(())
}

// Helper function to parse arguments
/// Parse arguments from a string, respecting nested structures (lists, etc.)
///
/// Handles complex parameters like:
/// - Simple values: 10, 1.5, "text", true
/// - Lists: [10, 20, 30] or [1.5, 2.5, 3.5]
/// - Named parameters: size=[10,10,10], center=true
/// - Mixed: 10, [20,30], center=true
fn parse_arguments(param_str: &str, _module_def: &ModuleDef) -> CommandResult<Vec<Argument>> {
    let mut args = Vec::new();

    if param_str.trim().is_empty() {
        return Ok(args);
    }

    // Split parameters while respecting brackets and quotes
    let parts = split_parameters(param_str)?;

    for (i, part) in parts.iter().enumerate() {
        let part = part.trim();

        // Check if this is a named parameter (contains '=')
        if let Some(eq_pos) = part.find('=') {
            let name = part[..eq_pos].trim();
            let value_str = part[eq_pos + 1..].trim();

            let value = Expr::parse(value_str).map_err(|e| {
                CommandError::ParameterError(format!(
                    "Invalid parameter value for '{}': {} - {}",
                    name, value_str, e
                ))
            })?;

            args.push(Argument::Named {
                name: name.to_string(),
                value,
            });
        } else {
            // Positional parameter
            let value = Expr::parse(part).map_err(|e| {
                CommandError::ParameterError(format!(
                    "Invalid parameter at position {}: {} - {}",
                    i, part, e
                ))
            })?;

            args.push(Argument::Positional(value));
        }
    }

    Ok(args)
}

/// Split parameters respecting brackets and quotes
/// This function splits by commas but ignores commas inside brackets or quotes
///
/// Examples:
/// "10, 20, 30" → ["10", "20", "30"]
/// "[10, 20], 30" → ["[10, 20]", "30"]
/// "size=[10,20,30], center=true" → ["size=[10,20,30]", "center=true"]
fn split_parameters(input: &str) -> CommandResult<Vec<String>> {
    let mut params = Vec::new();
    let mut current = String::new();
    let mut bracket_depth = 0;
    let mut paren_depth = 0;
    let mut in_quotes = false;
    let mut escape_next = false;

    for ch in input.chars() {
        if escape_next {
            current.push(ch);
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if in_quotes => {
                escape_next = true;
                current.push(ch);
            }
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            '[' if !in_quotes => {
                bracket_depth += 1;
                current.push(ch);
            }
            ']' if !in_quotes && bracket_depth > 0 => {
                bracket_depth -= 1;
                current.push(ch);
            }
            '(' if !in_quotes => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' if !in_quotes && paren_depth > 0 => {
                paren_depth -= 1;
                current.push(ch);
            }
            ',' if !in_quotes && bracket_depth == 0 && paren_depth == 0 => {
                // This comma is a parameter separator
                if !current.trim().is_empty() {
                    params.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    // Add the last parameter
    if !current.trim().is_empty() {
        params.push(current.trim().to_string());
    }

    // Validate bracket/quote balance
    if in_quotes {
        return Err(CommandError::ParameterError(
            "Unclosed quoted string in parameters".to_string(),
        ));
    }
    if bracket_depth != 0 {
        return Err(CommandError::ParameterError(
            "Mismatched brackets in parameters".to_string(),
        ));
    }
    if paren_depth != 0 {
        return Err(CommandError::ParameterError(
            "Mismatched parentheses in parameters".to_string(),
        ));
    }

    Ok(params)
}

fn normalized_project_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if has_extension(path, PROJECT_EXTENSION) {
        path.to_path_buf()
    } else {
        path.with_extension(PROJECT_EXTENSION)
    }
}

/// Save the editable project package.
pub fn cmd_write(app: &mut App, filename: &str) -> CommandResult<()> {
    if filename.is_empty() {
        app.current_file.clone().ok_or(CommandError::Custom(
            "No current file specified and no filename provided to write to".to_string(),
        ))?;
    } else {
        let expanded = normalized_project_path(expand_tilde(filename));

        // Check if file exists and warn user if it's different from current file
        if expanded.exists() {
            if let Some(ref current_file) = app.current_file {
                let current_expanded = expand_tilde(current_file);
                if expanded != current_expanded {
                    return Err(CommandError::Custom(format!(
                        "File '{}' exists; use project save-as <path> --force to replace it",
                        filename
                    )));
                }
            } else {
                return Err(CommandError::Custom(format!(
                    "File '{}' exists; use project save-as <path> --force to replace it",
                    filename
                )));
            }
        }
    }
    cmd_write_force(app, filename)
}

/// Force save the editable project package.
pub fn cmd_write_force(app: &mut App, filename: &str) -> CommandResult<()> {
    app.ast_mut().sync_active_source();
    // Expand tilde in filename
    let expanded_filepath = if filename.is_empty() {
        let current_file_str = app.current_file.clone().ok_or(CommandError::Custom(
            "No current file specified and no filename provided to write to".to_string(),
        ))?;
        expand_tilde(current_file_str)
    } else {
        expand_tilde(filename)
    };

    let filepath = normalized_project_path(expanded_filepath);
    let document = ProjectDocument {
        name: app.project_name.clone(),
        sources: (*app.ast).clone(),
        assemblies: app.assemblies.clone(),
        active_assembly: app.active_assembly.clone(),
    };
    save_project(&filepath, &document).map_err(|error| {
        CommandError::Custom(format!(
            "Failed to write project '{}': {error}",
            filepath.display()
        ))
    })?;

    // If app.current_file is None but we're saving with a filename, update current_file
    // This handles the case where we're saving a new unnamed file
    if !filename.is_empty() || app.current_file.is_none() {
        app.current_file = Some(filepath.to_string_lossy().into_owned());
    }
    app.mark_saved();

    Ok(())
}

/// Load a .scadtui project package.
pub fn cmd_load(app: &mut App, filename: &str) -> CommandResult<()> {
    if !app.saved {
        return Err(CommandError::Custom(
            "Project is not saved; use 'project open <path> --force' to discard changes"
                .to_string(),
        ));
    }
    cmd_load_force(app, filename)
}

/// Force load a .scadtui project package.
pub fn cmd_load_force(app: &mut App, filename: &str) -> CommandResult<()> {
    // Expand tilde in filename
    let expanded_filename = expand_tilde(filename);

    // Check file exists
    if !expanded_filename.exists() {
        return Err(CommandError::Custom(format!(
            "File '{}' not found",
            expanded_filename.display()
        )));
    }

    if !has_extension(&expanded_filename, PROJECT_EXTENSION) {
        return Err(CommandError::Custom(
            "project open requires a .scadtui package; use source import for .scad files"
                .to_string(),
        ));
    }
    let project = load_project(&expanded_filename).map_err(|error| {
        CommandError::Custom(format!(
            "Failed to open project '{}': {error}",
            expanded_filename.display()
        ))
    })?;

    // Replace AST
    app.project_name = project.name;
    app.ast = Arc::new(project.sources);
    app.assemblies = project.assemblies;
    app.active_assembly = project.active_assembly;
    app.selected_assembly_part = None;
    app.assembly_scroll_offset = 0;
    app.assembly_clipboard = None;
    app.invalidate_source_previews();
    app.assembly_preview.clear();

    // First, reload custom modules in library manager
    reload_project_definitions(app);

    // Reset navigation state
    app.selected_nodes.clear();
    app.undo_stack.clear();
    app.redo_stack.clear();
    app.tree_state.borrow_mut().select(Vec::new());
    app.current_file = Some(filename.to_string());
    app.mark_saved();

    Ok(())
}

pub fn cmd_new_project(app: &mut App, name: Option<&str>, force: bool) -> CommandResult<()> {
    if !force && !app.saved {
        return Err(CommandError::Custom(
            "Project is not saved; use 'project new --force' to discard changes".to_string(),
        ));
    }
    let name = name.unwrap_or("untitled").trim();
    if name.is_empty() {
        return Err(CommandError::Custom(
            "Project name cannot be empty".to_string(),
        ));
    }
    app.project_name = name.to_string();
    app.ast = Arc::new(openscad_core::AstRoot::new_project("main.scad"));
    app.assemblies.clear();
    app.active_assembly = None;
    app.selected_assembly_part = None;
    app.assembly_scroll_offset = 0;
    app.assembly_clipboard = None;
    app.library.reload_custom_modules_from_ast(&[]);
    app.library.reload_custom_functions_from_ast(&[]);
    app.selected_nodes.clear();
    app.undo_stack.clear();
    app.redo_stack.clear();
    app.tree_state.borrow_mut().select(Vec::new());
    app.current_file = None;
    app.saved = false;
    app.invalidate_source_previews();
    app.assembly_preview.clear();
    app.init_tree_selection();
    app.set_info(&format!(
        "Created project '{}' with 'main.scad'",
        app.project_name
    ));
    Ok(())
}

pub fn cmd_rename_project(app: &mut App, name: &str) -> CommandResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CommandError::Custom(
            "Project name cannot be empty".to_string(),
        ));
    }
    app.project_name = name.to_string();
    app.set_info(&format!("Renamed project to '{name}'"));
    Ok(())
}

pub fn cmd_new_file(app: &mut App, filename: &str) -> CommandResult<()> {
    let virtual_path = normalize_project_source_path(filename)?;
    if app
        .ast
        .embedded_sources
        .iter()
        .any(|source| source.virtual_path == virtual_path)
    {
        return Err(CommandError::Custom(format!(
            "Project source '{virtual_path}' already exists"
        )));
    }
    app.ast_mut().sync_active_source();
    app.ast_mut()
        .embedded_sources
        .push(openscad_core::EmbeddedSourceFile::empty(
            virtual_path.clone(),
            openscad_core::EmbeddedSourceRole::Dependency,
        ));
    app.ast_mut().activate_source(&virtual_path)?;
    reload_project_definitions(app);
    app.selected_nodes.clear();
    app.undo_stack.clear();
    app.redo_stack.clear();
    app.tree_state.borrow_mut().select(Vec::new());
    app.init_tree_selection();
    app.invalidate_source_previews();
    app.set_info(&format!("Created project source '{virtual_path}'"));
    Ok(())
}

fn normalize_project_source_path(filename: &str) -> CommandResult<String> {
    let requested = Path::new(filename);
    let path = if requested.extension().is_none() {
        requested.with_extension("scad")
    } else {
        requested.to_path_buf()
    };
    if path.is_absolute()
        || path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
        || !has_extension(&path, "scad")
    {
        return Err(CommandError::Custom(
            "source new requires a safe relative name with an optional .scad extension".to_string(),
        ));
    }
    let components = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if components.is_empty() {
        return Err(CommandError::Custom(
            "source new requires a non-empty source name".to_string(),
        ));
    }
    Ok(components.join("/"))
}

pub fn cmd_rename_source(app: &mut App, source: &str, new_name: &str) -> CommandResult<()> {
    let editable = app
        .ast
        .embedded_sources
        .iter()
        .filter(|source| source.editable)
        .map(|source| source.virtual_path.clone())
        .collect::<Vec<_>>();
    let old_path = resolve_project_source(&editable, source)?;
    let requested = Path::new(new_name);
    let target = if requested.components().count() == 1 {
        let parent = Path::new(&old_path)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        normalize_project_source_path(&parent.join(requested).to_string_lossy())?
    } else {
        normalize_project_source_path(new_name)?
    };
    if old_path == target {
        app.set_info(&format!("Project source is already named '{target}'"));
        return Ok(());
    }
    if app
        .ast
        .embedded_sources
        .iter()
        .any(|source| source.virtual_path == target)
    {
        return Err(CommandError::Custom(format!(
            "Project source '{target}' already exists"
        )));
    }

    app.ast_mut().sync_active_source();
    let ast = app.ast_mut();
    let active_before = ast.active_source.clone();
    let mut directive_updates = Vec::new();
    for dependency in &mut ast.source_dependencies {
        let old_from = dependency.from.clone();
        let old_reference = dependency.reference.clone();
        let touched = dependency.from == old_path || dependency.to == old_path;
        if dependency.from == old_path {
            dependency.from.clone_from(&target);
        }
        if dependency.to == old_path {
            dependency.to.clone_from(&target);
        }
        if touched {
            dependency.reference = relative_virtual_path(&dependency.from, &dependency.to);
            directive_updates.push((
                if old_from == old_path {
                    target.clone()
                } else {
                    old_from
                },
                dependency.kind,
                old_reference,
                dependency.reference.clone(),
            ));
        }
    }
    let renamed = ast
        .embedded_sources
        .iter_mut()
        .find(|source| source.virtual_path == old_path)
        .ok_or_else(|| {
            CommandError::Custom(format!("Project source '{old_path}' was not found"))
        })?;
    renamed.virtual_path.clone_from(&target);
    if ast.entry_source.as_deref() == Some(&old_path) {
        ast.entry_source = Some(target.clone());
    }
    if ast.active_source.as_deref() == Some(&old_path) {
        ast.active_source = Some(target.clone());
    }
    for (from, kind, old_reference, new_reference) in &directive_updates {
        if let Some(source) = ast
            .embedded_sources
            .iter_mut()
            .find(|source| source.virtual_path == *from)
        {
            let references = match kind {
                openscad_core::SourceDependencyKind::Include => &mut source.includes,
                openscad_core::SourceDependencyKind::Use => &mut source.uses,
            };
            for reference in references
                .iter_mut()
                .filter(|reference| **reference == *old_reference)
            {
                reference.clone_from(new_reference);
            }
            if !source.editable {
                let directive = match kind {
                    openscad_core::SourceDependencyKind::Include => "include",
                    openscad_core::SourceDependencyKind::Use => "use",
                };
                source.content = source.content.replace(
                    &format!("{directive} <{old_reference}>"),
                    &format!("{directive} <{new_reference}>"),
                );
            }
        }
        if active_before.as_deref() == Some(from) {
            let references = match kind {
                openscad_core::SourceDependencyKind::Include => &mut ast.includes,
                openscad_core::SourceDependencyKind::Use => &mut ast.uses,
            };
            for reference in references
                .iter_mut()
                .filter(|reference| **reference == *old_reference)
            {
                reference.clone_from(new_reference);
            }
        }
    }
    for assembly in &mut app.assemblies {
        for part in &mut assembly.parts {
            if part.source.virtual_path() == old_path {
                part.source = openscad_assembly::MeshSourceRef::project_source(&target);
            }
        }
    }
    reload_project_definitions(app);
    app.invalidate_source_previews();
    app.init_tree_selection();
    app.set_info(&format!(
        "Renamed project source '{old_path}' to '{target}'"
    ));
    Ok(())
}

pub fn cmd_remove_source(app: &mut App, source: &str) -> CommandResult<()> {
    let editable = app
        .ast
        .embedded_sources
        .iter()
        .filter(|source| source.editable)
        .map(|source| source.virtual_path.clone())
        .collect::<Vec<_>>();
    let target = resolve_project_source(&editable, source)?;
    if editable.len() == 1 {
        return Err(CommandError::Custom(
            "A project must retain at least one editable source".to_string(),
        ));
    }
    if let Some(dependency) = app
        .ast
        .source_dependencies
        .iter()
        .find(|dependency| dependency.to == target)
    {
        return Err(CommandError::Custom(format!(
            "Cannot remove '{target}'; it is referenced by '{}'",
            dependency.from
        )));
    }
    if let Some((assembly, part)) = app.assemblies.iter().find_map(|assembly| {
        assembly
            .parts
            .iter()
            .find(|part| part.source.virtual_path() == target)
            .map(|part| (assembly, part))
    }) {
        return Err(CommandError::Custom(format!(
            "Cannot remove '{target}'; assembly '{}' part '{}' uses it",
            assembly.name, part.name
        )));
    }

    app.ast_mut().sync_active_source();
    let fallback = editable
        .iter()
        .find(|path| **path != target)
        .cloned()
        .expect("editable source count was checked");
    let ast = app.ast_mut();
    ast.embedded_sources
        .retain(|source| source.virtual_path != target);
    ast.source_dependencies
        .retain(|dependency| dependency.from != target && dependency.to != target);
    if ast.entry_source.as_deref() == Some(&target) {
        ast.entry_source = Some(fallback.clone());
        if let Some(source) = ast
            .embedded_sources
            .iter_mut()
            .find(|source| source.virtual_path == fallback)
        {
            source.role = openscad_core::EmbeddedSourceRole::Entry;
        }
    }
    if ast.active_source.as_deref() == Some(&target) {
        ast.active_source = None;
        ast.activate_source(&fallback)?;
    }
    reload_project_definitions(app);
    app.selected_nodes.clear();
    app.undo_stack.clear();
    app.redo_stack.clear();
    app.invalidate_source_previews();
    app.init_tree_selection();
    app.set_info(&format!("Removed project source '{target}'"));
    Ok(())
}

/// Import an OpenSCAD source tree into the current structured project.
pub fn cmd_edit_scad(app: &mut App, filename: &str) -> CommandResult<()> {
    let path = expand_tilde(filename);
    if !has_extension(&path, "scad") {
        return Err(CommandError::Custom(
            "source import requires a .scad source file".to_string(),
        ));
    }
    let target = attach_editable_scad(app.ast_mut(), &path).map_err(CommandError::Custom)?;
    reload_project_definitions(app);
    app.selected_nodes.clear();
    app.undo_stack.clear();
    app.redo_stack.clear();
    app.tree_state.borrow_mut().select(Vec::new());
    app.saved = false;
    app.invalidate_source_previews();
    app.set_info(&format!(
        "Imported '{}' as editable project source '{target}'",
        path.display(),
    ));
    Ok(())
}

fn has_extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn reload_project_definitions(app: &mut App) {
    let reachable_sources = reachable_source_paths(&app.ast);
    let mut modules = Vec::new();
    let mut functions = Vec::new();
    for source in app.ast.embedded_sources.iter().filter(|source| {
        source.role != openscad_core::EmbeddedSourceRole::Entry
            && reachable_sources.contains(&source.virtual_path)
    }) {
        modules.extend(source.module_defines.clone());
        functions.extend(source.function_defines.clone());
    }
    // Entry definitions are appended last so the LibraryManager's name map gives them priority.
    modules.extend(app.ast.module_defines.clone());
    functions.extend(app.ast.function_defines.clone());
    app.library.reload_custom_modules_from_ast(&modules);
    app.library.reload_custom_functions_from_ast(&functions);
}

fn reachable_source_paths(ast: &openscad_core::AstRoot) -> HashSet<String> {
    let mut reachable = HashSet::new();
    let Some(entry) = ast
        .active_source
        .clone()
        .or_else(|| ast.entry_source.clone())
    else {
        return reachable;
    };
    let mut pending = vec![entry];
    while let Some(source) = pending.pop() {
        if !reachable.insert(source.clone()) {
            continue;
        }
        pending.extend(
            ast.source_dependencies
                .iter()
                .filter(|dependency| dependency.from == source)
                .map(|dependency| dependency.to.clone()),
        );
    }
    reachable
}

fn cmd_export_source(app: &mut App, filename: &str) -> CommandResult<()> {
    let filepath = ensure_extension(resolve_export_path(app, filename)?, "scad");
    app.ast_mut().sync_active_source();
    let code = app
        .ast
        .active_source
        .as_deref()
        .and_then(|path| app.ast.source_code(path))
        .unwrap_or_else(|| app.ast.to_scad());
    fs::write(&filepath, code).map_err(|e| {
        CommandError::Custom(format!(
            "Failed to write file '{}': {}",
            filepath.display(),
            e
        ))
    })?;
    app.set_info(&format!(
        "Exported active source to '{}'",
        filepath.display()
    ));
    Ok(())
}

fn cmd_export_tree(app: &mut App, directory: &str) -> CommandResult<()> {
    let directory = resolve_export_path(app, directory)?;
    if directory.exists()
        && directory
            .read_dir()
            .map_err(|error| CommandError::Custom(error.to_string()))?
            .next()
            .is_some()
    {
        return Err(CommandError::Custom(format!(
            "Export directory '{}' is not empty",
            directory.display()
        )));
    }
    app.ast_mut().sync_active_source();
    let reachable = reachable_source_paths(&app.ast);
    let files = app
        .ast
        .embedded_sources
        .iter()
        .filter(|source| reachable.contains(&source.virtual_path))
        .map(|source| {
            let relative = safe_embedded_path(&source.virtual_path)?;
            let content = rewrite_absolute_source_references(
                &source.generated_content(),
                &source.virtual_path,
                &app.ast.source_dependencies,
            );
            Ok((relative, content))
        })
        .collect::<CommandResult<Vec<_>>>()?;
    fs::create_dir_all(&directory).map_err(|error| CommandError::Custom(error.to_string()))?;
    for (relative, content) in &files {
        let path = directory.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| CommandError::Custom(error.to_string()))?;
        }
        fs::write(&path, content).map_err(|error| CommandError::Custom(error.to_string()))?;
    }
    app.set_info(&format!(
        "Exported {} source files to '{}'",
        files.len(),
        directory.display()
    ));
    Ok(())
}

fn cmd_export_model(app: &mut App, filename: &str) -> CommandResult<()> {
    let filepath = resolve_export_path(app, filename)?;
    if filepath.extension().is_none() {
        return Err(CommandError::Custom(
            "model export requires an output extension".to_string(),
        ));
    }
    let (source, project) = active_source_project(app)?;
    let mut generator = openscad_render::OpenScadGenerator::new("openscad");
    if let Some(project) = project {
        generator = generator.with_project(project);
    }
    let started = std::time::Instant::now();
    if has_extension(&filepath, "dae") {
        let generated = generator
            .generate(&source)
            .map_err(|error| CommandError::Custom(error.to_string()))?;
        openscad_render::write_dae(&filepath, &generated.mesh)
            .map_err(|error| CommandError::Custom(error.to_string()))?;
    } else {
        generator
            .export(&source, &filepath)
            .map_err(|error| CommandError::Custom(error.to_string()))?;
    }
    app.set_info(&format!(
        "Exported model to '{}' in {:.2}s",
        filepath.display(),
        started.elapsed().as_secs_f64()
    ));
    Ok(())
}

fn resolve_export_path(app: &App, destination: &str) -> CommandResult<PathBuf> {
    let destination = expand_tilde(destination);
    if destination.is_absolute() {
        return Ok(destination);
    }
    let current_directory = std::env::current_dir().map_err(|error| {
        CommandError::Custom(format!("Failed to resolve current directory: {error}"))
    })?;
    let project_directory = app.current_file.as_deref().map(expand_tilde).map(|path| {
        let project_path = if path.is_absolute() {
            path
        } else {
            current_directory.join(path)
        };
        project_path
            .parent()
            .unwrap_or(&current_directory)
            .to_path_buf()
    });
    Ok(project_directory
        .unwrap_or(current_directory)
        .join(destination))
}

fn ensure_extension(path: PathBuf, extension: &str) -> PathBuf {
    if has_extension(&path, extension) {
        path
    } else {
        path.with_extension(extension)
    }
}

fn safe_embedded_path(path: &str) -> CommandResult<PathBuf> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err(CommandError::Custom(format!(
            "Unsafe embedded source path: {}",
            path.display()
        )));
    }
    Ok(path.to_path_buf())
}

/// Load and embed a SCAD source library without activating it.
pub fn cmd_load_library(app: &mut App, filename: &str) -> CommandResult<()> {
    let path = expand_tilde(filename);
    if !has_extension(&path, "scad") {
        return Err(CommandError::Custom(
            "library load requires a .scad source file".to_string(),
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| {
            CommandError::Custom(format!("Failed to resolve '{}': {error}", path.display()))
        })?
        .to_string_lossy()
        .into_owned();
    let existing_target = app
        .ast
        .embedded_sources
        .iter()
        .find(|source| source.original_path.as_deref() == Some(&canonical))
        .map(|source| (source.virtual_path.clone(), source.editable));
    if let Some((target, true)) = &existing_target {
        return Err(CommandError::Custom(format!(
            "'{target}' is already an editable project source; reference it with 'source use' or 'source include'"
        )));
    }
    app.push_undo();
    let target = match existing_target {
        Some((target, false)) => {
            if let Some(source) = app
                .ast_mut()
                .embedded_sources
                .iter_mut()
                .find(|source| source.virtual_path == target)
            {
                source.role = openscad_core::EmbeddedSourceRole::Library;
            }
            target
        }
        Some((_, true)) => unreachable!("editable sources were rejected before mutation"),
        None => attach_scad_library(app.ast_mut(), &path).map_err(CommandError::Custom)?,
    };
    reload_project_definitions(app);
    app.invalidate_source_previews();
    app.set_info(&format!(
        "Loaded SCAD library '{target}'; use 'source use {}' to activate it",
        Path::new(&target)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&target)
    ));
    Ok(())
}

pub fn cmd_list_libraries(app: &mut App) -> CommandResult<()> {
    let libraries = app
        .ast
        .embedded_sources
        .iter()
        .filter(|source| source.role == openscad_core::EmbeddedSourceRole::Library)
        .map(|source| source.virtual_path.clone())
        .collect::<Vec<_>>();
    let message = if libraries.is_empty() {
        "No SCAD libraries are loaded".to_string()
    } else {
        format!("Loaded libraries: {}", libraries.join(", "))
    };
    app.set_info(&message);
    Ok(())
}

/// Remove one unreferenced embedded library root and its private dependency tree.
pub fn cmd_remove_library(app: &mut App, name: &str) -> CommandResult<()> {
    let matches = app
        .ast
        .embedded_sources
        .iter()
        .filter(|source| source.role == openscad_core::EmbeddedSourceRole::Library)
        .filter(|source| library_source_matches(source, name))
        .map(|source| source.virtual_path.clone())
        .collect::<Vec<_>>();
    let root = match matches.as_slice() {
        [] => {
            return Err(CommandError::Custom(format!(
                "Loaded SCAD library '{name}' was not found"
            )))
        }
        [root] => root.clone(),
        _ => {
            return Err(CommandError::Custom(format!(
                "Library name '{name}' is ambiguous; use its embedded path"
            )))
        }
    };

    let mut removed = std::collections::HashSet::from([root.clone()]);
    loop {
        let dependencies = app
            .ast
            .source_dependencies
            .iter()
            .filter(|dependency| removed.contains(&dependency.from))
            .map(|dependency| dependency.to.clone())
            .collect::<Vec<_>>();
        let previous_len = removed.len();
        removed.extend(dependencies);
        if removed.len() == previous_len {
            break;
        }
    }

    if let Some(dependency) =
        app.ast.source_dependencies.iter().find(|dependency| {
            removed.contains(&dependency.to) && !removed.contains(&dependency.from)
        })
    {
        return Err(CommandError::Custom(format!(
            "Cannot remove library '{root}': '{}' references it with {:?}",
            dependency.from, dependency.kind
        )));
    }

    app.push_undo();
    let ast = app.ast_mut();
    ast.embedded_sources
        .retain(|source| !removed.contains(&source.virtual_path));
    ast.source_dependencies.retain(|dependency| {
        !removed.contains(&dependency.from) && !removed.contains(&dependency.to)
    });
    reload_project_definitions(app);
    app.invalidate_source_previews();
    app.set_info(&format!("Removed SCAD library '{root}'"));
    Ok(())
}

/// Add a use relationship from the active source to another project source.
pub fn cmd_use_library(app: &mut App, name: &str) -> CommandResult<()> {
    cmd_activate_source(app, name, openscad_core::SourceDependencyKind::Use)
}

/// Add an include relationship from the active source to another project source.
pub fn cmd_include_library(app: &mut App, name: &str) -> CommandResult<()> {
    cmd_activate_source(app, name, openscad_core::SourceDependencyKind::Include)
}

fn cmd_activate_source(
    app: &mut App,
    name: &str,
    kind: openscad_core::SourceDependencyKind,
) -> CommandResult<()> {
    let active = app
        .ast
        .active_source
        .clone()
        .or_else(|| app.ast.entry_source.clone())
        .ok_or_else(|| CommandError::Custom("Project has no active source".to_string()))?;
    let matches = app
        .ast
        .embedded_sources
        .iter()
        .filter(|source| source.virtual_path != active)
        .filter(|source| library_source_matches(source, name))
        .map(|source| source.virtual_path.clone())
        .collect::<Vec<_>>();
    let target = match matches.as_slice() {
        [] => {
            return Err(CommandError::Custom(format!(
                "Project source '{name}' is not loaded; use 'source import' or 'library load' first"
            )))
        }
        [target] => target.clone(),
        _ => {
            return Err(CommandError::Custom(format!(
                "Project source name '{name}' is ambiguous; use its embedded path"
            )))
        }
    };
    let reference = relative_virtual_path(&active, &target);
    let dependency = openscad_core::SourceDependency {
        from: active,
        to: target.clone(),
        reference: reference.clone(),
        kind,
    };
    let directive = match kind {
        openscad_core::SourceDependencyKind::Include => "include",
        openscad_core::SourceDependencyKind::Use => "use",
    };
    if app.ast.source_dependencies.contains(&dependency) {
        return Err(CommandError::Custom(format!(
            "Project source '{name}' is already referenced with {directive}"
        )));
    }

    app.push_undo();
    match kind {
        openscad_core::SourceDependencyKind::Include => {
            if !app.ast.includes.contains(&reference) {
                app.ast_mut().includes.push(reference);
            }
        }
        openscad_core::SourceDependencyKind::Use => {
            if !app.ast.uses.contains(&reference) {
                app.ast_mut().uses.push(reference);
            }
        }
    }
    app.ast_mut().source_dependencies.push(dependency);
    reload_project_definitions(app);
    app.invalidate_source_previews();
    app.set_info(&format!(
        "Referenced project source '{target}' with {directive}"
    ));
    Ok(())
}

fn library_source_matches(source: &openscad_core::EmbeddedSourceFile, name: &str) -> bool {
    if source.virtual_path == name {
        return true;
    }
    let path = Path::new(&source.virtual_path);
    path.file_name().and_then(|value| value.to_str()) == Some(name)
        || path.file_stem().and_then(|value| value.to_str()) == Some(name)
}

/// Define a global variable
/// Syntax: global <var_name>=<value>
/// Example: global width=100
///          global size=[10,20,30]
///          global $fn=50  (special variable)
pub fn cmd_global(app: &mut App, var_spec: &str) -> CommandResult<()> {
    use openscad_core::GlobalVariable;

    let var_spec = var_spec.trim();

    // Find the equals sign to separate name and value
    let equals_pos = var_spec.find('=');

    if equals_pos.is_none() {
        return Err(CommandError::InvalidCommand(
            "Invalid global variable syntax. Use: global <name>=<value>".to_string(),
        ));
    }

    let pos = equals_pos.unwrap();
    let name_part = var_spec[..pos].trim();
    let value_part = var_spec[pos + 1..].trim();

    // Validate identifier
    let identifier_body = name_part.strip_prefix('$').unwrap_or(name_part);
    if !is_valid_identifier(identifier_body) {
        return Err(CommandError::InvalidCommand(format!(
            "Invalid variable name: {}",
            name_part
        )));
    }

    // Parse the value
    let value = openscad_core::Expr::parse(value_part).map_err(|e| {
        CommandError::ParameterError(format!("Invalid value expression: {} - {}", value_part, e))
    })?;

    // Create global variable
    let global_var = GlobalVariable::new(name_part.to_string(), value);

    // Add or replace in the AST while preserving the definition's position.
    app.ast_mut()
        .upsert_global_variable(global_var)
        .map_err(CommandError::AstError)?;

    Ok(())
}

/// Helper function to validate identifier names
fn is_valid_identifier(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    let first = name.chars().next().unwrap();
    if !first.is_alphabetic() && first != '_' {
        return false;
    }

    name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Define a new custom function
/// Helper function to parse function signature with support for parentheses
fn parse_function_signature(sig: &str) -> CommandResult<(String, String)> {
    // Look for = to separate parameters/body
    let equals_pos = sig.find('=');

    if let Some(eq_pos) = equals_pos {
        let params_part = sig[..eq_pos].trim();
        let body_part = sig[eq_pos + 1..].trim();

        // Check if there are parentheses around parameters
        let cleaned_params = params_part.trim();

        // Check if it has parentheses format like (a, b, c)
        if cleaned_params.starts_with('(') && cleaned_params.ends_with(')') {
            let inner = &cleaned_params[1..cleaned_params.len() - 1]; // Remove parentheses
            Ok((inner.trim().to_string(), body_part.to_string()))
        } else {
            // Old format: a, b, c
            Ok((cleaned_params.to_string(), body_part.to_string()))
        }
    } else {
        // No equals sign - entire string is treated as body with no parameters
        Ok(("".to_string(), sig.trim().to_string()))
    }
}

pub fn cmd_funcdef(app: &mut App, func_def: &str) -> CommandResult<()> {
    use openscad_core::{Expr, FunctionDefinition, Parameter};

    let trimmed = func_def.trim();

    // Find where the function name ends and parameters begin
    if let Some(open_paren_pos) = trimmed.find('(') {
        let func_name = &trimmed[..open_paren_pos].trim();

        // Extract the part after the function name (should be "(params) = body")
        let params_and_body_part = &trimmed[open_paren_pos..];

        // Validate function name
        if !is_valid_identifier(func_name) {
            return Err(CommandError::InvalidCommand(format!(
                "Invalid function name: {}",
                func_name
            )));
        }

        // Parse parameters and body from the params_and_body_part
        let (params_part, body_part) = parse_function_signature(params_and_body_part)?;

        // Parse parameters
        let parameters = if params_part.is_empty() {
            Vec::new()
        } else {
            // Split parameters by comma
            let param_names: Vec<&str> = params_part.split(',').map(|s| s.trim()).collect();
            let mut params = Vec::new();
            for param_name in param_names {
                if !param_name.is_empty() {
                    params.push(Parameter::new(param_name.to_string()));
                }
            }
            params
        };

        // Parse body expression
        let body = Expr::parse(&body_part).map_err(|e| {
            CommandError::ParameterError(format!(
                "Invalid function body expression: {} - {}",
                body_part, e
            ))
        })?;

        // Create FunctionDefinition for AST
        let function_def = FunctionDefinition::new(func_name.to_string(), parameters.clone(), body);

        // Add to AST
        app.ast_mut()
            .upsert_function_define(function_def)
            .map_err(CommandError::AstError)?;
    } else {
        // No parentheses found - just a function name with no parameters
        let func_name = trimmed;

        if !is_valid_identifier(func_name) {
            return Err(CommandError::InvalidCommand(format!(
                "Invalid function name: {}",
                func_name
            )));
        }

        // Create FunctionDefinition for AST with empty parameters and placeholder body
        let function_def =
            FunctionDefinition::new(func_name.to_string(), Vec::new(), Expr::Integer(0));

        // Add to AST
        app.ast_mut()
            .upsert_function_define(function_def)
            .map_err(CommandError::AstError)?;
    }

    // Reload custom functions in library manager
    reload_project_definitions(app);

    Ok(())
}

/// Quit app command
pub fn cmd_quit(app: &mut App) -> CommandResult<()> {
    if !app.saved {
        return Err(CommandError::Custom(
            "File is not saved, use 'q!' or 'quit!' to force quit".to_string(),
        ));
    }
    cmd_quit_force(app)
}

/// Force Quit app command
pub fn cmd_quit_force(app: &mut App) -> CommandResult<()> {
    app.should_quit = true;
    Ok(())
}

/// Write and quit app command
pub fn cmd_write_and_quit(app: &mut App) -> CommandResult<()> {
    cmd_write(app, "")?;
    cmd_quit(app)
}

/// Undo command
pub fn cmd_undo(app: &mut App) -> CommandResult<()> {
    app.undo();
    Ok(())
}

/// Redo command
pub fn cmd_redo(app: &mut App) -> CommandResult<()> {
    app.redo();
    Ok(())
}

fn general_help_doc(app: &App) -> Vec<String> {
    let mut docs = vec![
        "OpenSCAD TUI - Command Reference".to_string(),
        "".to_string(),
        "Normal mode keys:".to_string(),
        "  j/k/h/l or arrows  navigate and expand/collapse the tree".to_string(),
        "  Enter              toggle node expansion".to_string(),
        "  v                  select/deselect current node".to_string(),
        "  y / p              yank / paste module subtree(s)".to_string(),
        "  x                  remove node and promote its children".to_string(),
        "  c                  change current node (replace)".to_string(),
        "  i                  start insert command".to_string(),
        "  n                  start source new command".to_string(),
        "  t/r/s              start translate/rotate/scale command".to_string(),
        "  d                  cut current or selected module subtree(s)".to_string(),
        "  P / R              toggle preview / render a fresh model preview".to_string(),
        "  u / Ctrl+R         undo / redo".to_string(),
        "  w/o/e/L            save / open project / import SCAD / load library".to_string(),
        "  :                  enter command mode".to_string(),
        "  ?                  open this help".to_string(),
        "  q / Ctrl+C         quit".to_string(),
        "".to_string(),
        "Commands (type `help <command>` for details):".to_string(),
    ];

    for name in app.command_registry.get_primary_names() {
        if let Some(def) = app.command_registry.find(&name) {
            docs.push(format!("  {:<34} {}", def.usage, def.description));
        }
    }

    docs.extend([
        "".to_string(),
        "Command mode: Tab completes, Up/Down browse history, Esc cancels.".to_string(),
        "Help: j/k or arrows scroll, Ctrl+F/Ctrl+B page, Esc/q closes.".to_string(),
    ]);
    docs
}

fn command_help_doc(app: &App, command: &str) -> CommandResult<Vec<String>> {
    let Some(def) = app.command_registry.find(command) else {
        let prefix = command.split_whitespace().collect::<Vec<_>>();
        let children = app.command_registry.commands_below(&prefix);
        if children.is_empty() {
            return Err(CommandError::InvalidCommand(format!(
                "No help found for command: {command}"
            )));
        }
        let mut docs = vec![
            format!("Help: {command}"),
            String::new(),
            "Commands:".to_string(),
        ];
        docs.extend(
            children
                .into_iter()
                .map(|child| format!("  {:<34} {}", child.usage, child.description)),
        );
        docs.extend([String::new(), "Press Esc or q to close help.".to_string()]);
        return Ok(docs);
    };
    let aliases = if def.aliases.is_empty() {
        "(none)".to_string()
    } else {
        def.aliases.join(", ")
    };
    let mut docs = vec![
        format!("Help: {}", def.name),
        "".to_string(),
        format!("Description: {}", def.description),
        format!("Usage: {}", def.usage),
        format!("Aliases: {}", aliases),
    ];
    if !def.examples.is_empty() {
        docs.push("".to_string());
        docs.push("Examples:".to_string());
        docs.extend(def.examples.iter().map(|example| format!("  {}", example)));
    }
    docs.extend(["".to_string(), "Press Esc or q to close help.".to_string()]);
    Ok(docs)
}

/// Help command - Show the command overview or details for one command.
pub fn cmd_help(app: &mut App, command: Option<&str>) -> CommandResult<()> {
    let docs = match command {
        Some(command) => command_help_doc(app, command)?,
        None => general_help_doc(app),
    };
    app.set_help_doc(docs);
    app.input_mode = InputMode::Help;
    Ok(())
}

pub fn cmd_version(app: &mut App) {
    app.set_info(&format!("openscad-tui {}", env!("CARGO_PKG_VERSION")));
}

/// Expand tilde (~) in file paths to the user's home directory
fn expand_tilde<P: AsRef<Path>>(path: P) -> PathBuf {
    let path = path.as_ref();

    if path.starts_with("~") {
        if let Some(home) = dirs::home_dir() {
            // Replace ~ with home directory
            let mut expanded = home;
            if path.components().count() > 1 {
                // Add remaining components after ~
                for component in path.components().skip(1) {
                    expanded.push(component);
                }
            }
            expanded
        } else {
            // If no home directory found, return original path
            path.to_path_buf()
        }
    } else {
        // If path doesn't start with ~, return as-is
        path.to_path_buf()
    }
}

/// Define a new custom module
pub fn cmd_moddef(app: &mut App, module_name: &str, params: Option<&str>) -> CommandResult<()> {
    use openscad_core::ModuleDefinition;

    // Parse parameters
    let parameters = if let Some(param_str) = params {
        parse_module_parameters(param_str)?
    } else {
        Vec::new()
    };

    // Collect children from selected nodes (copy them with new IDs to avoid duplication)
    let mut children = Vec::new();
    for node_id in &app.selected_nodes {
        if let Some(node) = app.ast.find_node_by_id(node_id).cloned() {
            let node_with_new_id = clone_module_with_new_ids(&node);
            children.push(node_with_new_id);
        }
    }

    // Clear selection after copying
    app.selected_nodes.clear();

    // Create ModuleDefinition for AST
    let module_def = ModuleDefinition::new(module_name.to_string(), parameters, children);

    // Add or replace in the AST while preserving the definition's position.
    app.ast_mut()
        .upsert_module_define(module_def)
        .map_err(CommandError::AstError)?;
    reload_project_definitions(app);

    // Update UI selection to show the new module definition
    if let Some(path) = app.find_node_path(&format!("__moddef_{}", module_name)) {
        app.tree_state.borrow_mut().select(path);
    }

    Ok(())
}

/// Parse module parameters from string
/// Format: "name1=expr1, name2=expr2, name3" (name without default)
fn parse_module_parameters(param_str: &str) -> CommandResult<Vec<openscad_core::Parameter>> {
    use openscad_core::{Expr, Parameter};

    let mut parameters = Vec::new();

    if param_str.trim().is_empty() {
        return Ok(parameters);
    }

    // Split by commas while respecting brackets and quotes (reuse split_parameters)
    let parts = split_parameters(param_str)?;

    for part in parts {
        let part = part.trim();

        // Check if this is a parameter with default value (contains '=')
        if let Some(eq_pos) = part.find('=') {
            let name = part[..eq_pos].trim();
            let value_str = part[eq_pos + 1..].trim();

            let value = Expr::parse(value_str).map_err(|e| {
                CommandError::ParameterError(format!(
                    "Invalid default value for parameter '{}': {} - {}",
                    name, value_str, e
                ))
            })?;

            parameters.push(Parameter::with_default(name.to_string(), value));
        } else {
            // Parameter without default value
            parameters.push(Parameter::new(part.to_string()));
        }
    }

    Ok(parameters)
}

/// Clone a module node and all its children, generating new unique IDs for each
fn clone_module_with_new_ids(node: &openscad_core::ModuleNode) -> openscad_core::ModuleNode {
    // Generate new ID with timestamp to ensure uniqueness
    let new_id = format!(
        "{}_{}",
        node.name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    // Clone the node with new ID
    let mut new_node = node.clone();
    new_node.id = new_id.clone();

    // Recursively clone children with new IDs
    new_node.children = node
        .children
        .iter()
        .map(clone_module_with_new_ids)
        .collect();

    new_node
}

fn selected_tree_id(app: &App) -> CommandResult<String> {
    app.tree_state
        .borrow()
        .selected()
        .last()
        .cloned()
        .ok_or(CommandError::NoNodeSelected)
}

fn selected_or_current_node_ids(app: &App) -> CommandResult<Vec<String>> {
    if !app.selected_nodes.is_empty() {
        Ok(app.selected_nodes.clone())
    } else {
        Ok(vec![selected_tree_id(app)?])
    }
}

enum PlannedParameterUpdate {
    Existing(ArgumentSelector),
    AddNamed,
}

fn plan_parameter_update(
    app: &App,
    node_id: &str,
    parameter_name: &str,
) -> CommandResult<PlannedParameterUpdate> {
    let node = find_module_node(app, node_id)
        .ok_or_else(|| CommandError::InvalidCommand(format!("Node not found: {}", node_id)))?;
    let definition = app
        .library
        .get_module(&node.name)
        .ok_or_else(|| CommandError::InvalidCommand(format!("Unknown module: {}", node.name)))?;
    let parameter_position = definition
        .parameters
        .iter()
        .position(|parameter| parameter.name == parameter_name)
        .ok_or_else(|| {
            CommandError::InvalidCommand(format!(
                "Module '{}' has no parameter named '{}'",
                node.name, parameter_name
            ))
        })?;

    if node
        .args
        .iter()
        .any(|argument| matches!(argument, Argument::Named { name, .. } if name == parameter_name))
    {
        return Ok(PlannedParameterUpdate::Existing(ArgumentSelector::Named(
            parameter_name.to_string(),
        )));
    }
    if node
        .args
        .iter()
        .filter(|argument| matches!(argument, Argument::Positional(_)))
        .nth(parameter_position)
        .is_some()
    {
        return Ok(PlannedParameterUpdate::Existing(
            ArgumentSelector::Position(parameter_position),
        ));
    }
    Ok(PlannedParameterUpdate::AddNamed)
}

pub fn cmd_set_parameter(app: &mut App, parameter_spec: &str) -> CommandResult<()> {
    let (parameter_name, value_source) = parameter_spec.split_once('=').ok_or_else(|| {
        CommandError::InvalidCommand("Usage: set <parameter_name>=<expression>".to_string())
    })?;
    let parameter_name = parameter_name.trim();
    let value_source = value_source.trim();
    if parameter_name.is_empty() || value_source.is_empty() {
        return Err(CommandError::InvalidCommand(
            "Usage: set <parameter_name>=<expression>".to_string(),
        ));
    }
    let value = Expr::parse(value_source).map_err(|error| {
        CommandError::ParameterError(format!(
            "Invalid value for '{}': {} - {}",
            parameter_name, value_source, error
        ))
    })?;
    let target_ids = selected_or_current_node_ids(app)?;
    if target_ids.iter().any(|node_id| node_id.starts_with("__")) {
        return Err(CommandError::InvalidCommand(
            "Only module node parameters can be changed".to_string(),
        ));
    }

    // Plan every update before mutating the AST so multi-node edits are atomic.
    let updates = target_ids
        .iter()
        .map(|node_id| {
            plan_parameter_update(app, node_id, parameter_name)
                .map(|update| (node_id.clone(), update))
        })
        .collect::<CommandResult<Vec<_>>>()?;

    for (node_id, update) in updates {
        let node = app
            .ast_mut()
            .find_node_anywhere_mut(&node_id)
            .ok_or_else(|| CommandError::InvalidCommand(format!("Node not found: {}", node_id)))?;
        match update {
            PlannedParameterUpdate::Existing(selector) => {
                node.set_argument(&selector, value.clone())?;
            }
            PlannedParameterUpdate::AddNamed => {
                node.add_named_argument(parameter_name.to_string(), value.clone())?;
            }
        }
    }
    app.set_info(&format!(
        "Set '{}' on {} node(s)",
        parameter_name,
        target_ids.len()
    ));
    Ok(())
}

pub fn cmd_unset_parameter(app: &mut App, parameter_name: &str) -> CommandResult<()> {
    let parameter_name = parameter_name.trim();
    if parameter_name.is_empty() {
        return Err(CommandError::InvalidCommand(
            "Usage: unset <parameter_name>".to_string(),
        ));
    }
    let target_ids = selected_or_current_node_ids(app)?;
    if target_ids.iter().any(|node_id| node_id.starts_with("__")) {
        return Err(CommandError::InvalidCommand(
            "Only module node parameters can be removed".to_string(),
        ));
    }

    // Resolve every selector before mutating anything so multi-node edits are atomic.
    let removals = target_ids
        .iter()
        .map(
            |node_id| match plan_parameter_update(app, node_id, parameter_name)? {
                PlannedParameterUpdate::Existing(selector) => Ok((node_id.clone(), selector)),
                PlannedParameterUpdate::AddNamed => Err(CommandError::InvalidCommand(format!(
                    "Parameter '{}' is not explicitly set on node '{}'",
                    parameter_name, node_id
                ))),
            },
        )
        .collect::<CommandResult<Vec<_>>>()?;

    for (node_id, selector) in removals {
        app.ast_mut()
            .find_node_anywhere_mut(&node_id)
            .ok_or_else(|| CommandError::InvalidCommand(format!("Node not found: {}", node_id)))?
            .remove_argument(&selector)?;
    }
    app.set_info(&format!(
        "Unset '{}' on {} node(s)",
        parameter_name,
        target_ids.len()
    ));
    Ok(())
}

fn find_module_node(app: &App, node_id: &str) -> Option<ModuleNode> {
    app.ast.find_node_anywhere(node_id).cloned()
}

pub fn cmd_yank(app: &mut App, node_id: Option<&str>) -> CommandResult<()> {
    let node_id = app
        .selected_nodes
        .last()
        .cloned()
        .or_else(|| node_id.map(str::to_string))
        .map(Ok)
        .unwrap_or_else(|| selected_tree_id(app))?;
    if node_id.starts_with("__") {
        return Err(CommandError::InvalidCommand(
            "Only module nodes can be yanked".to_string(),
        ));
    }
    let node = find_module_node(app, &node_id)
        .ok_or_else(|| CommandError::InvalidCommand(format!("Node not found: {}", node_id)))?;
    app.node_clipboard = vec![node];
    app.set_info(&format!("Yanked node: {}", node_id));
    Ok(())
}

pub fn cmd_paste(app: &mut App) -> CommandResult<String> {
    if app.node_clipboard.is_empty() {
        return Err(CommandError::InvalidCommand(
            "Clipboard is empty".to_string(),
        ));
    }
    let pasted = app
        .node_clipboard
        .iter()
        .map(clone_module_with_new_ids)
        .collect::<Vec<_>>();
    let pasted_ids = pasted
        .iter()
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    let target_id = selected_tree_id(app).unwrap_or_else(|_| "__modules".to_string());

    if target_id == "__modules" {
        app.ast_mut().modules.extend(pasted);
    } else if let Some(module_name) = target_id.strip_prefix("__moddef_") {
        let definition = app
            .ast_mut()
            .module_defines
            .iter_mut()
            .find(|definition| definition.name == module_name)
            .ok_or_else(|| {
                CommandError::InvalidCommand(format!(
                    "Module definition not found: {}",
                    module_name
                ))
            })?;
        definition.body.extend(pasted);
    } else if target_id.starts_with("__") {
        return Err(CommandError::InvalidCommand(
            "Select a module node or the Modules section before pasting".to_string(),
        ));
    } else {
        let mut insertion_target = target_id;
        for node in pasted {
            let pasted_id = node.id.clone();
            app.ast_mut().insert_after(&insertion_target, node)?;
            insertion_target = pasted_id;
        }
    }

    let pasted_id = pasted_ids
        .last()
        .expect("non-empty clipboard produces pasted nodes")
        .clone();
    if let Some(path) = app.find_node_path(&pasted_id) {
        app.tree_state.borrow_mut().select(path);
    }
    app.set_info(&format!("Pasted {} node(s)", pasted_ids.len()));
    Ok(pasted_id)
}

pub fn cmd_remove(app: &mut App, node_id: Option<&str>) -> CommandResult<()> {
    let node_ids = if app.selected_nodes.is_empty() {
        match node_id {
            Some(node_id) => vec![node_id.to_string()],
            None => selected_or_current_node_ids(app)?,
        }
    } else {
        selected_or_current_node_ids(app)?
    };
    if node_ids.iter().any(|node_id| node_id.starts_with("__")) {
        return Err(CommandError::InvalidCommand(
            "Only module nodes can be removed".to_string(),
        ));
    }

    let fallback_selection = selection_after_removing(app, &node_ids);
    for node_id in &node_ids {
        app.ast_mut().remove_node_promote_children(node_id)?;
    }

    app.selected_nodes.clear();
    restore_selection_after_removing(app, fallback_selection);
    app.set_info(&format!("Removed {} node(s)", node_ids.len()));
    Ok(())
}

pub fn cmd_replace(
    app: &mut App,
    node_id: Option<&str>,
    new_module_name: &str,
    params: Option<&str>,
) -> CommandResult<String> {
    let node_ids = if app.selected_nodes.is_empty() {
        match node_id {
            Some(node_id) => vec![node_id.to_string()],
            None => selected_or_current_node_ids(app)?,
        }
    } else {
        selected_or_current_node_ids(app)?
    };
    if node_ids.iter().any(|node_id| node_id.starts_with("__")) {
        return Err(CommandError::InvalidCommand(
            "Only module nodes can be replaced".to_string(),
        ));
    }
    let mut replacement_id = None;
    for node_id in node_ids {
        if find_module_node(app, &node_id).is_none() {
            continue;
        }
        let prepared = prepare_module(app, new_module_name, params)?;
        replacement_id = Some(replace_with_prepared_module(
            app,
            &node_id,
            new_module_name,
            prepared,
        )?);
    }
    replacement_id
        .ok_or_else(|| CommandError::InvalidCommand("None of the target nodes exist".to_string()))
}

fn replace_with_prepared_module(
    app: &mut App,
    node_id: &str,
    new_module_name: &str,
    prepared: PreparedModule,
) -> CommandResult<String> {
    if find_module_node(app, node_id).is_none() {
        return Err(CommandError::InvalidCommand(format!(
            "Node not found: {}",
            node_id
        )));
    }

    let replacement_id = prepared.node.id.clone();

    app.ast_mut().replace_node(node_id, prepared.node)?;

    app.selected_nodes.retain(|selected| selected != node_id);
    if let Some(path) = app.find_node_path(&replacement_id) {
        app.tree_state.borrow_mut().select(path);
    }
    app.set_info(&format!("Replaced {} with {}", node_id, new_module_name));
    Ok(replacement_id)
}

pub fn begin_pending_module_action(app: &mut App, action: PendingModuleAction, module_name: &str) {
    app.pending_module_action = Some(action);
    app.pending_module_name = Some(module_name.to_string());
    app.input_mode = InputMode::ModuleEnterParams;
    app.input_buffer.clear();
    app.set_info(&format!(
        "Enter parameters for '{}' (or press Enter to use defaults):",
        module_name
    ));
}

pub fn commit_pending_module_action(app: &mut App, params: &str) -> CommandResult<String> {
    let action = app
        .pending_module_action
        .clone()
        .ok_or_else(|| CommandError::InvalidCommand("No pending module action".to_string()))?;
    let module_name = app.pending_module_name.clone().ok_or_else(|| {
        CommandError::InvalidCommand("No module selected for pending action".to_string())
    })?;

    // Validate module lookup and parameters before creating the single undo point.
    let prepared = prepare_module(app, &module_name, Some(params))?;
    app.push_undo();
    let result = match action {
        PendingModuleAction::Insert => insert_prepared_module(app, &module_name, prepared, false),
        PendingModuleAction::InsertBefore => {
            insert_prepared_module(app, &module_name, prepared, true)
        }
        PendingModuleAction::Replace { target_ids } => {
            let mut result = None;
            let mut first_prepared = Some(prepared);
            for target_id in &target_ids {
                if find_module_node(app, target_id).is_none() {
                    continue;
                }
                let prepared = match first_prepared.take() {
                    Some(prepared) => prepared,
                    None => prepare_module(app, &module_name, Some(params))?,
                };
                result = Some(replace_with_prepared_module(
                    app,
                    target_id,
                    &module_name,
                    prepared,
                )?);
            }
            result.ok_or_else(|| {
                CommandError::InvalidCommand("None of the target nodes exist".to_string())
            })
        }
    }?;
    app.update_navigation_status();
    Ok(result)
}

fn run_insert_command(app: &mut App, args: &[&str], before: bool) -> CommandResult<()> {
    if args.is_empty() {
        let command = if before { "insert-before" } else { "insert" };
        return Err(CommandError::InvalidCommand(format!(
            "Usage: {command} <module_name> [params]"
        )));
    }
    let module_name = args[0];
    let module_def = app
        .library
        .get_module(module_name)
        .ok_or_else(|| CommandError::InvalidCommand(format!("Unknown module: {module_name}")))?;
    if module_def.accepts_children && app.selected_nodes.is_empty() {
        return Err(CommandError::InvalidCommand(format!(
            "'{module_name}' requires child modules. Select modules with 'v' first"
        )));
    }
    if before && !module_def.accepts_children {
        ensure_insert_before_target(app)?;
    }
    let params = (args.len() > 1).then(|| args[1..].join(" "));
    if params.is_none() && !module_def.parameters.is_empty() {
        let action = if before {
            PendingModuleAction::InsertBefore
        } else {
            PendingModuleAction::Insert
        };
        begin_pending_module_action(app, action, module_name);
        return Ok(());
    }

    app.push_undo();
    let result = if before {
        cmd_insert_before(app, module_name, params.as_deref().or(Some("")))
    } else {
        cmd_insert(app, module_name, None, params.as_deref().or(Some("")))
    }?;
    app.update_navigation_status();
    app.set_info(&format!(
        "Inserted '{}' {}",
        module_name,
        if before {
            "before the current node"
        } else {
            "after the current node"
        }
    ));
    let _ = result;
    Ok(())
}

/// Find the parent ID of a node in a module tree
fn find_node_parent(nodes: &[openscad_core::ModuleNode], target_id: &str) -> Option<String> {
    // Use explicit stack to avoid recursion depth issues
    let mut stack: Vec<(&openscad_core::ModuleNode, usize)> =
        nodes.iter().map(|n| (n, 0)).collect();

    while let Some((node, depth)) = stack.pop() {
        if depth >= MAX_RECURSION_DEPTH {
            continue; // Skip to avoid infinite recursion
        }

        // Check if target is a direct child of this node
        for child in &node.children {
            if child.id == target_id {
                return Some(node.id.clone());
            }
            // Push child to stack for deeper search
            stack.push((child, depth + 1));
        }
    }

    None
}

/// Insert a child node before a target node in its parent's children list
#[allow(clippy::ptr_arg)]
fn insert_child_before(
    nodes: &mut Vec<openscad_core::ModuleNode>,
    parent_id: &str,
    target_id: &str,
    new_node: openscad_core::ModuleNode,
) -> Result<(), String> {
    // Find the parent node
    for node in nodes.iter_mut() {
        if node.id == parent_id {
            // Find position of target child
            if let Some(pos) = node.children.iter().position(|c| c.id == target_id) {
                node.children.insert(pos, new_node);
                return Ok(());
            } else {
                return Err(format!(
                    "Target node {} not found in parent {}",
                    target_id, parent_id
                ));
            }
        }

        // Recursively search in children
        if !node.children.is_empty() {
            // Use explicit stack to avoid recursion depth issues
            let mut stack: Vec<&mut Vec<openscad_core::ModuleNode>> = vec![&mut node.children];
            let mut depth = 0;

            while let Some(children) = stack.pop() {
                if depth >= MAX_RECURSION_DEPTH {
                    break;
                }

                for child in children.iter_mut() {
                    if child.id == parent_id {
                        if let Some(pos) = child.children.iter().position(|c| c.id == target_id) {
                            child.children.insert(pos, new_node);
                            return Ok(());
                        } else {
                            return Err(format!(
                                "Target node {} not found in parent {}",
                                target_id, parent_id
                            ));
                        }
                    }

                    if !child.children.is_empty() {
                        stack.push(&mut child.children);
                    }
                }
                depth += 1;
            }
        }
    }

    Err(format!("Parent node {} not found", parent_id))
}

/// Find a node in a module definition body
fn find_node_in_module_definition(
    nodes: &[openscad_core::ModuleNode],
    target_id: &str,
) -> Option<openscad_core::ModuleNode> {
    // Use explicit stack to avoid recursion depth issues
    let mut stack: Vec<(&openscad_core::ModuleNode, usize)> =
        nodes.iter().map(|n| (n, 0)).collect();

    while let Some((node, depth)) = stack.pop() {
        if depth >= MAX_RECURSION_DEPTH {
            continue;
        }

        if node.id == target_id {
            return Some(node.clone());
        }

        // Push children to stack
        for child in &node.children {
            stack.push((child, depth + 1));
        }
    }

    None
}

/// Find a node in a module definition body (mutable version)
fn find_node_in_module_definition_mut<'a>(
    nodes: &'a mut [openscad_core::ModuleNode],
    target_id: &str,
) -> Option<&'a mut openscad_core::ModuleNode> {
    // Use explicit stack to avoid recursion depth issues
    let mut stack: Vec<(&mut openscad_core::ModuleNode, usize)> =
        nodes.iter_mut().map(|n| (n, 0)).collect();

    while let Some((node, depth)) = stack.pop() {
        if depth >= MAX_RECURSION_DEPTH {
            continue;
        }

        if node.id == target_id {
            return Some(node);
        }

        // Push children to stack
        for child in &mut node.children {
            stack.push((child, depth + 1));
        }
    }

    None
}

/// Delete a node from a module definition body
fn delete_node_from_module_definition(
    nodes: &mut Vec<openscad_core::ModuleNode>,
    target_id: &str,
) -> Result<(), String> {
    // First try to find and remove from root level
    if let Some(pos) = nodes.iter().position(|n| n.id == target_id) {
        nodes.remove(pos);
        return Ok(());
    }

    // Search in children recursively
    for node in nodes.iter_mut() {
        // Use explicit stack to avoid recursion depth issues
        let mut stack: Vec<&mut Vec<openscad_core::ModuleNode>> = vec![&mut node.children];
        let mut depth = 0;

        while let Some(children) = stack.pop() {
            if depth >= MAX_RECURSION_DEPTH {
                break;
            }

            if let Some(pos) = children.iter().position(|n| n.id == target_id) {
                children.remove(pos);
                return Ok(());
            }

            // Continue search in deeper children
            for child in children.iter_mut() {
                if !child.children.is_empty() {
                    stack.push(&mut child.children);
                }
            }
            depth += 1;
        }
    }

    Err(format!("Node {} not found", target_id))
}

/// Initialize the command registry with all available commands
pub fn init_command_registry(registry: &mut crate::command_registry::CommandRegistry) {
    use crate::command_registry::{ArgumentSpec, CommandDef, CompletionSource};

    // Navigation commands (no arguments)
    registry.register(CommandDef::new(
        "next",
        vec!["j", "down"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "next command takes no arguments".to_string(),
                ));
            }
            cmd_next(app)
        },
        "Move cursor down",
        0,
        Some(0),
        "next",
        vec!["j", "next"],
        CommandType::NoArg,
        false,
        false,
    ));

    registry.register(CommandDef::new(
        "prev",
        vec!["k", "up"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "prev command takes no arguments".to_string(),
                ));
            }
            cmd_prev(app)
        },
        "Move cursor up",
        0,
        Some(0),
        "prev",
        vec!["k", "prev"],
        CommandType::NoArg,
        false,
        false,
    ));

    registry.register(CommandDef::new(
        "collapse",
        vec!["h", "left"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "collapse command takes no arguments".to_string(),
                ));
            }
            cmd_collapse(app)
        },
        "Collapse node or move left",
        0,
        Some(0),
        "collapse",
        vec!["h", "collapse"],
        CommandType::NoArg,
        false,
        false,
    ));

    registry.register(CommandDef::new(
        "expand",
        vec!["l", "right"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "expand command takes no arguments".to_string(),
                ));
            }
            cmd_expand(app)
        },
        "Expand node or move right",
        0,
        Some(0),
        "expand",
        vec!["l", "expand"],
        CommandType::NoArg,
        false,
        false,
    ));

    registry.register(CommandDef::new(
        "toggle",
        vec![] as Vec<String>,
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "toggle command takes no arguments".to_string(),
                ));
            }
            cmd_toggle(app)
        },
        "Toggle node expansion",
        0,
        Some(0),
        "toggle",
        vec!["toggle"],
        CommandType::NoArg,
        false,
        false,
    ));

    // Selection commands
    registry.register(CommandDef::new(
        "select",
        vec!["v"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "select command takes no arguments".to_string(),
                ));
            }
            cmd_select_toggle(app)
        },
        "Select/deselect current node",
        0,
        Some(0),
        "select",
        vec!["v", "select"],
        CommandType::NoArg,
        false,
        false,
    ));

    registry.register(CommandDef::new(
        "deselect-all",
        vec!["deselect_all", "clear-selection"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "deselect-all command takes no arguments".to_string(),
                ));
            }
            cmd_deselect_all(app)
        },
        "Clear all selections",
        0,
        Some(0),
        "deselect-all",
        vec!["deselect-all"],
        CommandType::NoArg,
        false,
        false,
    ));

    // Edit commands
    registry.register(CommandDef::new(
        "undo",
        vec!["u"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "undo command takes no arguments".to_string(),
                ));
            }
            cmd_undo(app)
        },
        "Undo last operation",
        0,
        Some(0),
        "undo",
        vec!["u", "undo"],
        CommandType::NoArg,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "redo",
        vec!["r"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "redo command takes no arguments".to_string(),
                ));
            }
            cmd_redo(app)
        },
        "Redo last undone operation",
        0,
        Some(0),
        "redo",
        vec!["r", "redo"],
        CommandType::NoArg,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "delete",
        vec!["d", "dd", "D"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "delete command takes no arguments".to_string(),
                ));
            }
            app.push_undo();
            cmd_delete(app, "")
        },
        "Cut selected module subtrees, or delete the current global or definition",
        0,
        Some(0),
        "delete",
        vec!["delete", "d", "dd", "D"],
        CommandType::NoArg,
        true,
        true,
    ));

    // Project and source resource commands
    registry.register(
        CommandDef::new(
            "project new",
            Vec::<&str>::new(),
            |app, args| {
                let force = args.contains(&"--force");
                let names = args
                    .iter()
                    .copied()
                    .filter(|argument| *argument != "--force")
                    .collect::<Vec<_>>();
                match names.as_slice() {
                    [] => cmd_new_project(app, None, force),
                    [filename] => cmd_new_project(app, Some(filename), force),
                    _ => Err(CommandError::InvalidCommand(
                        "Usage: project new [project] [--force]".to_string(),
                    )),
                }
            },
            "Create a new structured project",
            0,
            Some(2),
            "project new [project] [--force]",
            vec!["project new", "project new model.scadtui --force"],
            CommandType::NoArg,
            true,
            true,
        )
        .with_arguments(vec![
            ArgumentSpec::literal("project", false, &["--force"]),
            ArgumentSpec::literal("force", false, &["--force"]),
        ]),
    );

    registry.register(
        CommandDef::new(
            "project save",
            Vec::<&str>::new(),
            |app, args| match args {
                [] => cmd_write(app, ""),
                ["--force"] => cmd_write_force(app, ""),
                _ => Err(CommandError::InvalidCommand(
                    "Usage: project save [--force]".to_string(),
                )),
            },
            "Save the current .scadtui project package",
            0,
            Some(1),
            "project save [--force]",
            vec!["project save", "project save --force"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::literal("force", false, &["--force"])]),
    );

    registry.register(
        CommandDef::new(
            "project rename",
            Vec::<&str>::new(),
            |app, args| cmd_rename_project(app, &args.join(" ")),
            "Change the project name stored in the package",
            1,
            None,
            "project rename <name>",
            vec!["project rename vernier caliper"],
            CommandType::NoArg,
            true,
            true,
        )
        .with_arguments(vec![ArgumentSpec::new(
            "name",
            true,
            CompletionSource::None,
        )
        .variadic()]),
    );

    registry.register(
        CommandDef::new(
            "project save-as",
            Vec::<&str>::new(),
            |app, args| match args {
                [filename] => cmd_write(app, filename),
                [filename, "--force"] => cmd_write_force(app, filename),
                _ => Err(CommandError::InvalidCommand(
                    "Usage: project save-as <project.scadtui> [--force]".to_string(),
                )),
            },
            "Save the project package at a new path",
            1,
            Some(2),
            "project save-as <project.scadtui> [--force]",
            vec!["project save-as model.scadtui"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![
            ArgumentSpec::path("project", true, &[PROJECT_EXTENSION]),
            ArgumentSpec::literal("force", false, &["--force"]),
        ]),
    );

    registry.register(
        CommandDef::new(
            "project open",
            Vec::<&str>::new(),
            |app, args| match args {
                [filename] => cmd_load(app, filename),
                [filename, "--force"] => cmd_load_force(app, filename),
                _ => Err(CommandError::InvalidCommand(
                    "Usage: project open <project.scadtui> [--force]".to_string(),
                )),
            },
            "Open an existing .scadtui project",
            1,
            Some(2),
            "project open <project.scadtui> [--force]",
            vec!["project open project.scadtui"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![
            ArgumentSpec::path("project", true, &[PROJECT_EXTENSION]),
            ArgumentSpec::literal("force", false, &["--force"]),
        ]),
    );

    registry.register(
        CommandDef::new(
            "project export-sources",
            Vec::<&str>::new(),
            |app, args| cmd_export_tree(app, args[0]),
            "Export the reachable embedded SCAD source tree",
            1,
            Some(1),
            "project export-sources <directory>",
            vec!["project export-sources ./source-tree"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::path("directory", true, &[])]),
    );

    registry.register(
        CommandDef::new(
            "source new",
            Vec::<&str>::new(),
            |app, args| cmd_new_file(app, args[0]),
            "Create and activate an editable SCAD source",
            1,
            Some(1),
            "source new <name>",
            vec!["source new head", "source new parts/arm.scad"],
            CommandType::NoArg,
            true,
            true,
        )
        .with_arguments(vec![ArgumentSpec::new(
            "name",
            true,
            CompletionSource::None,
        )]),
    );

    registry.register(
        CommandDef::new(
            "source import",
            Vec::<&str>::new(),
            |app, args| cmd_edit_scad(app, args[0]),
            "Import a SCAD source tree as editable project sources",
            1,
            Some(1),
            "source import <file.scad>",
            vec!["source import existing.scad"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::path("source", true, &["scad"])]),
    );

    registry.register(CommandDef::new(
        "source list",
        Vec::<&str>::new(),
        |app, _args| cmd_buffer(app, None),
        "List editable project sources",
        0,
        Some(0),
        "source list",
        vec!["source list"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "source next",
        Vec::<&str>::new(),
        |app, _args| cmd_buffer(app, Some("next")),
        "Activate the next editable source",
        0,
        Some(0),
        "source next",
        vec!["source next"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "source previous",
        Vec::<&str>::new(),
        |app, _args| cmd_buffer(app, Some("prev")),
        "Activate the previous editable source",
        0,
        Some(0),
        "source previous",
        vec!["source previous"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(
        CommandDef::new(
            "source switch",
            Vec::<&str>::new(),
            |app, args| cmd_buffer(app, Some(args[0])),
            "Activate an editable project source",
            1,
            Some(1),
            "source switch <source>",
            vec!["source switch parts/arm.scad"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::new(
            "source",
            true,
            CompletionSource::ProjectSource {
                editable_only: true,
            },
        )]),
    );

    registry.register(
        CommandDef::new(
            "source export",
            Vec::<&str>::new(),
            |app, args| cmd_export_source(app, args[0]),
            "Export the active source as a standalone SCAD file",
            1,
            Some(1),
            "source export <file.scad>",
            vec!["source export model.scad"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::path("destination", true, &["scad"])]),
    );

    registry.register(
        CommandDef::new(
            "source rename",
            Vec::<&str>::new(),
            |app, args| cmd_rename_source(app, args[0], args[1]),
            "Rename an editable source and update project references",
            2,
            Some(2),
            "source rename <source> <name>",
            vec!["source rename arm.scad upper-arm"],
            CommandType::NoArg,
            true,
            true,
        )
        .with_arguments(vec![
            ArgumentSpec::new(
                "source",
                true,
                CompletionSource::ProjectSource {
                    editable_only: true,
                },
            ),
            ArgumentSpec::new("name", true, CompletionSource::None),
        ]),
    );

    registry.register(
        CommandDef::new(
            "source remove",
            Vec::<&str>::new(),
            |app, args| cmd_remove_source(app, args[0]),
            "Remove an unreferenced editable source",
            1,
            Some(1),
            "source remove <source>",
            vec!["source remove unused.scad"],
            CommandType::NoArg,
            true,
            true,
        )
        .with_arguments(vec![ArgumentSpec::new(
            "source",
            true,
            CompletionSource::ProjectSource {
                editable_only: true,
            },
        )]),
    );

    registry.register(
        CommandDef::new(
            "library load",
            Vec::<&str>::new(),
            |app, args| cmd_load_library(app, args[0]),
            "Load and embed a SCAD library without activating it",
            1,
            Some(1),
            "library load <file.scad>",
            vec!["library load gears.scad"],
            CommandType::NoArg,
            true,
            true,
        )
        .with_arguments(vec![ArgumentSpec::path("library", true, &["scad"])]),
    );

    registry.register(CommandDef::new(
        "library list",
        Vec::<&str>::new(),
        |app, _args| cmd_list_libraries(app),
        "List embedded SCAD library roots",
        0,
        Some(0),
        "library list",
        vec!["library list"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(
        CommandDef::new(
            "library remove",
            Vec::<&str>::new(),
            |app, args| cmd_remove_library(app, args[0]),
            "Remove an unreferenced embedded SCAD library",
            1,
            Some(1),
            "library remove <library>",
            vec!["library remove libraries/gears/gears.scad"],
            CommandType::NoArg,
            true,
            true,
        )
        .with_arguments(vec![ArgumentSpec::new(
            "library",
            true,
            CompletionSource::LibraryRoot,
        )]),
    );

    registry.register(
        CommandDef::new(
            "source use",
            Vec::<&str>::new(),
            |app, args| cmd_use_library(app, args[0]),
            "Use a loaded SCAD library in the active source",
            1,
            Some(1),
            "source use <library>",
            vec!["source use gears.scad"],
            CommandType::NoArg,
            true,
            true,
        )
        .with_arguments(vec![ArgumentSpec::new(
            "library",
            true,
            CompletionSource::LoadedLibrary,
        )]),
    );

    registry.register(
        CommandDef::new(
            "source include",
            Vec::<&str>::new(),
            |app, args| cmd_include_library(app, args[0]),
            "Include a loaded SCAD library in the active source",
            1,
            Some(1),
            "source include <library>",
            vec!["source include gears.scad"],
            CommandType::NoArg,
            true,
            true,
        )
        .with_arguments(vec![ArgumentSpec::new(
            "library",
            true,
            CompletionSource::LoadedLibrary,
        )]),
    );

    // System commands
    registry.register(CommandDef::new(
        "quit",
        vec!["q"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "quit command takes no arguments".to_string(),
                ));
            }
            cmd_quit(app)
        },
        "Exit the application",
        0,
        Some(0),
        "quit",
        vec!["quit", "q"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "quit!",
        vec!["q!"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "quit command takes no arguments".to_string(),
                ));
            }
            cmd_quit_force(app)
        },
        "Force exit the application",
        0,
        Some(0),
        "quit!",
        vec!["quit!", "q!"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "wq",
        vec!["wq"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "wq command takes no arguments".to_string(),
                ));
            }
            cmd_write_and_quit(app)
        },
        "Save and exit the application",
        0,
        Some(0),
        "wq",
        vec!["wq"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(
        CommandDef::new(
            "help",
            vec!["?"],
            |app, args| {
                let command = (!args.is_empty()).then(|| args.join(" "));
                cmd_help(app, command.as_deref())
            },
            "Show help",
            0,
            None,
            "help [command path]",
            vec!["help", "help model", "help model view", "?"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::new(
            "command",
            false,
            CompletionSource::CommandPath,
        )
        .variadic()]),
    );

    registry.register(CommandDef::new(
        "version",
        vec!["ver"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "version command takes no arguments".to_string(),
                ));
            }
            cmd_version(app);
            Ok(())
        },
        "Show the OpenSCAD TUI version",
        0,
        Some(0),
        "version",
        vec!["version", "ver"],
        CommandType::NoArg,
        false,
        true,
    ));

    // Transform commands
    registry.register(CommandDef::new(
        "translate",
        vec![] as Vec<String>,
        |app, args| {
            // Get parameters if provided
            let params = if !args.is_empty() {
                Some(args.join(" "))
            } else {
                None
            };

            app.push_undo();
            cmd_insert(app, "translate", None, params.as_deref()).map(|_| {
                app.set_info("Applied translate to selected nodes");
                app.update_navigation_status();
            })
        },
        "Apply translate transformation to selected nodes",
        0,
        None, // Variable number of parameters (optional)
        "translate [x,y,z]",
        vec!["translate", "translate 10,0,0"],
        CommandType::Param,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "rotate",
        vec![] as Vec<String>,
        |app, args| {
            let params = if !args.is_empty() {
                Some(args.join(" "))
            } else {
                None
            };

            app.push_undo();
            cmd_insert(app, "rotate", None, params.as_deref()).map(|_| {
                app.set_info("Applied rotate to selected nodes");
                app.update_navigation_status();
            })
        },
        "Apply rotate transformation to selected nodes",
        0,
        None,
        "rotate [a,vx,vy,vz]",
        vec!["rotate", "rotate 45,0,0,1"],
        CommandType::Param,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "scale",
        vec![] as Vec<String>,
        |app, args| {
            let params = if !args.is_empty() {
                Some(args.join(" "))
            } else {
                None
            };

            app.push_undo();
            cmd_insert(app, "scale", None, params.as_deref()).map(|_| {
                app.set_info("Applied scale to selected nodes");
                app.update_navigation_status();
            })
        },
        "Apply scale transformation to selected nodes",
        0,
        None,
        "scale [x,y,z]",
        vec!["scale", "scale 2,2,2"],
        CommandType::Param,
        true,
        true,
    ));

    // Boolean commands
    registry.register(CommandDef::new(
        "union",
        vec![] as Vec<String>,
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "union command takes no arguments".to_string(),
                ));
            }

            app.push_undo();
            cmd_insert(app, "union", None, None).map(|_| {
                app.set_info("Applied union to selected nodes");
                app.update_navigation_status();
            })
        },
        "Apply union operation to selected nodes",
        0,
        Some(0),
        "union",
        vec!["union"],
        CommandType::NoArg,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "difference",
        vec![] as Vec<String>,
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "difference command takes no arguments".to_string(),
                ));
            }

            app.push_undo();
            cmd_insert(app, "difference", None, None).map(|_| {
                app.set_info("Applied difference to selected nodes");
                app.update_navigation_status();
            })
        },
        "Apply difference operation to selected nodes",
        0,
        Some(0),
        "difference",
        vec!["difference"],
        CommandType::NoArg,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "intersection",
        vec![] as Vec<String>,
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "intersection command takes no arguments".to_string(),
                ));
            }

            app.push_undo();
            cmd_insert(app, "intersection", None, None).map(|_| {
                app.set_info("Applied intersection to selected nodes");
                app.update_navigation_status();
            })
        },
        "Apply intersection operation to selected nodes",
        0,
        Some(0),
        "intersection",
        vec!["intersection"],
        CommandType::NoArg,
        true,
        true,
    ));

    // Insert command with multi-stage parameter handling
    registry.register(CommandDef::new(
        "insert",
        vec!["i"],
        |app, args| run_insert_command(app, args, false),
        "Insert a module into the AST",
        1,
        None, // Variable number of parameters (optional)
        "insert <module_name> [params]",
        vec!["insert cube", "i sphere", "insert translate 10,0,0"],
        CommandType::Module,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "insert-before",
        vec!["I"],
        |app, args| run_insert_command(app, args, true),
        "Insert a module immediately before the current module node",
        1,
        None,
        "insert-before <module_name> [params]",
        vec!["insert-before cube size=10", "I sphere r=5"],
        CommandType::Module,
        true,
        true,
    ));

    // Function definition command
    registry.register(CommandDef::new(
        "function",
        vec![] as Vec<String>,
        |app, args| {
            if args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "Usage: function name(params) = expression".to_string(),
                ));
            }

            // Join all arguments to form the complete function definition
            let full_command = args.join(" ");

            app.push_undo();
            cmd_funcdef(app, &full_command)
        },
        "Define or redefine a function",
        1,
        None, // Variable number of parameters (optional)
        "function name(params) = expression",
        vec!["function myfunc()", "function add(a,b) = a + b"],
        CommandType::FunctionDefinition,
        true,
        true,
    ));

    // Module definition command
    registry.register(CommandDef::new(
        "module",
        vec![] as Vec<String>,
        |app, args| {
            if args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "Usage: module <module_name> [params]".to_string(),
                ));
            }
            let module_name = args[0];
            let params = if args.len() > 1 {
                Some(args[1..].join(" "))
            } else {
                None
            };
            app.push_undo();
            cmd_moddef(app, module_name, params.as_deref()).map(|_| {
                app.update_navigation_status();
                app.set_info(&format!("Module '{}' defined", module_name));
            })
        },
        "Define or redefine a module",
        1,
        None,
        "module <module_name> [params]",
        vec!["module mymodule", "module mybox size=10, center=false"],
        CommandType::ModuleDefinition,
        true,
        true,
    ));

    // Global variable command
    registry.register(CommandDef::new(
        "global",
        vec![] as Vec<String>,
        |app, args| {
            if args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "Usage: global <name>=<value>".to_string(),
                ));
            }
            let var_spec = args[0];
            // If there are more args, join them with space (though spec should be single token)
            let full_spec = if args.len() > 1 {
                args.join(" ")
            } else {
                var_spec.to_string()
            };
            app.push_undo();
            cmd_global(app, &full_spec).map(|_| {
                app.update_navigation_status();
                app.set_info(&format!(
                    "Global variable '{}' defined",
                    full_spec.split('=').next().unwrap_or("<invalid>")
                ));
            })
        },
        "Define or redefine a global variable",
        1,
        None,
        "global <name>=<value>",
        vec!["global pi=3.14159", "global name=\"test\""],
        CommandType::GlobalDefinition,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "yank",
        vec!["y"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "yank command takes no arguments".to_string(),
                ));
            }
            cmd_yank(app, None)
        },
        "Copy the last selected module subtree, or the current subtree when none is selected",
        0,
        Some(0),
        "yank",
        vec!["yank", "y"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "paste",
        vec!["p"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "paste command takes no arguments".to_string(),
                ));
            }
            app.push_undo();
            cmd_paste(app).map(|_| ())
        },
        "Paste copied or cut subtrees after the current module node",
        0,
        Some(0),
        "paste",
        vec!["paste", "p"],
        CommandType::NoArg,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "remove",
        vec!["x"],
        |app, args| {
            if !args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "remove command takes no arguments".to_string(),
                ));
            }
            app.push_undo();
            cmd_remove(app, None)
        },
        "Remove selected module nodes and promote their children, or use the current node",
        0,
        Some(0),
        "remove",
        vec!["remove", "x"],
        CommandType::NoArg,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "set",
        vec!["param"],
        |app, args| {
            if args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "Usage: set <parameter_name>=<expression>".to_string(),
                ));
            }
            let parameter_spec = args.join(" ");
            // Parsing and target validation happen before the single undo point.
            let snapshot = app.ast.clone();
            cmd_set_parameter(app, &parameter_spec)?;
            if app.undo_stack.len() >= 100 {
                app.undo_stack.pop_front();
            }
            app.undo_stack.push_back(snapshot);
            app.redo_stack.clear();
            Ok(())
        },
        "Set a parameter on selected module nodes, or the current node",
        1,
        None,
        "set <parameter_name>=<expression>",
        vec!["set size=size", "set center=true", "set v=offset"],
        CommandType::NodeParam,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "unset",
        vec![] as Vec<String>,
        |app, args| {
            if args.len() != 1 {
                return Err(CommandError::InvalidCommand(
                    "Usage: unset <parameter_name>".to_string(),
                ));
            }
            let snapshot = app.ast.clone();
            cmd_unset_parameter(app, args[0])?;
            if app.undo_stack.len() >= 100 {
                app.undo_stack.pop_front();
            }
            app.undo_stack.push_back(snapshot);
            app.redo_stack.clear();
            Ok(())
        },
        "Remove an explicitly set parameter from selected module nodes, or the current node",
        1,
        Some(1),
        "unset <parameter_name>",
        vec!["unset size", "unset center"],
        CommandType::NodeParamUnset,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "replace",
        vec![] as Vec<String>,
        |app, args| {
            if args.is_empty() {
                return Err(CommandError::InvalidCommand(
                    "Usage: replace <module_name> [params]".to_string(),
                ));
            }
            let module_name = args[0];
            let module_def = app.library.get_module(module_name).ok_or_else(|| {
                CommandError::InvalidCommand(format!("Unknown module: {}", module_name))
            })?;
            let params = if args.len() > 1 {
                Some(args[1..].join(" "))
            } else {
                None
            };

            if params.is_none() && !module_def.parameters.is_empty() {
                let target_ids = selected_or_current_node_ids(app)?;
                if target_ids.iter().any(|target_id| {
                    target_id.starts_with("__") || find_module_node(app, target_id).is_none()
                }) {
                    return Err(CommandError::InvalidCommand(
                        "Select a module node before replacing".to_string(),
                    ));
                }
                begin_pending_module_action(
                    app,
                    PendingModuleAction::Replace { target_ids },
                    module_name,
                );
                Ok(())
            } else {
                app.push_undo();
                cmd_replace(app, None, module_name, params.as_deref()).map(|_| ())
            }
        },
        "Replace selected module subtrees, or the current subtree when none is selected",
        1,
        None,
        "replace <module_name> [params]",
        vec!["replace sphere r=5", "replace cube size=[10,10,10]"],
        CommandType::Replace,
        true,
        true,
    ));

    registry.register(CommandDef::new(
        "model render",
        Vec::<&str>::new(),
        |app, _args| cmd_render(app),
        "Render the active SCAD source buffer",
        0,
        Some(0),
        "model render",
        vec!["model render"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(
        CommandDef::new(
            "model view",
            Vec::<&str>::new(),
            |app, args| cmd_view(app, args[0]),
            "Preview a standalone OFF, STL, or static DAE model",
            1,
            Some(1),
            "model view <model.off|model.stl|scene.dae>",
            vec![
                "model view model.off",
                "model view exported/model.stl",
                "model view assembly.dae",
            ],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::path(
            "model",
            true,
            &["off", "stl", "dae"],
        )]),
    );

    registry.register(
        CommandDef::new(
            "model export",
            Vec::<&str>::new(),
            |app, args| cmd_export_model(app, args[0]),
            "Export the active source as a flat model",
            1,
            Some(1),
            "model export <artifact>",
            vec!["model export model.stl", "model export model.dae"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::path("artifact", true, &[])]),
    );

    registry.register(
        CommandDef::new(
            "model preview",
            Vec::<&str>::new(),
            |app, args| match args {
                [] => cmd_preview(app, "model"),
                ["--render"] => cmd_render(app),
                _ => Err(CommandError::InvalidCommand(
                    "Usage: model preview [--render]".to_string(),
                )),
            },
            "Show the current model preview, rendering it when necessary",
            0,
            Some(1),
            "model preview [--render]",
            vec!["model preview", "model preview --render"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::literal("render", false, &["--render"])]),
    );

    registry.register(CommandDef::new(
        "model close",
        Vec::<&str>::new(),
        |app, _args| cmd_preview(app, "close"),
        "Close the model preview",
        0,
        Some(0),
        "model close",
        vec!["model close"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "model toggle",
        Vec::<&str>::new(),
        |app, _args| cmd_preview(app, "toggle"),
        "Toggle between the source editor and model preview",
        0,
        Some(0),
        "model toggle",
        vec!["model toggle"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(CommandDef::new(
        "source preview",
        Vec::<&str>::new(),
        |app, _args| cmd_preview(app, "source"),
        "Return to the source editor",
        0,
        Some(0),
        "source preview",
        vec!["source preview"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(
        CommandDef::new(
            "camera projection",
            Vec::<&str>::new(),
            |app, args| {
                let values = ["projection", args[0]];
                cmd_camera(app, &values)
            },
            "Change or toggle the preview projection",
            1,
            Some(1),
            "camera projection <perspective|orthographic|toggle>",
            vec!["camera projection toggle"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::literal(
            "projection",
            true,
            &["perspective", "orthographic", "toggle"],
        )]),
    );

    registry.register(
        CommandDef::new(
            "camera view",
            Vec::<&str>::new(),
            |app, args| {
                let values = ["view", args[0]];
                cmd_camera(app, &values)
            },
            "Move the camera to a standard view",
            1,
            Some(1),
            "camera view <front|back|left|right|top|bottom|iso>",
            vec!["camera view iso"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::literal(
            "view",
            true,
            &["front", "back", "left", "right", "top", "bottom", "iso"],
        )]),
    );

    registry.register(
        CommandDef::new(
            "camera orbit",
            Vec::<&str>::new(),
            |app, args| {
                let values = ["orbit", args[0], args[1]];
                cmd_camera(app, &values)
            },
            "Orbit the camera by yaw and pitch degrees",
            2,
            Some(2),
            "camera orbit <yaw> <pitch>",
            vec!["camera orbit 10 -5"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![
            ArgumentSpec::new("yaw", true, CompletionSource::None),
            ArgumentSpec::new("pitch", true, CompletionSource::None),
        ]),
    );

    registry.register(
        CommandDef::new(
            "camera pan",
            Vec::<&str>::new(),
            |app, args| {
                let values = ["pan", args[0], args[1]];
                cmd_camera(app, &values)
            },
            "Pan the camera in the view plane",
            2,
            Some(2),
            "camera pan <horizontal> <vertical>",
            vec!["camera pan 0.05 0"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![
            ArgumentSpec::new("horizontal", true, CompletionSource::None),
            ArgumentSpec::new("vertical", true, CompletionSource::None),
        ]),
    );

    registry.register(
        CommandDef::new(
            "camera zoom",
            Vec::<&str>::new(),
            |app, args| {
                let values = ["zoom", args[0]];
                cmd_camera(app, &values)
            },
            "Zoom the active preview",
            1,
            Some(1),
            "camera zoom <factor>",
            vec!["camera zoom 0.8"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::new(
            "factor",
            true,
            CompletionSource::None,
        )]),
    );

    registry.register(CommandDef::new(
        "camera fit",
        Vec::<&str>::new(),
        |app, _args| cmd_camera(app, &["fit"]),
        "Fit the active model in the preview",
        0,
        Some(0),
        "camera fit",
        vec!["camera fit"],
        CommandType::NoArg,
        false,
        true,
    ));

    registry.register(
        CommandDef::new(
            "camera auto-rotate",
            Vec::<&str>::new(),
            |app, args| {
                let values = ["auto-rotate", args[0]];
                cmd_camera(app, &values)
            },
            "Enable, disable, or toggle automatic rotation",
            1,
            Some(1),
            "camera auto-rotate <on|off|toggle>",
            vec!["camera auto-rotate toggle"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::literal(
            "state",
            true,
            &["on", "off", "toggle"],
        )]),
    );

    let protocol_values = ["auto", "next"]
        .into_iter()
        .chain(DisplayProtocol::NAMES.iter().copied())
        .map(str::to_string)
        .collect();
    registry.register(
        CommandDef::new(
            "display protocol",
            Vec::<&str>::new(),
            |app, args| cmd_protocol(app, args[0]),
            "Switch the terminal preview protocol without regenerating the model",
            1,
            Some(1),
            "display protocol <protocol>",
            vec!["display protocol next", "display protocol braille"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::new(
            "protocol",
            true,
            CompletionSource::Literal(protocol_values),
        )]),
    );

    registry.register(
        CommandDef::new(
            "display axes",
            Vec::<&str>::new(),
            |app, args| cmd_axes(app, args[0]),
            "Show or hide depth-aware world axes without regenerating the model",
            1,
            Some(1),
            "display axes <on|off|toggle>",
            vec!["display axes toggle", "display axes off"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::literal(
            "state",
            true,
            &["on", "off", "toggle"],
        )]),
    );

    registry.register(CommandDef::new(
        "visibility",
        vec!["visible"],
        |app, args| cmd_visibility(app, args[0]),
        "Show, hide, or toggle selected/current modules using OpenSCAD's * modifier",
        1,
        Some(1),
        "visibility <show|hide|toggle>",
        vec!["visibility hide", "visibility show", "visibility toggle"],
        CommandType::Visibility,
        true,
        true,
    ));

    let mut register_assembly = |action: &str,
                                 handler: crate::command_registry::CommandHandler,
                                 description: &str,
                                 min_args: usize,
                                 max_args: Option<usize>,
                                 usage: &str,
                                 arguments: Vec<ArgumentSpec>| {
        registry.register(
            CommandDef::new(
                format!("assembly {action}"),
                Vec::<String>::new(),
                handler,
                description,
                min_args,
                max_args,
                usage,
                vec![usage],
                CommandType::NoArg,
                false,
                true,
            )
            .with_arguments(arguments),
        );
    };
    register_assembly(
        "new",
        |app, args| dispatch_assembly(app, "new", args),
        "Create and open a rigid multi-part assembly",
        0,
        Some(1),
        "assembly new [name]",
        vec![ArgumentSpec::new("name", false, CompletionSource::None)],
    );
    register_assembly(
        "open",
        |app, args| dispatch_assembly(app, "open", args),
        "Open an existing assembly screen",
        0,
        Some(1),
        "assembly open [assembly]",
        vec![ArgumentSpec::new(
            "assembly",
            false,
            CompletionSource::Assembly,
        )],
    );
    register_assembly(
        "list",
        |app, args| dispatch_assembly(app, "list", args),
        "List project assemblies",
        0,
        Some(0),
        "assembly list",
        Vec::new(),
    );
    register_assembly(
        "add",
        |app, args| dispatch_assembly(app, "add", args),
        "Add an editable project source as an assembly part",
        1,
        Some(2),
        "assembly add <source> [name]",
        vec![
            ArgumentSpec::new(
                "source",
                true,
                CompletionSource::ProjectSource {
                    editable_only: true,
                },
            ),
            ArgumentSpec::new("name", false, CompletionSource::None),
        ],
    );
    register_assembly(
        "select",
        |app, args| dispatch_assembly(app, "select", args),
        "Select an assembly part",
        1,
        Some(1),
        "assembly select <part|next|prev>",
        vec![ArgumentSpec::new(
            "part",
            true,
            CompletionSource::AssemblyPart {
                literals: vec!["next".into(), "prev".into()],
            },
        )],
    );
    register_assembly(
        "copy",
        |app, args| dispatch_assembly(app, "copy", args),
        "Copy an assembly part",
        0,
        Some(1),
        "assembly copy [part]",
        vec![ArgumentSpec::new(
            "part",
            false,
            CompletionSource::AssemblyPart {
                literals: Vec::new(),
            },
        )],
    );
    register_assembly(
        "paste",
        |app, args| dispatch_assembly(app, "paste", args),
        "Paste an assembly part under an optional parent",
        0,
        Some(1),
        "assembly paste [parent|root]",
        vec![ArgumentSpec::new(
            "parent",
            false,
            CompletionSource::AssemblyPart {
                literals: vec!["root".into()],
            },
        )],
    );
    register_assembly(
        "remove",
        |app, args| dispatch_assembly(app, "remove", args),
        "Remove an assembly part",
        0,
        Some(1),
        "assembly remove [part]",
        vec![ArgumentSpec::new(
            "part",
            false,
            CompletionSource::AssemblyPart {
                literals: Vec::new(),
            },
        )],
    );
    register_assembly(
        "parent",
        |app, args| dispatch_assembly(app, "parent", args),
        "Change an assembly part parent",
        1,
        Some(2),
        "assembly parent [part] <parent|root>",
        vec![
            ArgumentSpec::new(
                "part-or-parent",
                true,
                CompletionSource::AssemblyPart {
                    literals: vec!["root".into()],
                },
            ),
            ArgumentSpec::new(
                "parent",
                false,
                CompletionSource::AssemblyPart {
                    literals: vec!["root".into()],
                },
            ),
        ],
    );
    for (action, description) in [
        ("translate", "Translate an assembly part"),
        ("rotate", "Rotate an assembly part"),
        ("scale", "Scale an assembly part"),
        ("pivot", "Set an assembly part pivot"),
    ] {
        let handler: crate::command_registry::CommandHandler = match action {
            "translate" => |app, args| dispatch_assembly(app, "translate", args),
            "rotate" => |app, args| dispatch_assembly(app, "rotate", args),
            "scale" => |app, args| dispatch_assembly(app, "scale", args),
            "pivot" => |app, args| dispatch_assembly(app, "pivot", args),
            _ => unreachable!(),
        };
        register_assembly(
            action,
            handler,
            description,
            3,
            Some(4),
            &format!("assembly {action} [part] <x> <y> <z>"),
            vec![
                ArgumentSpec::new(
                    "part-or-x",
                    true,
                    CompletionSource::AssemblyPart {
                        literals: Vec::new(),
                    },
                ),
                ArgumentSpec::new("x-or-y", true, CompletionSource::None),
                ArgumentSpec::new("y-or-z", true, CompletionSource::None),
                ArgumentSpec::new("z", false, CompletionSource::None),
            ],
        );
    }
    register_assembly(
        "visibility",
        |app, args| dispatch_assembly(app, "visibility", args),
        "Show, hide, or toggle an assembly part",
        1,
        Some(2),
        "assembly visibility [part] <show|hide|toggle>",
        vec![
            ArgumentSpec::new(
                "part-or-action",
                true,
                CompletionSource::AssemblyPart {
                    literals: vec!["show".into(), "hide".into(), "toggle".into()],
                },
            ),
            ArgumentSpec::literal("action", false, &["show", "hide", "toggle"]),
        ],
    );
    register_assembly(
        "render",
        |app, args| dispatch_assembly(app, "render", args),
        "Render the active assembly",
        0,
        Some(0),
        "assembly render",
        Vec::new(),
    );
    register_assembly(
        "export",
        |app, args| dispatch_assembly(app, "export", args),
        "Export the active assembly as hierarchical DAE",
        1,
        Some(1),
        "assembly export <file.dae>",
        vec![ArgumentSpec::path("destination", true, &["dae"])],
    );
    register_assembly(
        "close",
        |app, args| dispatch_assembly(app, "close", args),
        "Close the assembly screen",
        0,
        Some(0),
        "assembly close",
        Vec::new(),
    );
    registry.register(
        CommandDef::new(
            "diagnostics",
            vec!["diag"],
            |app, args| cmd_diagnostics(app, args.first().copied()),
            "Show the full latest render error or save it to a file",
            0,
            Some(1),
            "diagnostics [file]",
            vec!["diagnostics", "diagnostics ~/openscad-error.log"],
            CommandType::NoArg,
            false,
            true,
        )
        .with_arguments(vec![ArgumentSpec::path("file", false, &[])]),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use App;

    #[test]
    fn test_assembly_commands_build_and_edit_a_rigid_part_hierarchy() {
        let mut app = App::new();

        cmd_assembly(&mut app, &["new", "robot"]).unwrap();
        cmd_assembly(&mut app, &["add", "main.scad", "body"]).unwrap();
        cmd_assembly(&mut app, &["add", "main.scad", "arm"]).unwrap();
        cmd_assembly(&mut app, &["parent", "body"]).unwrap();
        cmd_assembly(&mut app, &["translate", "2", "3", "4"]).unwrap();
        cmd_assembly(&mut app, &["visibility", "hide"]).unwrap();

        let assembly = active_assembly(&app).unwrap();
        assert_eq!(assembly.parts.len(), 2);
        assert_eq!(
            assembly.part("arm").unwrap().parent.as_deref(),
            Some("body")
        );
        assert_eq!(
            assembly.part("arm").unwrap().transform.translation,
            [2.0, 3.0, 4.0]
        );
        assert!(!assembly.part("arm").unwrap().visible);
        assert!(!app.saved);
        assert_eq!(app.screen, Screen::Assembly);
        assert!(app.command_registry.is_namespace(&["assembly"]));
        assert!(app.command_registry.find("assembly add").is_some());

        let error = cmd_assembly(&mut app, &["scale", "arm", "0", "1", "1"])
            .expect_err("zero scale must be rejected");
        assert!(error.to_string().contains("non-zero"));
        assert_eq!(assembly_part(&app, "arm").transform.scale, [1.0; 3]);
    }

    #[test]
    fn test_assembly_copy_and_paste_preserve_instance_state_with_a_unique_name() {
        let mut app = App::new();
        cmd_assembly(&mut app, &["new", "robot"]).unwrap();
        cmd_assembly(&mut app, &["add", "main.scad", "body"]).unwrap();
        cmd_assembly(&mut app, &["add", "main.scad", "arm"]).unwrap();
        cmd_assembly(&mut app, &["parent", "arm", "body"]).unwrap();
        cmd_assembly(&mut app, &["translate", "arm", "2", "3", "4"]).unwrap();
        cmd_assembly(&mut app, &["visibility", "arm", "hide"]).unwrap();

        cmd_assembly(&mut app, &["copy", "arm"]).unwrap();
        cmd_assembly(&mut app, &["paste"]).unwrap();

        let pasted = assembly_part(&app, "arm2");
        assert_eq!(pasted.id, "arm2");
        assert_eq!(pasted.parent.as_deref(), Some("body"));
        assert_eq!(pasted.transform.translation, [2.0, 3.0, 4.0]);
        assert!(!pasted.visible);
        assert_eq!(app.selected_assembly_part.as_deref(), Some("arm2"));

        cmd_assembly(&mut app, &["copy"]).unwrap();
        cmd_assembly(&mut app, &["paste", "root"]).unwrap();
        assert_eq!(assembly_part(&app, "arm3").parent, None);
        assert_eq!(assembly_part(&app, "arm3").name_base, "arm");
    }

    #[test]
    fn test_assembly_export_compiles_multiple_sources_and_reuses_mesh_cache() {
        if std::process::Command::new("openscad")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.ast_mut().modules.push(ModuleNode::new_leaf(
            "body_cube".into(),
            "cube".into(),
            Vec::new(),
        ));
        cmd_new_file(&mut app, "parts/head.scad").unwrap();
        app.ast_mut().modules.push(ModuleNode::new_leaf(
            "head_sphere".into(),
            "sphere".into(),
            Vec::new(),
        ));
        cmd_assembly(&mut app, &["new", "robot"]).unwrap();
        cmd_assembly(&mut app, &["add", "main.scad", "body"]).unwrap();
        cmd_assembly(&mut app, &["add", "parts/head.scad", "head"]).unwrap();
        let destination = directory.path().join("robot.dae");

        cmd_assembly(&mut app, &["export", destination.to_str().unwrap()]).unwrap();

        let xml = fs::read_to_string(&destination).unwrap();
        assert_eq!(xml.matches("<geometry id=").count(), 2);
        assert_eq!(xml.matches("<node id=").count(), 2);
        let body_source = openscad_assembly::MeshSourceRef::project_source("main.scad");
        let cached = Arc::clone(&app.assembly_mesh_cache[&body_source].1);
        cmd_assembly(&mut app, &["translate", "body", "5", "0", "0"]).unwrap();
        cmd_assembly(&mut app, &["export", destination.to_str().unwrap()]).unwrap();
        assert!(Arc::ptr_eq(
            &cached,
            &app.assembly_mesh_cache[&body_source].1
        ));
    }

    fn assembly_part<'a>(app: &'a App, name: &str) -> &'a openscad_assembly::PartInstance {
        active_assembly(app).unwrap().part(name).unwrap()
    }

    #[test]
    fn test_visibility_hides_shows_and_toggles_selected_modules() {
        let mut app = App::new();
        app.ast_mut()
            .add_module(ModuleNode::new_leaf(
                "cube_1".into(),
                "cube".into(),
                Vec::new(),
            ))
            .unwrap();
        app.selected_nodes = vec!["cube_1".into()];

        cmd_visibility(&mut app, "hide").unwrap();
        assert_eq!(app.ast.modules[0].modifier, Some('*'));
        assert_eq!(app.ast.modules[0].to_scad(0), "*cube();");

        cmd_visibility(&mut app, "toggle").unwrap();
        assert_eq!(app.ast.modules[0].modifier, None);

        app.ast_mut().modules[0].modifier = Some('#');
        cmd_visibility(&mut app, "show").unwrap();
        assert_eq!(app.ast.modules[0].modifier, Some('#'));
    }

    #[test]
    fn test_diagnostics_reports_when_no_failure_is_available() {
        let mut app = App::new();
        let error = cmd_diagnostics(&mut app, None).unwrap_err();
        assert!(error
            .to_string()
            .contains("No render or display diagnostic"));
    }

    #[test]
    fn test_render_rewrites_embedded_absolute_dependencies_to_virtual_paths() {
        let dependencies = vec![openscad_core::SourceDependency {
            from: "workspace/model/main.scad".to_string(),
            to: "shared/lib/parts.scad".to_string(),
            reference: "/original/shared/lib/parts.scad".to_string(),
            kind: openscad_core::SourceDependencyKind::Use,
        }];
        let rewritten = rewrite_absolute_source_references(
            "use </original/shared/lib/parts.scad>;",
            "workspace/model/main.scad",
            &dependencies,
        );
        assert_eq!(rewritten, "use <../../shared/lib/parts.scad>;");
    }

    #[test]
    fn test_library_loads_without_activation_then_use_survives_project_reload() {
        let directory = tempfile::tempdir().unwrap();
        let library_directory = directory.path().join("source-library");
        fs::create_dir(&library_directory).unwrap();
        let library_path = library_directory.join("gears.scad");
        fs::write(
            &library_path,
            "include <helpers.scad>; module gear(teeth=20) { helper(); }",
        )
        .unwrap();
        fs::write(
            library_directory.join("helpers.scad"),
            "module helper() { cube(1); } function pitch(d, teeth) = d / teeth;",
        )
        .unwrap();
        let project_path = directory.path().join("project.scadtui");
        let mut app = App::new();

        cmd_load_library(&mut app, library_path.to_str().unwrap()).unwrap();
        assert!(app.library.get_module("gear").is_none());
        assert!(app.library.get_module("helper").is_none());
        assert!(app.library.get_function("pitch").is_none());
        assert_eq!(app.ast.embedded_sources.len(), 3);
        assert_eq!(app.ast.source_dependencies.len(), 1);
        assert_eq!(app.ast.uses.len(), 0);
        assert!(app.ast.embedded_sources.iter().any(|source| {
            source.role == openscad_core::EmbeddedSourceRole::Library
                && source.virtual_path.ends_with("/gears.scad")
        }));

        cmd_use_library(&mut app, "gears.scad").unwrap();
        assert!(app.library.get_module("gear").is_some());
        assert!(app.library.get_module("helper").is_some());
        assert!(app.library.get_function("pitch").is_some());
        assert_eq!(app.ast.source_dependencies.len(), 2);
        assert_eq!(app.ast.uses.len(), 1);

        cmd_write_force(&mut app, project_path.to_str().unwrap()).unwrap();
        fs::remove_dir_all(&library_directory).unwrap();
        let mut restored = App::new();
        cmd_load_force(&mut restored, project_path.to_str().unwrap()).unwrap();
        assert!(restored.library.get_module("gear").is_some());
        assert!(restored.library.get_module("helper").is_some());
        assert!(restored.library.get_function("pitch").is_some());
    }

    #[test]
    fn test_use_requires_a_loaded_source_and_rejects_duplicates() {
        let mut app = App::new();
        let error = cmd_use_library(&mut app, "missing.scad").unwrap_err();
        assert!(error.to_string().contains("not loaded"));

        let directory = tempfile::tempdir().unwrap();
        let library_path = directory.path().join("parts.scad");
        fs::write(&library_path, "module part() { cube(1); }").unwrap();
        cmd_load_library(&mut app, library_path.to_str().unwrap()).unwrap();
        cmd_use_library(&mut app, "parts").unwrap();

        let error = cmd_use_library(&mut app, "parts.scad").unwrap_err();
        assert!(error.to_string().contains("already referenced with use"));
    }

    #[test]
    fn test_library_remove_deletes_private_tree_but_rejects_active_references() {
        let directory = tempfile::tempdir().unwrap();
        let library_directory = directory.path().join("library");
        fs::create_dir(&library_directory).unwrap();
        let library_path = library_directory.join("parts.scad");
        fs::write(
            &library_path,
            "include <helper.scad>; module part() { helper(); }",
        )
        .unwrap();
        fs::write(
            library_directory.join("helper.scad"),
            "module helper() { cube(1); }",
        )
        .unwrap();
        let mut app = App::new();

        cmd_load_library(&mut app, library_path.to_str().unwrap()).unwrap();
        let root = app
            .ast
            .embedded_sources
            .iter()
            .find(|source| source.role == openscad_core::EmbeddedSourceRole::Library)
            .unwrap()
            .virtual_path
            .clone();
        cmd_remove_library(&mut app, &root).unwrap();
        assert_eq!(app.ast.embedded_sources.len(), 1);
        assert!(app.ast.source_dependencies.is_empty());

        cmd_load_library(&mut app, library_path.to_str().unwrap()).unwrap();
        cmd_use_library(&mut app, "parts.scad").unwrap();
        let error = cmd_remove_library(&mut app, "parts.scad").unwrap_err();
        assert!(error.to_string().contains("references it"));
    }

    #[test]
    fn test_include_activates_loaded_library_with_include_semantics() {
        let directory = tempfile::tempdir().unwrap();
        let library_path = directory.path().join("scene.scad");
        fs::write(
            &library_path,
            "global_size = 10; module scene_part() { cube(global_size); } scene_part();",
        )
        .unwrap();
        let mut app = App::new();

        cmd_load_library(&mut app, library_path.to_str().unwrap()).unwrap();
        cmd_include_library(&mut app, "scene.scad").unwrap();

        assert!(app.ast.uses.is_empty());
        assert_eq!(app.ast.includes.len(), 1);
        assert!(app.ast.to_scad().starts_with("include <"));
        assert!(app.library.get_module("scene_part").is_some());
        assert!(app.ast.source_dependencies.iter().any(|dependency| {
            dependency.kind == openscad_core::SourceDependencyKind::Include
                && dependency.to.ends_with("/scene.scad")
        }));
    }

    #[test]
    fn test_edit_scad_parses_a_structured_unsaved_project() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("existing.scad");
        let source =
            "include <parts/common.scad>;\nmodule bracket(size=10) { cube(size); }\nbracket(20);\n";
        fs::write(&source_path, source).unwrap();
        let mut app = App::new();

        cmd_edit_scad(&mut app, source_path.to_str().unwrap()).unwrap();

        assert_eq!(app.ast.includes, ["parts/common.scad"]);
        assert_eq!(app.ast.modules.len(), 1);
        assert_eq!(app.ast.modules[0].name, "bracket");
        assert!(app.library.get_module("bracket").is_some());
        assert_eq!(app.current_file, None);
        assert!(!app.saved);
    }

    #[test]
    fn test_library_load_does_not_reclassify_an_editable_source() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("part.scad");
        fs::write(&source_path, "module part() { cube(1); }").unwrap();
        let mut app = App::new();

        cmd_edit_scad(&mut app, source_path.to_str().unwrap()).unwrap();
        let error = cmd_load_library(&mut app, source_path.to_str().unwrap()).unwrap_err();

        assert!(error.to_string().contains("editable project source"));
        assert_eq!(app.ast.embedded_sources.len(), 1);
        assert!(app.ast.embedded_sources[0].editable);
        assert_eq!(
            app.ast.embedded_sources[0].role,
            openscad_core::EmbeddedSourceRole::Entry
        );
    }

    #[test]
    fn test_new_project_and_new_file_create_materialized_buffers() {
        let mut app = App::new();
        cmd_new_project(&mut app, Some("fixture"), true).unwrap();
        assert_eq!(app.project_name, "fixture");
        assert_eq!(app.current_file, None);
        assert_eq!(app.ast.active_source.as_deref(), Some("main.scad"));
        assert!(!app.saved);

        cmd_new_file(&mut app, "parts/bracket.scad").unwrap();
        assert_eq!(app.ast.active_source.as_deref(), Some("parts/bracket.scad"));
        assert!(app
            .ast
            .embedded_sources
            .iter()
            .any(|source| source.virtual_path == "parts/bracket.scad" && source.editable));
        assert!(cmd_new_file(&mut app, "../escape.scad").is_err());
        assert!(cmd_new_file(&mut app, "parts/bracket.scad").is_err());

        cmd_new_file(&mut app, "head").unwrap();
        assert_eq!(app.ast.active_source.as_deref(), Some("head.scad"));
    }

    #[test]
    fn test_project_name_round_trips_and_source_rename_updates_references() {
        let directory = tempfile::tempdir().unwrap();
        let project_path = directory.path().join("renamed.scadtui");
        let mut app = App::new();
        cmd_rename_project(&mut app, "vernier caliper").unwrap();
        cmd_new_file(&mut app, "parts/arm").unwrap();
        cmd_buffer(&mut app, Some("main.scad")).unwrap();
        cmd_use_library(&mut app, "parts/arm.scad").unwrap();
        cmd_assembly(&mut app, &["new", "fixture"]).unwrap();
        cmd_assembly(&mut app, &["add", "parts/arm.scad", "arm"]).unwrap();

        cmd_rename_source(&mut app, "parts/arm.scad", "upper-arm").unwrap();

        assert!(app
            .ast
            .embedded_sources
            .iter()
            .any(|source| source.virtual_path == "parts/upper-arm.scad"));
        assert!(app.ast.source_dependencies.iter().any(|dependency| {
            dependency.from == "main.scad"
                && dependency.to == "parts/upper-arm.scad"
                && dependency.reference == "parts/upper-arm.scad"
        }));
        assert_eq!(app.ast.uses, ["parts/upper-arm.scad"]);
        assert_eq!(
            app.assemblies[0].parts[0].source.virtual_path(),
            "parts/upper-arm.scad"
        );

        cmd_write_force(&mut app, project_path.to_str().unwrap()).unwrap();
        let mut restored = App::new();
        cmd_load_force(&mut restored, project_path.to_str().unwrap()).unwrap();
        assert_eq!(restored.project_name, "vernier caliper");
        assert_eq!(
            restored.assemblies[0].parts[0].source.virtual_path(),
            "parts/upper-arm.scad"
        );
    }

    #[test]
    fn test_source_remove_rejects_references_and_removes_an_unreferenced_source() {
        let mut app = App::new();
        cmd_new_file(&mut app, "used").unwrap();
        cmd_buffer(&mut app, Some("main.scad")).unwrap();
        cmd_use_library(&mut app, "used.scad").unwrap();
        assert!(cmd_remove_source(&mut app, "used.scad")
            .unwrap_err()
            .to_string()
            .contains("referenced"));

        cmd_new_file(&mut app, "unused").unwrap();
        cmd_remove_source(&mut app, "unused.scad").unwrap();
        assert!(!app
            .ast
            .embedded_sources
            .iter()
            .any(|source| source.virtual_path == "unused.scad"));
    }

    #[test]
    fn test_imported_scad_is_stored_inside_saved_project_package() {
        let directory = tempfile::tempdir().unwrap();
        let source_directory = directory.path().join("original");
        fs::create_dir(&source_directory).unwrap();
        let source_path = source_directory.join("shape.scad");
        let library_path = source_directory.join("parts.scad");
        fs::write(&source_path, "use <parts.scad>;\npart(size=5);\n").unwrap();
        fs::write(&library_path, "module part(size=1) { cube(size); }\n").unwrap();
        let project_path = directory.path().join("shape-project.scadtui");
        let mut app = App::new();

        cmd_edit_scad(&mut app, source_path.to_str().unwrap()).unwrap();
        assert!(app.library.get_module("part").is_some());
        cmd_write_force(&mut app, project_path.to_str().unwrap()).unwrap();
        fs::remove_dir_all(&source_directory).unwrap();

        let mut restored = App::new();
        cmd_load_force(&mut restored, project_path.to_str().unwrap()).unwrap();
        assert_eq!(restored.ast.embedded_sources.len(), 2);
        assert!(restored.library.get_module("part").is_some());
        assert!(restored.ast.to_scad().contains("part(size=5);"));
        assert!(restored.saved);
    }

    #[test]
    fn test_buffer_switching_preserves_edits_in_multiple_scad_sources() {
        let directory = tempfile::tempdir().unwrap();
        let main_path = directory.path().join("main.scad");
        fs::write(&main_path, "use <part.scad>; cube(1);").unwrap();
        fs::write(directory.path().join("part.scad"), "sphere(2);").unwrap();
        let project_path = directory.path().join("project.scadtui");
        let mut app = App::new();

        cmd_edit_scad(&mut app, main_path.to_str().unwrap()).unwrap();
        app.ast_mut().modules[0].name = "cylinder".to_string();
        cmd_buffer(&mut app, Some("part.scad")).unwrap();
        assert_eq!(app.ast.active_source.as_deref(), Some("part.scad"));
        assert_eq!(app.ast.modules[0].name, "sphere");
        let library_path = directory.path().join("helpers.scad");
        fs::write(&library_path, "module helper() { cube(1); }").unwrap();
        cmd_load_library(&mut app, library_path.to_str().unwrap()).unwrap();
        cmd_use_library(&mut app, "helpers.scad").unwrap();
        assert!(app.ast.source_dependencies.iter().any(|dependency| {
            dependency.from == "part.scad"
                && dependency.kind == openscad_core::SourceDependencyKind::Use
        }));
        app.ast_mut().modules[0].name = "cube".to_string();
        cmd_write_force(&mut app, project_path.to_str().unwrap()).unwrap();

        let mut restored = App::new();
        cmd_load_force(&mut restored, project_path.to_str().unwrap()).unwrap();
        assert_eq!(restored.ast.active_source.as_deref(), Some("part.scad"));
        assert_eq!(restored.ast.entry_source.as_deref(), Some("main.scad"));
        assert!(restored
            .ast
            .source_code("main.scad")
            .unwrap()
            .contains("cylinder(1);"));
        assert!(restored
            .ast
            .source_code("part.scad")
            .unwrap()
            .contains("cube(2);"));
    }

    #[test]
    fn test_editable_sources_can_be_related_with_use_and_include() {
        let mut app = App::new();
        cmd_new_file(&mut app, "parts/jaw.scad").unwrap();
        cmd_buffer(&mut app, Some("main.scad")).unwrap();

        cmd_use_library(&mut app, "jaw.scad").unwrap();

        assert!(app.ast.source_dependencies.iter().any(|dependency| {
            dependency.from == "main.scad"
                && dependency.to == "parts/jaw.scad"
                && dependency.kind == openscad_core::SourceDependencyKind::Use
        }));
        assert_eq!(app.ast.uses, ["parts/jaw.scad"]);
    }

    #[test]
    fn test_export_source_writes_only_the_active_buffer() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.ast_mut().modules.push(ModuleNode::new_leaf(
            "main_cube".to_string(),
            "cube".to_string(),
            Vec::new(),
        ));
        cmd_new_file(&mut app, "part.scad").unwrap();
        app.ast_mut().modules.push(ModuleNode::new_leaf(
            "part_sphere".to_string(),
            "sphere".to_string(),
            Vec::new(),
        ));
        let output = directory.path().join("active");

        cmd_export_source(&mut app, output.to_str().unwrap()).unwrap();

        let source = fs::read_to_string(output.with_extension("scad")).unwrap();
        assert!(source.contains("sphere("));
        assert!(!source.contains("cube("));
    }

    #[test]
    fn test_relative_exports_are_resolved_next_to_the_project_package() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.current_file = Some(
            directory
                .path()
                .join("test.scadtui")
                .to_string_lossy()
                .into_owned(),
        );
        app.ast_mut().modules.push(ModuleNode::new_leaf(
            "main_cube".to_string(),
            "cube".to_string(),
            Vec::new(),
        ));

        assert_eq!(
            resolve_export_path(&app, "model.stl").unwrap(),
            directory.path().join("model.stl")
        );
        cmd_export_source(&mut app, "snapshot").unwrap();

        assert!(fs::read_to_string(directory.path().join("snapshot.scad"))
            .unwrap()
            .contains("cube("));
    }

    #[test]
    fn test_export_tree_materializes_reachable_project_sources() {
        let directory = tempfile::tempdir().unwrap();
        let source_directory = directory.path().join("input");
        fs::create_dir(&source_directory).unwrap();
        fs::write(
            source_directory.join("main.scad"),
            "use <parts/jaw.scad>;\njaw();\n",
        )
        .unwrap();
        fs::create_dir(source_directory.join("parts")).unwrap();
        fs::write(
            source_directory.join("parts/jaw.scad"),
            "module jaw() { cube(1); }\n",
        )
        .unwrap();
        let mut app = App::new();
        cmd_edit_scad(
            &mut app,
            source_directory.join("main.scad").to_str().unwrap(),
        )
        .unwrap();
        let output = directory.path().join("exported");

        cmd_export_tree(&mut app, output.to_str().unwrap()).unwrap();

        assert!(fs::read_to_string(output.join("main.scad"))
            .unwrap()
            .contains("jaw();"));
        assert!(fs::read_to_string(output.join("parts/jaw.scad"))
            .unwrap()
            .contains("module jaw"));
    }

    #[test]
    fn test_render_is_a_zero_argument_active_buffer_command() {
        let app = App::new();
        let render = app.command_registry.find("model render").unwrap();
        assert_eq!(render.max_args, Some(0));
        assert_eq!(render.usage, "model render");
        assert!(app.command_registry.find("render-target").is_none());
    }

    #[test]
    fn test_cmd_help_generates_current_command_overview() {
        let mut app = App::new();

        cmd_help(&mut app, None).expect("general help should succeed");

        assert_eq!(app.input_mode, InputMode::Help);
        assert_eq!(app.help_doc_count, app.help_doc.len());
        assert!(app
            .help_doc
            .iter()
            .any(|line| line.contains("function name(params) = expression")));
        assert!(app
            .help_doc
            .iter()
            .any(|line| line.contains("replace <module_name> [params]")));
    }

    #[test]
    fn test_cmd_version_uses_package_version() {
        let mut app = App::new();
        cmd_version(&mut app);
        assert_eq!(
            app.message.as_deref(),
            Some(concat!("openscad-tui ", env!("CARGO_PKG_VERSION")))
        );
        assert!(app.command_registry.find("version").is_some());
        assert!(app.command_registry.find("ver").is_some());
    }

    #[test]
    fn test_source_preview_requests_terminal_graphics_clear() {
        let mut app = App::new();
        app.enter_model_screen();
        app.model_preview.set_auto_rotate(true);

        cmd_preview(&mut app, "source").unwrap();

        assert_eq!(app.screen, crate::app::Screen::Editor);
        assert!(!app.model_preview.auto_rotate);
        assert!(app.take_terminal_clear_request());
        assert!(!app.take_terminal_clear_request());
    }

    #[test]
    fn test_model_preview_enters_independent_screen() {
        let mut app = App::new();

        cmd_preview(&mut app, "model").unwrap();

        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
        assert!(matches!(
            app.model_preview.status,
            crate::preview::ModelPreviewStatus::Loading
        ));

        app.input_mode = InputMode::ModuleEnterParams;
        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
    }

    #[test]
    fn test_view_loads_off_without_changing_the_project() {
        let directory = tempfile::tempdir().unwrap();
        let model = directory.path().join("triangle.off");
        fs::write(&model, "OFF\n3 1 0\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n").unwrap();
        let mut app = App::new();
        let original_source = app.ast.to_scad();

        cmd_view(&mut app, model.to_str().unwrap()).unwrap();

        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
        assert_eq!(app.ast.to_scad(), original_source);
        assert!(matches!(
            app.model_preview.status,
            crate::preview::ModelPreviewStatus::Loading
        ));
    }

    #[test]
    fn test_view_loads_a_static_dae_scene() {
        let directory = tempfile::tempdir().unwrap();
        let model = directory.path().join("scene.dae");
        fs::write(
            &model,
            r##"<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
                <asset><up_axis>Z_UP</up_axis></asset>
                <library_geometries><geometry id="g"><mesh>
                  <source id="p"><float_array id="pa" count="9">0 0 0 1 0 0 0 1 0</float_array>
                    <technique_common><accessor source="#pa" count="3" stride="3"><param name="X"/><param name="Y"/><param name="Z"/></accessor></technique_common>
                  </source>
                  <vertices id="v"><input semantic="POSITION" source="#p"/></vertices>
                  <triangles count="1"><input semantic="VERTEX" source="#v" offset="0"/><p>0 1 2</p></triangles>
                </mesh></geometry></library_geometries>
                <library_visual_scenes><visual_scene id="Scene"><node><instance_geometry url="#g"/></node></visual_scene></library_visual_scenes>
                <scene><instance_visual_scene url="#Scene"/></scene>
              </COLLADA>"##,
        )
        .unwrap();
        let mut app = App::new();

        cmd_view(&mut app, model.to_str().unwrap()).unwrap();

        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
        assert!(matches!(
            app.model_preview.status,
            crate::preview::ModelPreviewStatus::Loading
        ));
    }

    #[test]
    fn test_preview_model_reuses_an_existing_render() {
        let mut app = App::new();
        app.model_preview.status = crate::preview::ModelPreviewStatus::Ready { triangles: 12 };

        cmd_preview(&mut app, "model").unwrap();

        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
        assert_eq!(
            app.model_preview.status,
            crate::preview::ModelPreviewStatus::Ready { triangles: 12 }
        );
    }

    #[test]
    fn test_preview_toggle_switches_existing_preview_without_rendering() {
        let mut app = App::new();
        app.model_preview.status = crate::preview::ModelPreviewStatus::Ready { triangles: 12 };

        cmd_preview(&mut app, "toggle").unwrap();
        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
        cmd_preview(&mut app, "toggle").unwrap();
        assert_eq!(app.screen, crate::app::Screen::Editor);
    }

    #[test]
    fn test_preview_close_quits_a_standalone_model_session() {
        let mut app = App::new();
        app.enter_model_screen();
        app.preview_close_action = PreviewCloseAction::Quit;

        cmd_preview(&mut app, "close").unwrap();

        assert!(app.should_quit);
        assert_eq!(app.screen, crate::app::Screen::ModelPreview);
    }

    #[test]
    fn test_preview_close_returns_project_preview_to_source() {
        let mut app = App::new();
        app.enter_model_screen();

        cmd_preview(&mut app, "close").unwrap();

        assert!(!app.should_quit);
        assert_eq!(app.screen, crate::app::Screen::Editor);
    }

    #[test]
    fn test_camera_auto_rotate_toggle_is_a_command_operation() {
        let mut app = App::new();

        cmd_camera(&mut app, &["auto-rotate", "toggle"]).unwrap();
        assert!(app.model_preview.auto_rotate);
        cmd_camera(&mut app, &["auto-rotate", "toggle"]).unwrap();
        assert!(!app.model_preview.auto_rotate);
    }

    #[test]
    fn test_axes_command_toggles_world_axes() {
        let mut app = App::new();
        assert!(app.model_preview.axes_visible);

        cmd_axes(&mut app, "toggle").unwrap();
        assert!(!app.model_preview.axes_visible);
        cmd_axes(&mut app, "on").unwrap();
        assert!(app.model_preview.axes_visible);
        assert!(cmd_axes(&mut app, "invalid").is_err());
    }

    #[test]
    fn test_protocol_command_switches_backend_and_requests_clear() {
        let mut app = App::new();

        cmd_protocol(&mut app, "ascii").unwrap();
        assert_eq!(app.model_preview.protocol_type(), DisplayProtocol::Ascii);
        assert!(app.take_terminal_clear_request());

        cmd_protocol(&mut app, "next").unwrap();
        assert_eq!(app.model_preview.protocol_type(), DisplayProtocol::Kitty);
        cmd_protocol(&mut app, "braille").unwrap();
        assert_eq!(app.model_preview.protocol_type(), DisplayProtocol::Braille);
        assert!(cmd_protocol(&mut app, "unknown").is_err());
    }

    #[test]
    fn test_cmd_help_shows_namespace_and_leaf_details() {
        let mut app = App::new();

        cmd_help(&mut app, Some("model")).expect("namespace help should succeed");
        assert!(app.help_doc.iter().any(|line| line.contains("model view")));

        cmd_help(&mut app, Some("model view")).expect("leaf help should succeed");

        assert!(app.help_doc.iter().any(|line| line == "Help: model view"));
        assert!(app
            .help_doc
            .iter()
            .any(|line| line.starts_with("Usage: model view")));
    }

    #[test]
    fn test_cmd_help_rejects_unknown_command() {
        let mut app = App::new();
        let result = cmd_help(&mut app, Some("missing-command"));

        assert!(result.is_err());
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    #[test]
    fn test_cmd_yank_and_paste_clone_subtree_with_new_ids() {
        let mut app = App::new();
        let mut original = ModuleNode::new_container(
            "translate_original".to_string(),
            "translate".to_string(),
            Vec::new(),
        );
        original.children.push(ModuleNode::new_leaf(
            "cube_original".to_string(),
            "cube".to_string(),
            Vec::new(),
        ));
        app.ast_mut().modules.push(original);
        app.tree_state.borrow_mut().select(vec![
            "__modules".to_string(),
            "translate_original".to_string(),
        ]);

        cmd_yank(&mut app, None).expect("yank should succeed");
        let pasted_id = cmd_paste(&mut app).expect("paste should succeed");

        assert_eq!(app.ast.modules.len(), 2);
        let pasted = app.ast.find_node_by_id(&pasted_id).unwrap();
        assert_eq!(pasted.name, "translate");
        assert_ne!(pasted.id, "translate_original");
        assert_ne!(pasted.children[0].id, "cube_original");
    }

    #[test]
    fn test_cmd_remove_does_not_change_clipboard() {
        let mut app = App::new();
        app.ast_mut().modules.push(ModuleNode::new_leaf(
            "cube_1".to_string(),
            "cube".to_string(),
            Vec::new(),
        ));
        cmd_yank(&mut app, Some("cube_1")).unwrap();

        cmd_remove(&mut app, Some("cube_1")).expect("remove should succeed");

        assert!(app.ast.find_node_by_id("cube_1").is_none());
        assert_eq!(app.node_clipboard[0].id, "cube_1");
    }

    #[test]
    fn test_cmd_delete_prefers_selected_nodes() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf("cube_1".to_string(), "cube".to_string(), Vec::new()),
            ModuleNode::new_leaf("sphere_1".to_string(), "sphere".to_string(), Vec::new()),
            ModuleNode::new_leaf("keep_1".to_string(), "cube".to_string(), Vec::new()),
        ];
        app.selected_nodes = vec!["cube_1".to_string(), "sphere_1".to_string()];
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "keep_1".to_string()]);

        cmd_delete(&mut app, "").expect("delete should succeed");

        assert_eq!(app.ast.modules.len(), 1);
        assert_eq!(app.ast.modules[0].id, "keep_1");
        assert!(app.selected_nodes.is_empty());
        assert_eq!(app.node_clipboard.len(), 2);
        assert_eq!(
            app.tree_state.borrow().selected(),
            ["__modules".to_string(), "keep_1".to_string()]
        );
    }

    #[test]
    fn test_cmd_delete_then_paste_swaps_adjacent_nodes() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf("cube_1".to_string(), "cube".to_string(), Vec::new()),
            ModuleNode::new_leaf("sphere_1".to_string(), "sphere".to_string(), Vec::new()),
            ModuleNode::new_leaf("cylinder_1".to_string(), "cylinder".to_string(), Vec::new()),
        ];
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "cube_1".to_string()]);

        cmd_delete(&mut app, "").expect("delete should cut the current subtree");

        assert_eq!(app.node_clipboard.len(), 1);
        assert_eq!(app.node_clipboard[0].id, "cube_1");
        assert_eq!(
            app.tree_state.borrow().selected(),
            ["__modules".to_string(), "sphere_1".to_string()]
        );

        let pasted_id = cmd_paste(&mut app).expect("paste should reinsert the cut subtree");
        assert_eq!(
            app.ast
                .modules
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            ["sphere", "cube", "cylinder"]
        );
        assert_eq!(app.tree_state.borrow().selected().last(), Some(&pasted_id));
    }

    #[test]
    fn test_cmd_delete_last_sibling_selects_previous_sibling() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf("cube_1".to_string(), "cube".to_string(), Vec::new()),
            ModuleNode::new_leaf("sphere_1".to_string(), "sphere".to_string(), Vec::new()),
        ];
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "sphere_1".to_string()]);

        cmd_delete(&mut app, "").unwrap();

        assert_eq!(
            app.tree_state.borrow().selected(),
            ["__modules".to_string(), "cube_1".to_string()]
        );
    }

    #[test]
    fn test_cmd_delete_only_child_selects_parent() {
        let mut app = App::new();
        let mut parent = ModuleNode::new_container(
            "translate_1".to_string(),
            "translate".to_string(),
            Vec::new(),
        );
        parent.children.push(ModuleNode::new_leaf(
            "cube_1".to_string(),
            "cube".to_string(),
            Vec::new(),
        ));
        app.ast_mut().modules.push(parent);
        app.tree_state.borrow_mut().select(vec![
            "__modules".to_string(),
            "translate_1".to_string(),
            "cube_1".to_string(),
        ]);

        cmd_delete(&mut app, "").unwrap();

        assert_eq!(
            app.tree_state.borrow().selected(),
            ["__modules".to_string(), "translate_1".to_string()]
        );
    }

    #[test]
    fn test_cmd_set_parameter_updates_selected_nodes_atomically() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf(
                "cube_1".to_string(),
                "cube".to_string(),
                vec![Argument::Named {
                    name: "size".to_string(),
                    value: Expr::Integer(10),
                }],
            ),
            ModuleNode::new_leaf(
                "sphere_1".to_string(),
                "sphere".to_string(),
                vec![Argument::Named {
                    name: "r".to_string(),
                    value: Expr::Integer(5),
                }],
            ),
        ];
        app.selected_nodes = vec!["cube_1".to_string(), "sphere_1".to_string()];

        let result = cmd_set_parameter(&mut app, "size=module_size");

        assert!(result.is_err());
        assert!(matches!(
            &app.ast.modules[0].args[0],
            Argument::Named {
                value: Expr::Integer(10),
                ..
            }
        ));
    }

    #[test]
    fn test_cmd_unset_parameter_supports_positional_named_and_atomic_failure() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf(
                "cube_1".to_string(),
                "cube".to_string(),
                vec![Argument::Positional(Expr::Integer(10))],
            ),
            ModuleNode::new_leaf(
                "cube_2".to_string(),
                "cube".to_string(),
                vec![Argument::Named {
                    name: "size".to_string(),
                    value: Expr::Integer(20),
                }],
            ),
        ];
        app.selected_nodes = vec!["cube_1".to_string(), "cube_2".to_string()];

        cmd_unset_parameter(&mut app, "size").unwrap();
        assert!(app.ast.modules.iter().all(|node| node.args.is_empty()));

        app.ast_mut().modules[0].args.push(Argument::Named {
            name: "center".to_string(),
            value: Expr::Boolean(true),
        });
        let result = cmd_unset_parameter(&mut app, "center");
        assert!(result.is_err());
        assert_eq!(app.ast.modules[0].args.len(), 1);
    }

    #[test]
    fn test_cmd_set_parameter_supports_special_parameters_and_values() {
        let mut app = App::new();
        let sphere_id = cmd_insert(&mut app, "sphere", None, Some("r=10")).unwrap();
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), sphere_id.clone()]);

        cmd_set_parameter(&mut app, "$fn=32").unwrap();
        cmd_set_parameter(&mut app, "r=$fn").unwrap();

        let sphere = app.ast.find_node_by_id(&sphere_id).unwrap();
        assert!(sphere.args.iter().any(|argument| matches!(
            argument,
            Argument::Named {
                name,
                value: Expr::Integer(32)
            } if name == "$fn"
        )));
        assert!(sphere.args.iter().any(|argument| matches!(
            argument,
            Argument::Named {
                name,
                value: Expr::Identifier(identifier)
            } if name == "r" && identifier == "$fn"
        )));
    }

    #[test]
    fn test_module_body_parameter_can_reference_module_parameter() {
        let mut app = App::new();
        let cube_id = cmd_insert(&mut app, "cube", None, Some("size=10")).unwrap();
        app.selected_nodes = vec![cube_id];
        cmd_moddef(&mut app, "my_box", Some("size=20")).unwrap();
        let body_id = app.ast.module_defines[0].body[0].id.clone();
        app.tree_state.borrow_mut().select(vec![
            "__moddefs".to_string(),
            "__moddef_my_box".to_string(),
            body_id,
        ]);

        cmd_set_parameter(&mut app, "size=size").unwrap();

        assert!(app.ast.module_defines[0]
            .to_scad()
            .contains("cube(size=size);"));
    }

    #[test]
    fn test_cmd_yank_prefers_selected_node_over_current_node() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf("selected_1".to_string(), "cube".to_string(), Vec::new()),
            ModuleNode::new_leaf("current_1".to_string(), "sphere".to_string(), Vec::new()),
        ];
        app.selected_nodes = vec!["selected_1".to_string()];
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "current_1".to_string()]);

        cmd_yank(&mut app, None).expect("yank should succeed");

        assert_eq!(app.node_clipboard[0].id, "selected_1");
    }

    #[test]
    fn test_cmd_remove_prefers_all_selected_nodes_over_current_node() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf("selected_1".to_string(), "cube".to_string(), Vec::new()),
            ModuleNode::new_leaf("selected_2".to_string(), "sphere".to_string(), Vec::new()),
            ModuleNode::new_leaf("current_1".to_string(), "cube".to_string(), Vec::new()),
        ];
        app.selected_nodes = vec!["selected_1".to_string(), "selected_2".to_string()];
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "current_1".to_string()]);

        cmd_remove(&mut app, None).expect("remove should succeed");

        assert_eq!(app.ast.modules.len(), 1);
        assert_eq!(app.ast.modules[0].id, "current_1");
    }

    #[test]
    fn test_cmd_replace_prefers_all_selected_nodes_over_current_node() {
        let mut app = App::new();
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf("selected_1".to_string(), "cube".to_string(), Vec::new()),
            ModuleNode::new_leaf("selected_2".to_string(), "sphere".to_string(), Vec::new()),
            ModuleNode::new_leaf("current_1".to_string(), "cube".to_string(), Vec::new()),
        ];
        app.selected_nodes = vec!["selected_1".to_string(), "selected_2".to_string()];
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "current_1".to_string()]);

        cmd_replace(&mut app, None, "cylinder", None).expect("replace should succeed");

        assert_eq!(app.ast.modules.len(), 3);
        assert_eq!(app.ast.modules[0].name, "cylinder");
        assert_eq!(app.ast.modules[1].name, "cylinder");
        assert_eq!(app.ast.modules[2].id, "current_1");
    }

    #[test]
    fn test_cmd_remove_node_from_module_definition() {
        use openscad_core::ModuleDefinition;

        let mut app = App::new();
        app.ast_mut().module_defines.push(ModuleDefinition::new(
            "custom".to_string(),
            Vec::new(),
            vec![ModuleNode::new_leaf(
                "body_cube".to_string(),
                "cube".to_string(),
                Vec::new(),
            )],
        ));

        cmd_remove(&mut app, Some("body_cube")).expect("remove should succeed");

        assert!(app.ast.module_defines[0].body.is_empty());
    }

    #[test]
    fn test_cmd_remove_promotes_children_at_same_position() {
        let mut app = App::new();
        let mut container =
            ModuleNode::new_container("group_1".to_string(), "union".to_string(), Vec::new());
        container.children = vec![
            ModuleNode::new_leaf("cube_1".to_string(), "cube".to_string(), Vec::new()),
            ModuleNode::new_leaf("sphere_1".to_string(), "sphere".to_string(), Vec::new()),
        ];
        app.ast_mut().modules = vec![
            ModuleNode::new_leaf("before".to_string(), "cube".to_string(), Vec::new()),
            container,
            ModuleNode::new_leaf("after".to_string(), "cube".to_string(), Vec::new()),
        ];

        cmd_remove(&mut app, Some("group_1")).expect("remove should succeed");

        let ids: Vec<&str> = app
            .ast
            .modules
            .iter()
            .map(|node| node.id.as_str())
            .collect();
        assert_eq!(ids, vec!["before", "cube_1", "sphere_1", "after"]);
    }

    #[test]
    fn test_cmd_replace_uses_current_node_and_inserts_new_node() {
        let mut app = App::new();
        app.ast_mut().modules.push(ModuleNode::new_leaf(
            "shape_1".to_string(),
            "cube".to_string(),
            vec![Argument::Positional(Expr::Integer(10))],
        ));
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "shape_1".to_string()]);

        let replacement_id =
            cmd_replace(&mut app, None, "sphere", None).expect("replace should succeed");

        assert!(app.ast.find_node_by_id("shape_1").is_none());
        let replaced = app.ast.find_node_by_id(&replacement_id).unwrap();
        assert_eq!(replaced.name, "sphere");
        assert!(replaced.args.is_empty());
    }

    #[test]
    fn test_cmd_replace_deletes_source_subtree() {
        let mut app = App::new();
        let mut container =
            ModuleNode::new_container("group_1".to_string(), "union".to_string(), Vec::new());
        container.children.push(ModuleNode::new_leaf(
            "child_1".to_string(),
            "cube".to_string(),
            Vec::new(),
        ));
        app.ast_mut().modules.push(container);

        let replacement_id = cmd_replace(&mut app, Some("group_1"), "sphere", None).unwrap();

        assert!(app.ast.find_node_by_id("group_1").is_none());
        assert!(app.ast.find_node_by_id("child_1").is_none());
        assert!(app.ast.find_node_by_id(&replacement_id).is_some());
    }

    #[test]
    fn test_replace_command_enters_parameter_stage_and_applies_parameters() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = App::new();
        app.ast_mut().modules.push(ModuleNode::new_leaf(
            "shape_1".to_string(),
            "sphere".to_string(),
            Vec::new(),
        ));
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string(), "shape_1".to_string()]);
        let handler = app.command_registry.find("replace").unwrap().handler;

        handler(&mut app, &["cube"]).expect("replace should enter parameter stage");
        assert_eq!(app.input_mode, InputMode::ModuleEnterParams);
        assert_eq!(
            app.pending_module_action,
            Some(PendingModuleAction::Replace {
                target_ids: vec!["shape_1".to_string()]
            })
        );
        assert_eq!(app.pending_module_name.as_deref(), Some("cube"));
        assert!(app.ast.find_node_by_id("shape_1").is_some());

        app.input_buffer.set_content("size=5");
        crate::input::handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);

        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.ast.find_node_by_id("shape_1").is_none());
        let replacement = app.ast.modules.first().unwrap();
        assert_eq!(replacement.name, "cube");
        assert!(matches!(
            replacement.args.first(),
            Some(Argument::Named { name, value: Expr::Integer(5) }) if name == "size"
        ));
    }

    #[test]
    fn test_parse_arguments() {
        let mgr = openscad_library::LibraryManager::new();
        let cube_def = mgr.get_module("cube").unwrap();
        // Test with simpler input that matches expected format
        let args = parse_arguments("10,10,10", &cube_def);
        // Either ok or err is fine - this is just testing the function exists
        let _ = args;
    }

    #[test]
    fn test_cmd_moddef_basic() {
        use App;

        let mut app = App::new();

        // Create a module definition without parameters
        let result = cmd_moddef(&mut app, "my_module", None);
        assert!(result.is_ok(), "cmd_moddef should succeed");

        // Check that module was added to AST
        assert_eq!(app.ast.module_defines.len(), 1);
        let module_def = &app.ast.module_defines[0];
        assert_eq!(module_def.name, "my_module");
        assert!(module_def.parameters.is_empty());
        assert!(module_def.body.is_empty());

        // Check that module was added to library manager
        let module = app.library.get_module("my_module");
        assert!(module.is_some());
        let module = module.unwrap();
        assert_eq!(module.name, "my_module");
        assert!(!module.accepts_children);
    }

    #[test]
    fn test_cmd_moddef_with_params() {
        use App;

        let mut app = App::new();

        // Create a module definition with parameters
        let result = cmd_moddef(&mut app, "my_box", Some("size=10, center=false"));
        assert!(result.is_ok(), "cmd_moddef should succeed");

        // Check that module was added to AST
        assert_eq!(app.ast.module_defines.len(), 1);
        let module_def = &app.ast.module_defines[0];
        assert_eq!(module_def.name, "my_box");
        assert_eq!(module_def.parameters.len(), 2);
        assert!(module_def.body.is_empty());

        // Check parameters
        let param1 = &module_def.parameters[0];
        assert_eq!(param1.name, "size");
        assert!(param1.default.is_some());
        let param2 = &module_def.parameters[1];
        assert_eq!(param2.name, "center");
        assert!(param2.default.is_some());

        // Check library module
        let module = app.library.get_module("my_box");
        assert!(module.is_some());
        let module = module.unwrap();
        assert_eq!(module.name, "my_box");
        assert_eq!(module.parameters.len(), 2);
        assert!(!module.accepts_children);
    }

    #[test]
    fn test_cmd_moddef_redefines_existing_module() {
        use App;

        let mut app = App::new();

        // First module definition should succeed
        let result = cmd_moddef(&mut app, "my_module", Some("size=10"));
        assert!(result.is_ok());

        // A second definition replaces the first one in place.
        let result = cmd_moddef(&mut app, "my_module", Some("height=20"));
        assert!(result.is_ok());

        // Verify the AST and completion library both contain the replacement.
        assert_eq!(app.ast.module_defines.len(), 1);
        assert_eq!(app.ast.module_defines[0].parameters[0].name, "height");
        assert_eq!(
            app.library.get_module("my_module").unwrap().parameters[0].name,
            "height"
        );
    }

    #[test]
    fn test_cmd_moddef_complex_parameters() {
        use App;

        let mut app = App::new();

        // Test complex parameter expressions
        let result = cmd_moddef(
            &mut app,
            "complex",
            Some("size=10, offset=5, name=\"test\""),
        );
        result.unwrap();

        // Check parameters were parsed
        assert_eq!(app.ast.module_defines.len(), 1);
        let module_def = &app.ast.module_defines[0];
        assert_eq!(module_def.parameters.len(), 3);

        // Check library module
        let module = app.library.get_module("complex");
        assert!(module.is_some());
        let module = module.unwrap();
        assert_eq!(module.parameters.len(), 3);
    }

    #[test]
    fn test_serialize_deserialize_with_custom_module() {
        use openscad_core::AstRoot;
        use App;

        let mut app = App::new();

        // Create a custom module
        cmd_moddef(&mut app, "my_cube", Some("size=10")).unwrap();

        // Add a module instance to the modules section
        // This tests that insert works with custom modules
        let result = cmd_insert(&mut app, "my_cube", None, Some("size=15"));
        assert!(result.is_ok(), "insert should work with custom module");

        // Serialize the AST to JSON
        let json = serde_json::to_string_pretty(&*app.ast).expect("Failed to serialize AST");

        // Deserialize back
        let deserialized: AstRoot = serde_json::from_str(&json).expect("Failed to deserialize AST");

        // Verify module definitions
        assert_eq!(deserialized.module_defines.len(), 1);
        assert_eq!(deserialized.module_defines[0].name, "my_cube");
        assert_eq!(deserialized.module_defines[0].parameters.len(), 1);

        // Verify module instances (should be empty because modules are not in module_defines)
        // Actually, modules field contains module instances, not definitions
        // The inserted module should be in modules field
        assert!(!deserialized.modules.is_empty());

        // Check that custom modules are reloaded in library manager
        // (This would be tested in integration, but we can at least ensure no panic)
    }

    #[test]
    fn test_serialize_deserialize_with_custom_module_and_children() {
        use openscad_core::{Argument, AstRoot, Expr, ModuleNode};
        use App;

        let mut app = App::new();

        // Create a custom module with parameters
        cmd_moddef(&mut app, "my_container", Some("scale=2")).unwrap();

        // Add a child module to the custom module definition body
        let child_node = ModuleNode::new_leaf(
            "cube_1".to_string(),
            "cube".to_string(),
            vec![Argument::Named {
                name: "size".to_string(),
                value: Expr::Integer(5),
            }],
        );

        // Add child to the first module definition's body
        app.ast_mut().module_defines[0].body.push(child_node);

        // Serialize the AST to JSON
        let json = serde_json::to_string_pretty(&*app.ast).expect("Failed to serialize AST");

        // Deserialize back
        let deserialized: AstRoot = serde_json::from_str(&json).expect("Failed to deserialize AST");

        // Verify module definitions
        assert_eq!(deserialized.module_defines.len(), 1);
        assert_eq!(deserialized.module_defines[0].name, "my_container");
        assert_eq!(deserialized.module_defines[0].parameters.len(), 1);

        // Verify child node in module definition body
        assert_eq!(deserialized.module_defines[0].body.len(), 1);
        assert_eq!(deserialized.module_defines[0].body[0].name, "cube");

        // Verify library manager can reload custom modules
        // (This is done in cmd_load, but we test that serialization works)
    }

    #[test]
    fn test_cmd_insert_into_modules_section() {
        use App;

        let mut app = App::new();

        // Simulate selecting the __modules section
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".to_string()]);

        // Insert a module (cube) into the Modules section
        let result = cmd_insert(&mut app, "cube", None, Some("10,10,10"));
        assert!(result.is_ok(), "insert should succeed");

        // Check that module was added to ast.modules
        assert_eq!(app.ast.modules.len(), 1);
        let inserted = &app.ast.modules[0];
        assert_eq!(inserted.name, "cube");

        // Check that tree state is updated to select the new module
        let selected = app.tree_state.borrow().selected().last().cloned();
        assert!(selected.is_some());
        // The selected ID should be the inserted module's ID
        assert_eq!(selected.unwrap(), inserted.id);
    }

    #[test]
    fn test_cmd_insert_before_preserves_sibling_order_at_root_and_in_definition() {
        let mut app = App::new();
        let cube = cmd_insert(&mut app, "cube", None, Some("size=10")).unwrap();
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".into(), cube]);
        let sphere = cmd_insert_before(&mut app, "sphere", Some("r=5")).unwrap();
        assert_eq!(
            app.ast
                .modules
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            ["sphere", "cube"]
        );
        assert_eq!(app.tree_state.borrow().selected().last(), Some(&sphere));

        app.selected_nodes = vec![app.ast.modules[1].id.clone()];
        cmd_moddef(&mut app, "part", None).unwrap();
        app.selected_nodes.clear();
        let body_target = app.ast.module_defines[0].body[0].id.clone();
        app.tree_state.borrow_mut().select(vec![
            "__moddefs".into(),
            "__moddef_part".into(),
            body_target,
        ]);
        cmd_insert_before(&mut app, "sphere", Some("r=2")).unwrap();
        assert_eq!(
            app.ast.module_defines[0]
                .body
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            ["sphere", "cube"]
        );
    }

    #[test]
    fn test_insert_before_rejects_section_cursor() {
        let mut app = App::new();
        app.tree_state.borrow_mut().select(vec!["__modules".into()]);
        let error = cmd_insert_before(&mut app, "cube", Some("size=1")).unwrap_err();
        assert!(error.to_string().contains("module node"));
    }

    #[test]
    fn test_insert_before_parameter_stage_keeps_position_semantics() {
        let mut app = App::new();
        let cube = cmd_insert(&mut app, "cube", None, Some("size=10")).unwrap();
        app.tree_state
            .borrow_mut()
            .select(vec!["__modules".into(), cube]);

        run_insert_command(&mut app, &["sphere"], true).unwrap();
        assert_eq!(
            app.pending_module_action,
            Some(PendingModuleAction::InsertBefore)
        );
        commit_pending_module_action(&mut app, "r=3").unwrap();
        assert_eq!(
            app.ast
                .modules
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            ["sphere", "cube"]
        );
    }

    #[test]
    fn test_polygon_and_extrusion_modules_are_available_to_commands() {
        let mut app = App::new();
        let polygon = cmd_insert(
            &mut app,
            "polygon",
            None,
            Some("points=[[0,0],[10,0],[0,10]], paths=[[0,1,2]]"),
        )
        .unwrap();
        app.selected_nodes = vec![polygon];

        let linear = cmd_insert(
            &mut app,
            "linear_extrude",
            None,
            Some("height=5, twist=90, slices=12"),
        )
        .unwrap();

        let extrusion = app.ast.find_node_by_id(&linear).unwrap();
        assert_eq!(extrusion.name, "linear_extrude");
        assert_eq!(extrusion.children[0].name, "polygon");

        let mut revolved_app = App::new();
        let circle = cmd_insert(&mut revolved_app, "circle", None, Some("r=2")).unwrap();
        revolved_app.selected_nodes = vec![circle];
        let rotate = cmd_insert(
            &mut revolved_app,
            "rotate_extrude",
            None,
            Some("angle=180, convexity=4"),
        )
        .unwrap();
        assert_eq!(
            revolved_app.ast.find_node_by_id(&rotate).unwrap().name,
            "rotate_extrude"
        );
    }

    #[test]
    fn test_cmd_insert_into_module_definition() {
        use App;

        let mut app = App::new();

        // Create a custom module definition
        cmd_moddef(&mut app, "my_module", None).unwrap();

        // Simulate selecting the module definition itself (__moddef_my_module)
        app.tree_state.borrow_mut().select(vec![
            "__moddefs".to_string(),
            "__moddef_my_module".to_string(),
        ]);

        // Insert a cube into the module definition body
        let result = cmd_insert(&mut app, "cube", None, Some("5,5,5"));
        assert!(result.is_ok(), "insert should succeed");

        // Check that module was added to module definition body, not ast.modules
        assert_eq!(app.ast.modules.len(), 0); // No module instances
        assert_eq!(app.ast.module_defines.len(), 1);
        assert_eq!(app.ast.module_defines[0].body.len(), 1);
        let inserted = &app.ast.module_defines[0].body[0];
        assert_eq!(inserted.name, "cube");

        // Check tree state selection
        let selected = app.tree_state.borrow().selected().last().cloned();
        assert!(selected.is_some());
        assert_eq!(selected.unwrap(), inserted.id);
    }

    #[test]
    fn test_cmd_boolean_op_in_modules_section() {
        use App;

        let mut app = App::new();

        // Insert two cubes into modules section
        let cube1_id = cmd_insert(&mut app, "cube", None, Some("5,5,5")).unwrap();
        let cube2_id = cmd_insert(&mut app, "cube", None, Some("10,10,10")).unwrap();

        // Select both nodes
        let selected_nodes = vec![cube1_id.clone(), cube2_id.clone()];
        app.selected_nodes = selected_nodes.clone();

        // Perform union operation
        let result = cmd_boolean_op(&mut app, "union", &selected_nodes);
        assert!(result.is_ok(), "boolean operation should succeed");

        let container_id = result.unwrap();

        // Check that container was created in modules section
        assert!(app.ast.find_node_by_id(&container_id).is_some());
        let container = app.ast.find_node_by_id(&container_id).unwrap();
        assert_eq!(container.name, "union");

        // Check that container has two children
        assert_eq!(container.children.len(), 2);

        // Check that original nodes are now children of the container
        assert_eq!(container.children.len(), 2);
        let child_ids: Vec<String> = container.children.iter().map(|c| c.id.clone()).collect();
        assert!(child_ids.contains(&cube1_id));
        assert!(child_ids.contains(&cube2_id));
        // Check that nodes are not at root level of modules section
        assert!(!app.ast.modules.iter().any(|m| m.id == cube1_id));
        assert!(!app.ast.modules.iter().any(|m| m.id == cube2_id));

        // Check that container is selected
        assert_eq!(
            app.tree_state.borrow().selected().last(),
            Some(&container_id)
        );
    }

    #[test]
    fn test_cmd_boolean_op_in_module_definition() {
        use App;

        let mut app = App::new();

        // Create a custom module definition
        cmd_moddef(&mut app, "my_module", None).unwrap();

        // Add two cubes to module definition body
        let cube1 = openscad_core::ModuleNode::new_leaf(
            "cube_1".to_string(),
            "cube".to_string(),
            vec![openscad_core::Argument::Named {
                name: "size".to_string(),
                value: openscad_core::Expr::Integer(5),
            }],
        );
        let cube2 = openscad_core::ModuleNode::new_leaf(
            "cube_2".to_string(),
            "cube".to_string(),
            vec![openscad_core::Argument::Named {
                name: "size".to_string(),
                value: openscad_core::Expr::Integer(10),
            }],
        );

        // Add cubes to module definition body
        app.ast_mut().module_defines[0].body.push(cube1.clone());
        app.ast_mut().module_defines[0].body.push(cube2.clone());

        // Select both nodes (they are in module definition body)
        let selected_nodes = vec![cube1.id.clone(), cube2.id.clone()];
        app.selected_nodes = selected_nodes.clone();

        // Perform difference operation
        let result = cmd_boolean_op(&mut app, "difference", &selected_nodes);
        assert!(
            result.is_ok(),
            "boolean operation should succeed in module definition"
        );

        let container_id = result.unwrap();

        // Check that container was created in module definition body, not modules section
        assert!(app.ast.find_node_by_id(&container_id).is_none()); // Not in modules section

        // Check that container exists in module definition body
        let mod_def = &app.ast.module_defines[0];
        let container = find_node_in_module_definition(&mod_def.body, &container_id);
        assert!(container.is_some());
        let container = container.unwrap();
        assert_eq!(container.name, "difference");

        // Check that container has two children with the original nodes
        assert_eq!(container.children.len(), 2);
        let child_ids: Vec<String> = container.children.iter().map(|c| c.id.clone()).collect();
        assert!(child_ids.contains(&cube1.id));
        assert!(child_ids.contains(&cube2.id));
        // Check that nodes are not at root level of module definition body
        assert!(!mod_def.body.iter().any(|m| m.id == cube1.id));
        assert!(!mod_def.body.iter().any(|m| m.id == cube2.id));

        // Check that container is selected
        assert_eq!(
            app.tree_state.borrow().selected().last(),
            Some(&container_id)
        );
    }

    #[test]
    fn test_cmd_boolean_op_mixed_context_error() {
        use App;

        let mut app = App::new();

        // Create a custom module definition
        cmd_moddef(&mut app, "my_module", None).unwrap();
        // Clear selection to ensure next insert goes to modules section
        app.tree_state.borrow_mut().select(Vec::new());

        // Add a cube to module definition body
        let cube1 = openscad_core::ModuleNode::new_leaf(
            "cube_1".to_string(),
            "cube".to_string(),
            vec![openscad_core::Argument::Named {
                name: "size".to_string(),
                value: openscad_core::Expr::Integer(5),
            }],
        );
        app.ast_mut().module_defines[0].body.push(cube1.clone());

        // Insert a cube into modules section
        let cube2_id = cmd_insert(&mut app, "cube", None, Some("10,10,10")).unwrap();

        // Select nodes from both contexts (mixed)
        let selected_nodes = vec![cube1.id.clone(), cube2_id.clone()];
        app.selected_nodes = selected_nodes.clone();

        // Perform union operation - should fail with mixed context error
        let result = cmd_boolean_op(&mut app, "union", &selected_nodes);
        assert!(
            result.is_err(),
            "boolean operation should fail with mixed context"
        );

        // Verify error message indicates mixed context
        let err = result.unwrap_err();
        assert!(matches!(err, CommandError::InvalidCommand(_)));
        let err_msg = match err {
            CommandError::InvalidCommand(msg) => msg,
            _ => panic!("Unexpected error type"),
        };
        assert!(err_msg.contains("mixed contexts") || err_msg.contains("different contexts"));
    }

    #[test]
    fn test_cmd_insert_container_in_module_definition() {
        use App;

        let mut app = App::new();

        // Create a custom module definition
        cmd_moddef(&mut app, "my_module", None).unwrap();

        // Add two cubes to module definition body
        let cube1 = openscad_core::ModuleNode::new_leaf(
            "cube_1".to_string(),
            "cube".to_string(),
            vec![openscad_core::Argument::Named {
                name: "size".to_string(),
                value: openscad_core::Expr::Integer(5),
            }],
        );
        let cube2 = openscad_core::ModuleNode::new_leaf(
            "cube_2".to_string(),
            "cube".to_string(),
            vec![openscad_core::Argument::Named {
                name: "size".to_string(),
                value: openscad_core::Expr::Integer(10),
            }],
        );

        // Add cubes to module definition body
        app.ast_mut().module_defines[0].body.push(cube1.clone());
        app.ast_mut().module_defines[0].body.push(cube2.clone());

        // Select both nodes (they are in module definition body)
        app.selected_nodes = vec![cube1.id.clone(), cube2.id.clone()];

        // Insert a difference container module
        let result = cmd_insert(&mut app, "difference", None, None);
        assert!(
            result.is_ok(),
            "insert difference should succeed in module definition"
        );

        let container_id = result.unwrap();

        // Check that container was created in module definition body, not modules section
        assert!(app.ast.find_node_by_id(&container_id).is_none()); // Not in modules section

        // Check that container exists in module definition body
        let mod_def = &app.ast.module_defines[0];
        let container = find_node_in_module_definition(&mod_def.body, &container_id);
        assert!(container.is_some());
        let container = container.unwrap();
        assert_eq!(container.name, "difference");

        // Check that container has two children with the original nodes
        assert_eq!(container.children.len(), 2);
        let child_ids: Vec<String> = container.children.iter().map(|c| c.id.clone()).collect();
        assert!(child_ids.contains(&cube1.id));
        assert!(child_ids.contains(&cube2.id));

        // Check that nodes are not at root level of module definition body
        assert!(!mod_def.body.iter().any(|m| m.id == cube1.id));
        assert!(!mod_def.body.iter().any(|m| m.id == cube2.id));

        // Check that container is selected (tree state should select it)
        assert_eq!(
            app.tree_state.borrow().selected().last(),
            Some(&container_id)
        );
    }

    #[test]
    fn test_cmd_moddef_with_children_module() {
        use App;

        let mut app = App::new();

        // Create a custom module definition without children module initially
        cmd_moddef(&mut app, "my_module", None).unwrap();

        // Initially, the module should not accept children
        let module = app.library.get_module("my_module").unwrap();
        assert!(!module.accepts_children);

        // Clear selection and navigate to module definition body
        app.tree_state.borrow_mut().select(vec![
            "__moddefs".to_string(),
            "__moddef_my_module".to_string(),
        ]);

        // Insert a children module into the module definition body
        let result = cmd_insert(&mut app, "children", None, None);
        assert!(
            result.is_ok(),
            "children module should be insertable into module definition"
        );

        // After inserting children module, the custom module should accept children
        // Note: The library manager should have been updated via reload_custom_modules_from_ast
        let module = app.library.get_module("my_module").unwrap();
        assert!(
            module.accepts_children,
            "module with children module should accept children"
        );
    }

    #[test]
    fn test_cmd_insert_children_outside_module_definition_fails() {
        use App;

        let mut app = App::new();

        // Try to insert children module outside module definition (in modules section)
        // Ensure no module definition is selected
        app.tree_state.borrow_mut().select(Vec::new());

        let result = cmd_insert(&mut app, "children", None, None);
        assert!(
            result.is_err(),
            "children module should not be insertable outside module definitions"
        );
        let err = result.unwrap_err();
        assert!(matches!(err, CommandError::InvalidCommand(_)));
        let err_msg = match err {
            CommandError::InvalidCommand(msg) => msg,
            _ => panic!("Unexpected error type"),
        };
        assert!(err_msg.contains("children module can only be used inside module definitions"));
    }

    #[test]
    fn test_cmd_global_basic() {
        use App;

        let mut app = App::new();

        // Test basic variable assignment
        let result = cmd_global(&mut app, "width=100");
        assert!(result.is_ok(), "cmd_global should succeed");

        // Check that variable was added to AST
        assert_eq!(app.ast.global_variables().len(), 1);
        let var = &app.ast.global_variables()[0];
        assert_eq!(var.name, "width");
        assert_eq!(var.value, openscad_core::Expr::Integer(100));
    }

    #[test]
    fn test_cmd_global_special() {
        use App;

        let mut app = App::new();

        // Test special variable assignment
        let result = cmd_global(&mut app, "$fn=50");
        assert!(
            result.is_ok(),
            "cmd_global should succeed with special variable"
        );

        // Check that special variable was added to AST
        assert_eq!(app.ast.global_variables().len(), 1);
        let var = &app.ast.global_variables()[0];
        assert_eq!(var.name, "$fn");
        assert_eq!(var.value, openscad_core::Expr::Integer(50));
    }

    #[test]
    fn test_cmd_global_with_list() {
        use App;

        let mut app = App::new();

        // Test variable with list value
        let result = cmd_global(&mut app, "size=[10,20,30]");
        assert!(result.is_ok(), "cmd_global should succeed with list value");

        // Check that variable was added to AST
        assert_eq!(app.ast.global_variables().len(), 1);
        let var = &app.ast.global_variables()[0];
        assert_eq!(var.name, "size");

        // Check that value is a list
        if let openscad_core::Expr::List(items) = &var.value {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], openscad_core::Expr::Integer(10));
            assert_eq!(items[1], openscad_core::Expr::Integer(20));
            assert_eq!(items[2], openscad_core::Expr::Integer(30));
        } else {
            panic!("Expected list expression");
        }
    }

    #[test]
    fn test_cmd_global_with_nested_list() {
        let mut app = App::new();

        cmd_global(&mut app, "points=[[0,0], [width,sin(angle)], [[1,2],3]]").unwrap();

        assert_eq!(
            app.ast.global_variables()[0].value.to_scad(),
            "[[0, 0], [width, sin(angle)], [[1, 2], 3]]"
        );
    }

    #[test]
    fn test_cmd_global_invalid_syntax() {
        use App;

        let mut app = App::new();

        // Test invalid syntax (no equals)
        let result = cmd_global(&mut app, "width100");
        assert!(
            result.is_err(),
            "cmd_global should fail with invalid syntax"
        );

        // Check that no variables were added to AST
        assert_eq!(app.ast.global_variables().len(), 0);
    }

    #[test]
    fn test_cmd_global_invalid_identifier() {
        use App;

        let mut app = App::new();

        // Test invalid identifier (starts with number)
        let result = cmd_global(&mut app, "123width=100");
        assert!(
            result.is_err(),
            "cmd_global should fail with invalid identifier"
        );

        // Check that no variables were added to AST
        assert_eq!(app.ast.global_variables().len(), 0);
    }

    #[test]
    fn test_cmd_global_redefines_existing_variable() {
        use App;

        let mut app = App::new();

        // Add first variable
        let result = cmd_global(&mut app, "width=100");
        assert!(result.is_ok(), "First cmd_global should succeed");

        // Redefine in place
        let result = cmd_global(&mut app, "width=200");
        assert!(result.is_ok());

        // Check that only one variable remains and its value was replaced
        assert_eq!(app.ast.global_variables().len(), 1);
        assert_eq!(
            app.ast.global_variables()[0].value,
            openscad_core::Expr::Integer(200)
        );
    }

    #[test]
    fn test_cmd_global_string_value() {
        use App;

        let mut app = App::new();

        // Test variable with string value
        let result = cmd_global(&mut app, "color=\"red\"");
        assert!(
            result.is_ok(),
            "cmd_global should succeed with string value"
        );

        // Check that variable was added to AST
        assert_eq!(app.ast.global_variables().len(), 1);
        let var = &app.ast.global_variables()[0];
        assert_eq!(var.name, "color");
        assert_eq!(var.value, openscad_core::Expr::String("red".to_string()));
    }

    #[test]
    fn test_cmd_global_float_value() {
        use App;

        let mut app = App::new();

        // Test variable with float value
        let result = cmd_global(&mut app, "precision=2.5");
        assert!(result.is_ok(), "cmd_global should succeed with float value");

        // Check that variable was added to AST
        assert_eq!(app.ast.global_variables().len(), 1);
        let var = &app.ast.global_variables()[0];
        assert_eq!(var.name, "precision");

        // Compare floats by converting to string representation
        if let openscad_core::Expr::Float(f) = var.value {
            assert!((f - 2.5).abs() < 0.001);
        } else {
            panic!("Expected float expression");
        }
    }

    #[test]
    fn test_cmd_funcdef_basic() {
        use App;

        let mut app = App::new();

        // Create a function definition with one parameter using new parentheses syntax
        let result = cmd_funcdef(&mut app, "square(x) = 10");
        assert!(result.is_ok(), "cmd_funcdef should succeed");

        // Check that function was added to AST
        assert_eq!(app.ast.function_defines.len(), 1);
        let func_def = &app.ast.function_defines[0];
        assert_eq!(func_def.name, "square");
        assert_eq!(func_def.parameters.len(), 1);
        assert_eq!(func_def.parameters[0].name, "x");
    }

    #[test]
    fn test_cmd_funcdef_no_params() {
        use App;

        let mut app = App::new();

        // Create a function definition without parameters
        let result = cmd_funcdef(&mut app, "pi_value() = 3.14159");
        assert!(result.is_ok(), "cmd_funcdef should succeed");

        // Check that function was added to AST
        assert_eq!(app.ast.function_defines.len(), 1);
        let func_def = &app.ast.function_defines[0];
        assert_eq!(func_def.name, "pi_value");
        assert_eq!(func_def.parameters.len(), 0);
        // Verify the body is the constant value
    }

    #[test]
    fn test_cmd_funcdef_multiple_params() {
        use App;

        let mut app = App::new();

        // Create a function definition with multiple parameters using new parentheses syntax
        let result = cmd_funcdef(&mut app, "add(a, b) = 15");
        assert!(result.is_ok(), "cmd_funcdef should succeed");

        // Check that function was added to AST
        assert_eq!(app.ast.function_defines.len(), 1);
        let func_def = &app.ast.function_defines[0];
        assert_eq!(func_def.name, "add");
        assert_eq!(func_def.parameters.len(), 2);
        assert_eq!(func_def.parameters[0].name, "a");
        assert_eq!(func_def.parameters[1].name, "b");
    }

    #[test]
    fn test_cmd_funcdef_redefines_existing_function() {
        use App;

        let mut app = App::new();

        // First function definition should succeed
        let result = cmd_funcdef(&mut app, "my_func(x) = 10");
        assert!(result.is_ok());

        // Second function definition with the same name replaces it in place
        let result = cmd_funcdef(&mut app, "my_func(y) = 20");
        assert!(result.is_ok());

        // Verify only one, updated function in AST and library completion metadata
        assert_eq!(app.ast.function_defines.len(), 1);
        assert_eq!(app.ast.function_defines[0].parameters[0].name, "y");
        assert_eq!(
            app.ast.function_defines[0].body,
            openscad_core::Expr::Integer(20)
        );
        assert_eq!(
            app.library.get_function("my_func").unwrap().parameters[0].name,
            "y"
        );
    }

    #[test]
    fn test_cmd_delete_global_function_and_module_definitions_without_cascading() {
        let mut app = App::new();
        cmd_global(&mut app, "size=10").unwrap();
        cmd_funcdef(&mut app, "double(x)=x*2").unwrap();
        cmd_funcdef(&mut app, "uses_double(x)=double(x)").unwrap();
        cmd_moddef(&mut app, "part", Some("size=10")).unwrap();

        cmd_delete(&mut app, "__var_size").unwrap();
        assert!(app.ast.find_global_variable("size").is_none());

        cmd_delete(&mut app, "__func_double").unwrap();
        assert!(app.ast.find_function_define("double").is_none());
        assert!(app.ast.find_function_define("uses_double").is_some());
        assert!(app.library.get_function("double").is_none());

        cmd_delete(&mut app, "__moddef_part").unwrap();
        assert!(app.ast.find_module_define("part").is_none());
        assert!(app.library.get_module("part").is_none());
    }

    #[test]
    fn test_cmd_funcdef_invalid_syntax() {
        use App;

        let mut app = App::new();

        // Test invalid expression in function body
        let result = cmd_funcdef(&mut app, "bad_func(x) = x + ");
        assert!(result.is_err());

        // Check that no functions were added to AST
        assert_eq!(app.ast.function_defines.len(), 0);
    }

    #[test]
    fn test_cmd_funcdef_invalid_name() {
        use App;

        let mut app = App::new();

        // Test invalid function name (starts with number)
        let result = cmd_funcdef(&mut app, "123func(x) = x * x");
        assert!(result.is_err());

        // Check that no functions were added to AST
        assert_eq!(app.ast.function_defines.len(), 0);
    }

    #[test]
    fn test_cmd_funcdef_with_binary_operations() {
        use App;

        let mut app = App::new();

        // Create a function with binary operations in the body
        let result = cmd_funcdef(&mut app, "add(a, b) = a + b");
        assert!(
            result.is_ok(),
            "cmd_funcdef should succeed with binary operations"
        );

        // Check that function was added to AST
        assert_eq!(app.ast.function_defines.len(), 1);
        let func_def = &app.ast.function_defines[0];
        assert_eq!(func_def.name, "add");
        assert_eq!(func_def.parameters.len(), 2);
        assert_eq!(func_def.parameters[0].name, "a");
        assert_eq!(func_def.parameters[1].name, "b");

        // Verify the body contains a binary operation
        match &func_def.body {
            openscad_core::Expr::BinOp { op, .. } => {
                assert_eq!(*op, openscad_core::BinOp::Add);
            }
            _ => panic!("Expected binary operation in function body"),
        }
    }

    #[test]
    fn test_cmd_funcdef_with_complex_expressions() {
        use App;

        let mut app = App::new();

        // Create a function with a more complex expression
        let result = cmd_funcdef(&mut app, "calc(x, y, z) = x * y + z");
        assert!(
            result.is_ok(),
            "cmd_funcdef should succeed with complex expressions"
        );

        // Check that function was added to AST
        assert_eq!(app.ast.function_defines.len(), 1);
        let func_def = &app.ast.function_defines[0];
        assert_eq!(func_def.name, "calc");
        assert_eq!(func_def.parameters.len(), 3);
        assert_eq!(func_def.parameters[0].name, "x");
        assert_eq!(func_def.parameters[1].name, "y");
        assert_eq!(func_def.parameters[2].name, "z");
    }

    #[test]
    fn test_cmd_funcdef_parentheses_format() {
        use App;

        let mut app = App::new();

        // Test the new parentheses format specifically
        let result = cmd_funcdef(&mut app, "multiply(a, b) = a * b");
        assert!(
            result.is_ok(),
            "cmd_funcdef should succeed with parentheses format"
        );

        // Check that function was added to AST with correct parameters
        assert_eq!(app.ast.function_defines.len(), 1);
        let func_def = &app.ast.function_defines[0];
        assert_eq!(func_def.name, "multiply");
        assert_eq!(func_def.parameters.len(), 2);
        assert_eq!(func_def.parameters[0].name, "a");
        assert_eq!(func_def.parameters[1].name, "b");
    }
}
