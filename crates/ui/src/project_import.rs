use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use openscad_core::{
    parse_scad, parse_scad_definitions, AstRoot, EmbeddedSourceFile, EmbeddedSourceRole,
    SourceDependency, SourceDependencyKind,
};

struct SourceDraft {
    path: PathBuf,
    virtual_path: String,
    editable: bool,
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
    import_scad_project_with_library_roots(entry, &openscad_library_roots())
}

/// Import an existing SCAD source tree as project-owned editable documents.
/// Existing documents remain in the project and the imported entry becomes active.
pub fn attach_editable_scad(project: &mut AstRoot, entry: &Path) -> Result<String, String> {
    let canonical_entry = entry
        .canonicalize()
        .map_err(|error| format!("Failed to resolve '{}': {error}", entry.display()))?;
    if let Some(existing) = project.embedded_sources.iter().find(|source| {
        source.editable
            && source.original_path.as_deref() == Some(canonical_entry.to_string_lossy().as_ref())
    }) {
        let target = existing.virtual_path.clone();
        project
            .activate_source(&target)
            .map_err(|error| error.to_string())?;
        return Ok(target);
    }

    let mut imported = import_scad_project(&canonical_entry)?;
    let placeholder = project.embedded_sources.len() == 1
        && project.embedded_sources[0].editable
        && project.embedded_sources[0].original_path.is_none()
        && project.embedded_sources[0]
            .generated_content()
            .trim()
            .is_empty();
    if placeholder {
        project.embedded_sources.clear();
        project.source_dependencies.clear();
        project.entry_source = None;
        project.active_source = None;
    } else {
        project.sync_active_source();
    }

    let imported_entry = imported
        .entry_source
        .clone()
        .ok_or_else(|| "Imported source has no entry path".to_string())?;
    let occupied = project
        .embedded_sources
        .iter()
        .map(|source| source.virtual_path.as_str())
        .collect::<HashSet<_>>();
    let has_collision = imported
        .embedded_sources
        .iter()
        .any(|source| occupied.contains(source.virtual_path.as_str()));
    let prefix = has_collision.then(|| unique_import_prefix(project, &canonical_entry));
    let paths = imported
        .embedded_sources
        .iter()
        .map(|source| {
            let mapped = prefix
                .as_ref()
                .map(|prefix| format!("{prefix}/{}", source.virtual_path))
                .unwrap_or_else(|| source.virtual_path.clone());
            (source.virtual_path.clone(), mapped)
        })
        .collect::<HashMap<_, _>>();
    let target = paths[&imported_entry].clone();
    let first_project_source = project.entry_source.is_none();
    for source in &mut imported.embedded_sources {
        let is_entry = source.virtual_path == imported_entry;
        source.virtual_path = paths[&source.virtual_path].clone();
        source.role = if first_project_source && is_entry {
            EmbeddedSourceRole::Entry
        } else {
            EmbeddedSourceRole::Dependency
        };
    }
    for dependency in &mut imported.source_dependencies {
        dependency.from = paths[&dependency.from].clone();
        dependency.to = paths[&dependency.to].clone();
    }
    if first_project_source {
        project.entry_source = Some(target.clone());
    }
    project.embedded_sources.extend(imported.embedded_sources);
    project
        .source_dependencies
        .extend(imported.source_dependencies);
    project
        .activate_source(&target)
        .map_err(|error| error.to_string())?;
    Ok(target)
}

fn unique_import_prefix(project: &AstRoot, entry: &Path) -> String {
    let base = entry
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .or_else(|| entry.file_stem().and_then(|value| value.to_str()))
        .unwrap_or("import")
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let occupied = project
        .embedded_sources
        .iter()
        .map(|source| source.virtual_path.as_str())
        .collect::<HashSet<_>>();
    (1..)
        .map(|suffix| {
            if suffix == 1 {
                base.clone()
            } else {
                format!("{base}-{suffix}")
            }
        })
        .find(|candidate| {
            !occupied
                .iter()
                .any(|path| *path == candidate || path.starts_with(&format!("{candidate}/")))
        })
        .expect("an import namespace is always available")
}

fn import_scad_project_with_library_roots(
    entry: &Path,
    library_roots: &[PathBuf],
) -> Result<AstRoot, String> {
    let entry = entry
        .canonicalize()
        .map_err(|error| format!("Failed to resolve '{}': {error}", entry.display()))?;
    let entry_virtual = entry
        .file_name()
        .map(PathBuf::from)
        .and_then(|path| portable_path(&path))
        .ok_or_else(|| format!("Could not determine a filename for '{}'", entry.display()))?;
    let mut sources = HashMap::new();
    let mut dependencies = Vec::new();
    let mut visiting = HashSet::new();
    collect_source(
        &entry,
        entry_virtual.clone(),
        true,
        library_roots,
        &mut sources,
        &mut dependencies,
        &mut visiting,
    )?;

    let virtual_paths: HashMap<PathBuf, String> = sources
        .iter()
        .map(|(path, draft)| (path.clone(), draft.virtual_path.clone()))
        .collect();

    let entry_draft = sources
        .remove(&entry)
        .ok_or_else(|| "Entry source was not collected".to_string())?;
    let mut project = entry_draft.ast.clone();
    project.source_directory = entry
        .parent()
        .map(|path| path.to_string_lossy().into_owned());
    project.entry_source = Some(entry_virtual.clone());
    project.active_source = Some(entry_virtual.clone());

    let mut embedded = Vec::with_capacity(sources.len() + 1);
    embedded.push(to_embedded_source(
        entry_draft,
        entry_virtual,
        EmbeddedSourceRole::Entry,
    ));
    let mut remaining: Vec<_> = sources.into_values().collect();
    remaining.sort_by(|left, right| left.path.cmp(&right.path));
    for draft in remaining {
        let virtual_path = draft.virtual_path.clone();
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
        source.editable = false;
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
    project
        .embedded_sources
        .push(EmbeddedSourceFile::empty(entry, EmbeddedSourceRole::Entry));
}

fn collect_source(
    path: &Path,
    virtual_path: String,
    editable: bool,
    library_roots: &[PathBuf],
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
    let ast = if editable {
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
            virtual_path: virtual_path.clone(),
            editable,
            content,
            ast,
        },
    );

    for (reference, kind) in references {
        let Some((candidate, target_virtual, target_editable)) =
            resolve_reference(&path, &virtual_path, &reference, editable, library_roots)
        else {
            // OpenSCAD may know additional platform-specific paths. Preserve unresolved
            // directives so its own resolver still gets a chance at render time.
            continue;
        };
        let target = candidate
            .canonicalize()
            .map_err(|error| format!("Failed to resolve '{}': {error}", candidate.display()))?;
        dependencies.push(DependencyDraft {
            from: path.clone(),
            to: target.clone(),
            reference,
            kind,
        });
        collect_source(
            &target,
            target_virtual,
            target_editable,
            library_roots,
            sources,
            dependencies,
            visiting,
        )?;
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
        editable: draft.editable,
        content: draft.content,
        global_variables: draft.ast.global_variables,
        module_defines: draft.ast.module_defines,
        function_defines: draft.ast.function_defines,
        modules: draft.ast.modules,
        includes: draft.ast.includes,
        uses: draft.ast.uses,
    }
}

fn resolve_reference(
    caller: &Path,
    caller_virtual: &str,
    reference: &str,
    caller_editable: bool,
    library_roots: &[PathBuf],
) -> Option<(PathBuf, String, bool)> {
    let referenced = Path::new(reference);
    if referenced.is_absolute() {
        let filename = referenced.file_name()?;
        return referenced.is_file().then(|| {
            (
                referenced.to_path_buf(),
                format!("external/{}", filename.to_string_lossy()),
                false,
            )
        });
    }

    let adjacent = caller
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(referenced);
    if adjacent.is_file() {
        return virtual_dependency_path(caller_virtual, referenced)
            .map(|virtual_path| (adjacent, virtual_path, caller_editable));
    }

    library_roots.iter().find_map(|root| {
        let candidate = root.join(referenced);
        candidate.is_file().then(|| {
            portable_path(referenced).map(|virtual_path| (candidate, virtual_path, false))
        })?
    })
}

fn virtual_dependency_path(caller_virtual: &str, reference: &Path) -> Option<String> {
    let mut components = Path::new(caller_virtual)
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for component in reference.components() {
        match component {
            Component::Normal(value) => components.push(value.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                components.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    portable_path(&components.into_iter().collect::<PathBuf>())
}

fn portable_path(path: &Path) -> Option<String> {
    let components = path
        .components()
        .map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            Component::CurDir => None,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => None,
        })
        .collect::<Option<Vec<_>>>()?;
    (!components.is_empty()).then(|| components.join("/"))
}

fn openscad_library_roots() -> Vec<PathBuf> {
    let mut roots = std::env::var_os("OPENSCADPATH")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    if let Some(data) = dirs::data_local_dir() {
        let openscad = data.join("OpenSCAD");
        roots.push(openscad.join("libraries"));
    }
    if let Some(documents) = dirs::document_dir() {
        roots.push(documents.join("OpenSCAD").join("libraries"));
    }
    roots.extend([
        PathBuf::from("/usr/local/share/openscad/libraries"),
        PathBuf::from("/usr/share/openscad/libraries"),
    ]);
    let mut unique = HashSet::new();
    roots.retain(|root| root.is_dir() && unique.insert(root.clone()));
    roots
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
        assert_eq!(project.active_source.as_deref(), Some("main.scad"));
        assert!(project
            .embedded_sources
            .iter()
            .all(|source| source.editable));
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

    #[test]
    fn imports_dependencies_from_openscad_library_roots() {
        let project_directory = tempfile::tempdir().unwrap();
        let library_directory = tempfile::tempdir().unwrap();
        let bosl = library_directory.path().join("BOSL");
        fs::create_dir(&bosl).unwrap();
        fs::write(
            project_directory.path().join("main.scad"),
            "include <BOSL/constants.scad>\nuse <BOSL/transforms.scad>\nright(2) cube(1);",
        )
        .unwrap();
        fs::write(bosl.join("constants.scad"), "RIGHT = [1, 0, 0];").unwrap();
        fs::write(
            bosl.join("transforms.scad"),
            "include <constants.scad>\nmodule right(x=0) translate([x,0,0]) children();",
        )
        .unwrap();

        let project = import_scad_project_with_library_roots(
            &project_directory.path().join("main.scad"),
            &[library_directory.path().to_path_buf()],
        )
        .unwrap();

        assert_eq!(project.entry_source.as_deref(), Some("main.scad"));
        assert!(project
            .embedded_sources
            .iter()
            .any(|source| source.virtual_path == "BOSL/constants.scad"));
        let transforms = project
            .embedded_sources
            .iter()
            .find(|source| source.virtual_path == "BOSL/transforms.scad")
            .unwrap();
        assert!(!transforms.editable);
        assert!(
            project
                .embedded_sources
                .iter()
                .find(|source| source.virtual_path == "main.scad")
                .unwrap()
                .editable
        );
        assert!(transforms
            .module_defines
            .iter()
            .any(|definition| definition.name == "right"));
        assert_eq!(project.source_dependencies.len(), 3);
    }

    #[test]
    fn attaches_multiple_editable_source_trees_without_replacing_the_project() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        fs::write(first.path().join("main.scad"), "cube(1);").unwrap();
        fs::write(second.path().join("main.scad"), "sphere(2);").unwrap();
        let mut project = AstRoot::new_project("main.scad");

        let first_target =
            attach_editable_scad(&mut project, &first.path().join("main.scad")).unwrap();
        let second_target =
            attach_editable_scad(&mut project, &second.path().join("main.scad")).unwrap();

        assert_eq!(first_target, "main.scad");
        assert_ne!(second_target, first_target);
        assert_eq!(
            project.active_source.as_deref(),
            Some(second_target.as_str())
        );
        assert_eq!(
            project
                .embedded_sources
                .iter()
                .filter(|source| source.editable)
                .count(),
            2
        );
        assert!(project
            .source_code(&first_target)
            .unwrap()
            .contains("cube(1);"));
        assert!(project
            .source_code(&second_target)
            .unwrap()
            .contains("sphere(2);"));
    }
}
