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

    #[error("invalid OFF data: {0}")]
    InvalidOff(String),

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

    #[error("no generated mesh is available for camera rendering")]
    NoCachedMesh,

    #[error("render worker disconnected")]
    WorkerDisconnected,
}

pub type Result<T> = std::result::Result<T, RenderError>;
