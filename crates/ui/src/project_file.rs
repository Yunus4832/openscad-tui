use openscad_assembly::AssemblyDocument;
use openscad_core::AstRoot;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const PROJECT_EXTENSION: &str = "scadtui";
const FORMAT_NAME: &str = "openscad-tui-project";
const FORMAT_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDocument {
    pub sources: AstRoot,
    #[serde(default)]
    pub assemblies: Vec<AssemblyDocument>,
    #[serde(default)]
    pub active_assembly: Option<String>,
}

impl ProjectDocument {
    pub fn new(sources: AstRoot) -> Self {
        Self {
            sources,
            assemblies: Vec::new(),
            active_assembly: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ProjectManifest {
    format: String,
    format_version: u32,
    generator: Generator,
    project: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Generator {
    name: String,
    version: String,
}

pub fn save_project(path: &Path, project: &ProjectDocument) -> Result<(), String> {
    validate_document(project)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temporary = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    let mut archive = ZipWriter::new(temporary.as_file());
    let manifest = ProjectManifest {
        format: FORMAT_NAME.to_string(),
        format_version: FORMAT_VERSION,
        generator: Generator {
            name: "openscad-tui".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        project: "project.json".to_string(),
    };
    archive
        .start_file("manifest.json", options)
        .map_err(zip_error)?;
    archive
        .write_all(
            serde_json::to_string_pretty(&manifest)
                .map_err(json_error)?
                .as_bytes(),
        )
        .map_err(io_error)?;
    archive
        .start_file("project.json", options)
        .map_err(zip_error)?;
    archive
        .write_all(
            serde_json::to_string_pretty(project)
                .map_err(json_error)?
                .as_bytes(),
        )
        .map_err(io_error)?;
    for source in &project.sources.embedded_sources {
        validate_virtual_path(&source.virtual_path)?;
        archive
            .start_file(format!("sources/{}", source.virtual_path), options)
            .map_err(zip_error)?;
        archive
            .write_all(source.generated_content().as_bytes())
            .map_err(io_error)?;
    }
    archive.finish().map_err(zip_error)?;
    temporary
        .persist(path)
        .map_err(|error| format!("Failed to replace '{}': {}", path.display(), error.error))?;
    Ok(())
}

pub fn load_project(path: &Path) -> Result<ProjectDocument, String> {
    let file = File::open(path).map_err(io_error)?;
    let mut archive = ZipArchive::new(file).map_err(zip_error)?;
    let manifest: ProjectManifest = {
        let mut content = String::new();
        archive
            .by_name("manifest.json")
            .map_err(zip_error)?
            .read_to_string(&mut content)
            .map_err(io_error)?;
        serde_json::from_str(&content).map_err(json_error)?
    };
    if manifest.format != FORMAT_NAME {
        return Err(format!("Unsupported project format '{}'", manifest.format));
    }
    if manifest.format_version != FORMAT_VERSION {
        return Err(format!(
            "Unsupported project format version {}; expected {}",
            manifest.format_version, FORMAT_VERSION
        ));
    }
    validate_archive_path(&manifest.project)?;
    let mut content = String::new();
    archive
        .by_name(&manifest.project)
        .map_err(zip_error)?
        .read_to_string(&mut content)
        .map_err(io_error)?;
    let project: ProjectDocument = serde_json::from_str(&content).map_err(json_error)?;
    validate_document(&project)?;
    Ok(project)
}

fn validate_document(project: &ProjectDocument) -> Result<(), String> {
    let editable_sources = project
        .sources
        .embedded_sources
        .iter()
        .filter(|source| source.editable)
        .map(|source| source.virtual_path.as_str())
        .collect::<HashSet<_>>();
    let mut assembly_ids = HashSet::new();
    for assembly in &project.assemblies {
        if !assembly_ids.insert(assembly.id.as_str()) {
            return Err(format!("Duplicate assembly ID '{}'", assembly.id));
        }
        assembly
            .validate()
            .map_err(|error| format!("Invalid assembly '{}': {error}", assembly.name))?;
        for part in &assembly.parts {
            if !editable_sources.contains(part.source.virtual_path()) {
                return Err(format!(
                    "Assembly '{}' part '{}' references missing editable source '{}'",
                    assembly.name,
                    part.name,
                    part.source.virtual_path()
                ));
            }
        }
    }
    if let Some(active) = &project.active_assembly {
        if !project
            .assemblies
            .iter()
            .any(|assembly| assembly.id == *active)
        {
            return Err(format!("Active assembly '{active}' was not found"));
        }
    }
    Ok(())
}

fn validate_virtual_path(path: &str) -> Result<(), String> {
    validate_archive_path(path)
}

fn validate_archive_path(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!("Unsafe project archive path '{}'", path.display()));
    }
    Ok(())
}

fn io_error(error: std::io::Error) -> String {
    error.to_string()
}

fn zip_error(error: zip::result::ZipError) -> String {
    error.to_string()
}

fn json_error(error: serde_json::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_package_round_trip_contains_manifest_and_sources() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.scadtui");
        let mut sources = AstRoot::new_project("main.scad");
        sources.modules.push(openscad_core::ModuleNode::new_leaf(
            "cube_package".to_string(),
            "cube".to_string(),
            Vec::new(),
        ));
        sources.sync_active_source();
        let mut project = ProjectDocument::new(sources);
        let mut assembly = AssemblyDocument::new("fixture");
        assembly
            .add_part(
                openscad_assembly::MeshSourceRef::project_source("main.scad"),
                "body",
            )
            .unwrap();
        assembly.part_mut("body").unwrap().transform.translation = [1.0, 2.0, 3.0];
        project.assemblies.push(assembly);
        project.active_assembly = Some(project.assemblies[0].id.clone());

        save_project(&path, &project).unwrap();
        save_project(&path, &project).unwrap();
        let restored = load_project(&path).unwrap();
        assert_eq!(restored.sources.active_source.as_deref(), Some("main.scad"));
        assert!(restored
            .sources
            .source_code("main.scad")
            .unwrap()
            .contains("cube();"));
        assert_eq!(restored.assemblies[0].name, "fixture");
        assert_eq!(
            restored.assemblies[0]
                .part("body")
                .unwrap()
                .transform
                .translation,
            [1.0, 2.0, 3.0]
        );
        assert_eq!(
            restored.active_assembly,
            Some(restored.assemblies[0].id.clone())
        );

        let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
        assert!(archive.by_name("manifest.json").is_ok());
        assert!(archive.by_name("project.json").is_ok());
        assert!(archive.by_name("sources/main.scad").is_ok());
    }

    #[test]
    fn project_validation_rejects_missing_assembly_sources_and_active_ids() {
        let mut project = ProjectDocument::new(AstRoot::new_project("main.scad"));
        let mut assembly = AssemblyDocument::new("broken");
        assembly
            .add_part(
                openscad_assembly::MeshSourceRef::project_source("missing.scad"),
                "missing",
            )
            .unwrap();
        project.assemblies.push(assembly);
        assert!(validate_document(&project)
            .unwrap_err()
            .contains("missing editable source"));

        project.assemblies[0].parts.clear();
        project.active_assembly = Some("unknown".into());
        assert!(validate_document(&project)
            .unwrap_err()
            .contains("Active assembly 'unknown'"));
    }

    #[test]
    fn project_validation_rejects_duplicate_assembly_part_names() {
        let mut project = ProjectDocument::new(AstRoot::new_project("main.scad"));
        let mut assembly = AssemblyDocument::new("invalid names");
        let source = openscad_assembly::MeshSourceRef::project_source("main.scad");
        assembly.add_part(source.clone(), "arm").unwrap();
        assembly.add_part(source, "leg").unwrap();
        assembly.parts[1].name = "arm".into();
        project.assemblies.push(assembly);

        let error = validate_document(&project).unwrap_err();

        assert!(error.contains("duplicate part name 'arm'"));
    }
}
