use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RenderError {
    #[error("pixel dimensions must be non-zero, got {width}x{height}")]
    InvalidPixelSize { width: u32, height: u32 },

    #[error("pixel buffer size overflow for {width}x{height}")]
    PixelBufferOverflow { width: u32, height: u32 },

    #[error("mesh contains no finite vertices")]
    EmptyMesh,

    #[error("mesh vertex {index} contains a non-finite coordinate")]
    NonFiniteVertex { index: usize },

    #[error("triangle {triangle} references vertex {index}, but only {vertex_count} exist")]
    InvalidTriangleIndex {
        triangle: usize,
        index: u32,
        vertex_count: usize,
    },

    #[error("render instance references mesh {index}, but only {mesh_count} meshes exist")]
    InvalidMeshInstance { index: usize, mesh_count: usize },

    #[error("render instance {index} contains a non-finite transform")]
    NonFiniteTransform { index: usize },

    #[error("invalid OFF data: {0}")]
    InvalidOff(String),

    #[error("invalid STL data: {0}")]
    InvalidStl(String),

    #[error("unsupported mesh file format for '{path}'; expected .off or .stl")]
    UnsupportedMeshFormat { path: String },

    #[error("I/O error: {0}")]
    Io(String),

    #[error("OpenSCAD executable not found: {0}")]
    OpenScadNotFound(String),

    #[error("OpenSCAD timed out after {milliseconds} ms")]
    OpenScadTimeout { milliseconds: u128 },

    #[error("OpenSCAD failed with exit code {exit_code:?}: {stderr}")]
    OpenScadFailed {
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error("OpenSCAD cannot export the top-level 2D object as a 3D OFF mesh: {stderr}")]
    OpenScadNon3d { stderr: String },

    #[error("no generated mesh is available for camera rendering")]
    NoCachedMesh,

    #[error("render worker disconnected")]
    WorkerDisconnected,
}

impl RenderError {
    /// A compact, actionable message suitable for a status line.
    pub fn summary(&self) -> String {
        match self {
            Self::OpenScadNon3d { .. } => {
                "2D object: extrude before render; :diagnostics for details".to_string()
            }
            Self::OpenScadFailed { exit_code, .. } => {
                format!("OpenSCAD failed (exit code {exit_code:?}); run :diagnostics")
            }
            Self::InvalidOff(_) | Self::InvalidStl(_) => {
                format!("could not parse model: {self}; run :diagnostics")
            }
            _ => self.to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, RenderError>;

#[cfg(test)]
mod tests {
    use super::RenderError;

    #[test]
    fn non_3d_summary_is_actionable_without_flattening_diagnostics() {
        let error = RenderError::OpenScadNon3d {
            stderr: "Current top level object is not a 3D object.".into(),
        };
        assert!(error.summary().contains("extrude"));
        assert!(error.to_string().contains("Current top level object"));
    }
}
