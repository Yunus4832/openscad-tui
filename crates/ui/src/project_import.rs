use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use openscad_core::{
    parse_scad, parse_scad_definitions, AstRoot, EmbeddedSourceFile, EmbeddedSourceRole,
    SourceDependency, SourceDependencyKind,
};

struct SourceDraft {
    path: PathBuf,
    content: String,
    ast: AstRoot,
}

struct DependencyDraft {
    from: PathBuf,
    to: PathBuf,
    reference: String,
    kind: SourceDependencyKind,
}

pub fn import_scad_project(entry: &Path) -> Result<AstRoot, String> {
    let entry = entry
        .canonicalize()
        .map_err(|error| format!("Failed to resolve '{}': {error}", entry.display()))?;
    let mut sources = HashMap::new();
    let mut dependencies = Vec::new();
    let mut visiting = HashSet::new();
    collect_source(&entry, true, &mut sources, &mut dependencies, &mut visiting)?;

    let common_root = common_parent(sources.keys()).ok_or_else(|| {
        format!(
            "Could not determine a common source directory for '{}'",
            entry.display()
        )
    })?;
    let virtual_paths: HashMap<PathBuf, String> = sources
        .keys()
        .map(|path| {
            let relative = path.strip_prefix(&common_root).unwrap_or(path);
            (path.clone(), portable_path(relative))
        })
        .collect();
    let entry_virtual = virtual_paths
        .get(&entry)
        .cloned()
        .ok_or_else(|| "Entry source was not collected".to_string())?;

    let entry_draft = sources
        .remove(&entry)
        .ok_or_else(|| "Entry source was not collected".to_string())?;
    let mut project = entry_draft.ast.clone();
    project.source_directory = Some(common_root.to_string_lossy().into_owned());
    project.entry_source = Some(entry_virtual.clone());

    let mut embedded = Vec::with_capacity(sources.len() + 1);
    embedded.push(to_embedded_source(
        entry_draft,
        entry_virtual,
        EmbeddedSourceRole::Entry,
    ));
    let mut remaining: Vec<_> = sources.into_values().collect();
    remaining.sort_by(|left, right| left.path.cmp(&right.path));
    for draft in remaining {
        let virtual_path = virtual_paths
            .get(&draft.path)
            .cloned()
            .ok_or_else(|| "Dependency source has no virtual path".to_string())?;
        embedded.push(to_embedded_source(
            draft,
            virtual_path,
            EmbeddedSourceRole::Dependency,
        ));
    }
    project.embedded_sources = embedded;
    project.source_dependencies = dependencies
        .into_iter()
        .map(|dependency| SourceDependency {
            from: virtual_paths[&dependency.from].clone(),
            to: virtual_paths[&dependency.to].clone(),
            reference: dependency.reference,
            kind: dependency.kind,
        })
        .collect();
    Ok(project)
}

/// Attach a SCAD project as a definition library and return its embedded entry path.
pub fn attach_scad_library(project: &mut AstRoot, library: &Path) -> Result<String, String> {
    ensure_virtual_entry(project);
    let mut imported = import_scad_project(library)?;
    let stem = library
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("library")
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let occupied: HashSet<_> = project
        .embedded_sources
        .iter()
        .map(|source| source.virtual_path.clone())
        .collect();
    let mut suffix = 1usize;
    let prefix = loop {
        let candidate = if suffix == 1 {
            format!("libraries/{stem}")
        } else {
            format!("libraries/{stem}-{suffix}")
        };
        if !occupied
            .iter()
            .any(|path| path == &candidate || path.starts_with(&format!("{candidate}/")))
        {
            break candidate;
        }
        suffix += 1;
    };
    let paths: HashMap<_, _> = imported
        .embedded_sources
        .iter()
        .map(|source| {
            (
                source.virtual_path.clone(),
                format!("{prefix}/{}", source.virtual_path),
            )
        })
        .collect();
    let imported_entry_original = imported
        .entry_source
        .take()
        .ok_or_else(|| "Imported library has no entry source".to_string())?;
    let imported_entry = paths[&imported_entry_original].clone();
    for source in &mut imported.embedded_sources {
        let is_library_entry = source.virtual_path == imported_entry_original;
        source.virtual_path = paths[&source.virtual_path].clone();
        source.role = if is_library_entry {
            EmbeddedSourceRole::Library
        } else {
            EmbeddedSourceRole::Dependency
        };
    }
    for dependency in &mut imported.source_dependencies {
        dependency.from = paths[&dependency.from].clone();
        dependency.to = paths[&dependency.to].clone();
    }
    project.embedded_sources.extend(imported.embedded_sources);
    project
        .source_dependencies
        .extend(imported.source_dependencies);
    Ok(imported_entry)
}

fn ensure_virtual_entry(project: &mut AstRoot) {
    if project.entry_source.is_some() {
        return;
    }
    let entry = "main.scad".to_string();
    project.entry_source = Some(entry.clone());
    project.embedded_sources.push(EmbeddedSourceFile {
        virtual_path: entry,
        original_path: None,
        role: EmbeddedSourceRole::Entry,
        content: String::new(),
        global_variables: Vec::new(),
        module_defines: Vec::new(),
        function_defines: Vec::new(),
    });
}

fn collect_source(
    path: &Path,
    structured: bool,
    sources: &mut HashMap<PathBuf, SourceDraft>,
    dependencies: &mut Vec<DependencyDraft>,
    visiting: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("Failed to resolve '{}': {error}", path.display()))?;
    if sources.contains_key(&path) || !visiting.insert(path.clone()) {
        return Ok(());
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read SCAD file '{}': {error}", path.display()))?;
    let ast = if structured {
        parse_scad(&content)
    } else {
        parse_scad_definitions(&content)
    }
    .map_err(|error| format!("Failed to parse '{}': {error}", path.display()))?;
    let references = ast
        .includes
        .iter()
        .map(|reference| (reference.clone(), SourceDependencyKind::Include))
        .chain(
            ast.uses
                .iter()
                .map(|reference| (reference.clone(), SourceDependencyKind::Use)),
        )
        .collect::<Vec<_>>();
    sources.insert(
        path.clone(),
        SourceDraft {
            path: path.clone(),
            content,
            ast,
        },
    );

    for (reference, kind) in references {
        let referenced = Path::new(&reference);
        let candidate = if referenced.is_absolute() {
            referenced.to_path_buf()
        } else {
            path.parent()
                .unwrap_or_else(|| Path::new("."))
                .join(referenced)
        };
        // References not found next to the caller may be supplied by OPENSCADPATH or an OpenSCAD
        // installation library. Keep those external instead of rejecting an otherwise valid file.
        if !candidate.is_file() {
            continue;
        }
        let target = candidate
            .canonicalize()
            .map_err(|error| format!("Failed to resolve '{}': {error}", candidate.display()))?;
        dependencies.push(DependencyDraft {
            from: path.clone(),
            to: target.clone(),
            reference,
            kind,
        });
        collect_source(&target, false, sources, dependencies, visiting)?;
    }
    visiting.remove(&path);
    Ok(())
}

fn to_embedded_source(
    draft: SourceDraft,
    virtual_path: String,
    role: EmbeddedSourceRole,
) -> EmbeddedSourceFile {
    EmbeddedSourceFile {
        virtual_path,
        original_path: Some(draft.path.to_string_lossy().into_owned()),
        role,
        content: draft.content,
        global_variables: draft.ast.global_variables,
        module_defines: draft.ast.module_defines,
        function_defines: draft.ast.function_defines,
    }
}

fn common_parent<'a>(paths: impl Iterator<Item = &'a PathBuf>) -> Option<PathBuf> {
    let mut paths = paths;
    let first = paths.next()?.parent()?.to_path_buf();
    Some(paths.fold(first, |common, path| {
        let parent = path.parent().unwrap_or(path);
        let shared: PathBuf = common
            .components()
            .zip(parent.components())
            .take_while(|(left, right)| left == right)
            .map(|(component, _)| component.as_os_str())
            .collect();
        shared
    }))
}

fn portable_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_nested_sources_and_preserves_dependency_kinds() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("lib")).unwrap();
        fs::write(
            directory.path().join("main.scad"),
            "include <parts.scad>; use <lib/math.scad>; part();",
        )
        .unwrap();
        fs::write(
            directory.path().join("parts.scad"),
            "include <lib/shared.scad>; module part() { shared(); }",
        )
        .unwrap();
        fs::write(
            directory.path().join("lib/math.scad"),
            "function twice(x) = x * 2;",
        )
        .unwrap();
        fs::write(
            directory.path().join("lib/shared.scad"),
            "module shared() { cube(1); }",
        )
        .unwrap();

        let project = import_scad_project(&directory.path().join("main.scad")).unwrap();
        assert_eq!(project.embedded_sources.len(), 4);
        assert_eq!(project.source_dependencies.len(), 3);
        assert!(project
            .source_dependencies
            .iter()
            .any(|dependency| dependency.kind == SourceDependencyKind::Use));
        assert!(project
            .embedded_sources
            .iter()
            .find(|source| source.virtual_path == "parts.scad")
            .unwrap()
            .module_defines
            .iter()
            .any(|definition| definition.name == "part"));
    }
}
