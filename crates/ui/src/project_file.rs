use openscad_core::AstRoot;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const PROJECT_EXTENSION: &str = "scadtui";
const FORMAT_NAME: &str = "openscad-tui-project";
const FORMAT_VERSION: u32 = 1;

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

pub fn save_project(path: &Path, project: &AstRoot) -> Result<(), String> {
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
    for source in &project.embedded_sources {
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

pub fn load_project(path: &Path) -> Result<AstRoot, String> {
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
    serde_json::from_str(&content).map_err(json_error)
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
        let mut project = AstRoot::new_project("main.scad");
        project.modules.push(openscad_core::ModuleNode::new_leaf(
            "cube_package".to_string(),
            "cube".to_string(),
            Vec::new(),
        ));
        project.sync_active_source();

        save_project(&path, &project).unwrap();
        save_project(&path, &project).unwrap();
        let restored = load_project(&path).unwrap();
        assert_eq!(restored.active_source.as_deref(), Some("main.scad"));
        assert!(restored
            .source_code("main.scad")
            .unwrap()
            .contains("cube();"));

        let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
        assert!(archive.by_name("manifest.json").is_ok());
        assert!(archive.by_name("project.json").is_ok());
        assert!(archive.by_name("sources/main.scad").is_ok());
    }
}
