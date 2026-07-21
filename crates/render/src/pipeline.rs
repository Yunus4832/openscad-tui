use std::path::PathBuf;
use std::time::Instant;

use crate::{read_mesh_file, GenerationDiagnostics, MeshGeneration, OpenScadGenerator, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshInput {
    OpenScad(String),
    File(PathBuf),
}

pub trait MeshLoader: Send + Sync {
    fn load(&self, input: &MeshInput) -> Result<MeshGeneration>;
}

#[derive(Debug, Clone)]
pub struct MeshPipeline {
    openscad: OpenScadGenerator,
}

impl MeshPipeline {
    pub fn new(openscad: OpenScadGenerator) -> Self {
        Self { openscad }
    }
}

impl MeshLoader for MeshPipeline {
    fn load(&self, input: &MeshInput) -> Result<MeshGeneration> {
        match input {
            MeshInput::OpenScad(source) => self.openscad.generate(source),
            MeshInput::File(path) => {
                let started = Instant::now();
                let mesh = read_mesh_file(path)?;
                Ok(MeshGeneration {
                    mesh,
                    diagnostics: GenerationDiagnostics {
                        stdout: String::new(),
                        stderr: String::new(),
                        elapsed: started.elapsed(),
                    },
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_inputs_do_not_invoke_openscad() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("triangle.off");
        std::fs::write(&path, "OFF\n3 1 0\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n").unwrap();
        let pipeline = MeshPipeline::new(OpenScadGenerator::new("missing-openscad"));

        let loaded = pipeline.load(&MeshInput::File(path)).unwrap();

        assert_eq!(loaded.mesh.triangle_count(), 1);
    }
}
