use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::Builder;
use wait_timeout::ChildExt;

use crate::{parse_off, Mesh, RenderError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationDiagnostics {
    pub stdout: String,
    pub stderr: String,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeshGeneration {
    pub mesh: Mesh,
    pub diagnostics: GenerationDiagnostics,
}

pub trait MeshGenerator: Send + Sync {
    fn generate(&self, scad_source: &str) -> Result<MeshGeneration>;
}

#[derive(Debug, Clone)]
pub struct OpenScadGenerator {
    executable: PathBuf,
    working_directory: Option<PathBuf>,
    timeout: Duration,
}

impl OpenScadGenerator {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            working_directory: None,
            timeout: Duration::from_secs(120),
        }
    }

    pub fn with_working_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(directory.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn working_directory(&self) -> Option<&Path> {
        self.working_directory.as_deref()
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl MeshGenerator for OpenScadGenerator {
    fn generate(&self, scad_source: &str) -> Result<MeshGeneration> {
        let fallback_directory;
        let directory = match &self.working_directory {
            Some(directory) => directory.as_path(),
            None => {
                fallback_directory = tempfile::tempdir().map_err(io_error)?;
                fallback_directory.path()
            }
        };

        let mut source = Builder::new()
            .prefix("openscad-tui-")
            .suffix(".scad")
            .tempfile_in(directory)
            .map_err(io_error)?;
        source.write_all(scad_source.as_bytes()).map_err(io_error)?;
        source.flush().map_err(io_error)?;
        let output = Builder::new()
            .prefix("openscad-tui-")
            .suffix(".off")
            .tempfile_in(directory)
            .map_err(io_error)?;
        let stdout_path = output.path().with_extension("stdout.log");
        let stderr_path = output.path().with_extension("stderr.log");
        let stdout_file = File::create(&stdout_path).map_err(io_error)?;
        let stderr_file = File::create(&stderr_path).map_err(io_error)?;

        let started = Instant::now();
        let mut child = Command::new(&self.executable)
            .current_dir(directory)
            .arg("-o")
            .arg(output.path())
            .arg(source.path())
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file))
            .spawn()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    RenderError::OpenScadNotFound(self.executable.display().to_string())
                } else {
                    io_error(error)
                }
            })?;

        let status = match child.wait_timeout(self.timeout).map_err(io_error)? {
            Some(status) => status,
            None => {
                child.kill().map_err(io_error)?;
                let _ = child.wait();
                cleanup_log(&stdout_path);
                cleanup_log(&stderr_path);
                return Err(RenderError::OpenScadTimeout {
                    milliseconds: self.timeout.as_millis(),
                });
            }
        };
        let elapsed = started.elapsed();
        let stdout = fs::read_to_string(&stdout_path).unwrap_or_default();
        let stderr = fs::read_to_string(&stderr_path).unwrap_or_default();
        cleanup_log(&stdout_path);
        cleanup_log(&stderr_path);
        if !status.success() {
            return Err(RenderError::OpenScadFailed {
                exit_code: status.code(),
                stderr,
            });
        }

        let off_source = fs::read_to_string(output.path()).map_err(io_error)?;
        let mesh = parse_off(&off_source)?;
        Ok(MeshGeneration {
            mesh,
            diagnostics: GenerationDiagnostics {
                stdout,
                stderr,
                elapsed,
            },
        })
    }
}

fn io_error(error: std::io::Error) -> RenderError {
    RenderError::Io(error.to_string())
}

fn cleanup_log(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_executable_has_a_specific_error() {
        let generator = OpenScadGenerator::new("definitely-not-an-openscad-executable");
        assert!(matches!(
            generator.generate("cube(1);"),
            Err(RenderError::OpenScadNotFound(_))
        ));
    }

    #[cfg(unix)]
    fn executable_script(contents: &str) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fake-openscad");
        let mut script = File::create(&path).unwrap();
        script.write_all(contents.as_bytes()).unwrap();
        script.sync_all().unwrap();
        drop(script);
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        File::open(directory.path()).unwrap().sync_all().unwrap();
        (directory, path)
    }

    #[cfg(unix)]
    #[test]
    fn parses_off_written_by_a_fake_executable() {
        let (directory, executable) = executable_script(
            "#!/bin/sh\nprintf 'OFF\\n3 1 0\\n0 0 0\\n1 0 0\\n0 1 0\\n3 0 1 2\\n' > \"$2\"\necho generated >&2\n",
        );
        let generation = OpenScadGenerator::new(executable)
            .with_working_directory(directory.path())
            .generate("cube(1);")
            .unwrap();
        assert_eq!(generation.mesh.triangle_count(), 1);
        assert!(generation.diagnostics.stderr.contains("generated"));
        assert!(directory
            .path()
            .read_dir()
            .unwrap()
            .all(|entry| entry.unwrap().file_name() == "fake-openscad"));
    }

    #[cfg(unix)]
    #[test]
    fn reports_failure_and_timeout() {
        let (failure_directory, failure_executable) =
            executable_script("#!/bin/sh\necho broken >&2\nexit 7\n");
        let failure = OpenScadGenerator::new(failure_executable)
            .with_working_directory(failure_directory.path())
            .generate("cube(1);");
        assert!(
            matches!(
                failure,
                Err(RenderError::OpenScadFailed {
                    exit_code: Some(7),
                    ..
                })
            ),
            "unexpected failure result: {failure:?}"
        );

        let (timeout_directory, timeout_executable) = executable_script("#!/bin/sh\nsleep 1\n");
        assert!(matches!(
            OpenScadGenerator::new(timeout_executable)
                .with_working_directory(timeout_directory.path())
                .with_timeout(Duration::from_millis(10))
                .generate("cube(1);"),
            Err(RenderError::OpenScadTimeout { .. })
        ));
    }

    #[test]
    #[ignore = "requires a local OpenSCAD executable"]
    fn exports_with_local_openscad() {
        let generation = OpenScadGenerator::new("openscad")
            .with_timeout(Duration::from_secs(30))
            .generate("cube(1);")
            .unwrap();
        assert!(generation.mesh.triangle_count() >= 12);
    }
}
