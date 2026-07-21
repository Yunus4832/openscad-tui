//! Mesh generation and renderer-independent model preview primitives.

mod camera;
mod error;
mod exporter;
mod framebuffer;
mod importer;
mod mesh;
mod off;
mod openscad;
mod pipeline;
mod rasterizer;
mod service;

pub use camera::{Camera, Projection, StandardView};
pub use error::{RenderError, Result};
pub use exporter::write_dae;
pub use framebuffer::{Framebuffer, PixelSize, RgbaFrame};
pub use glam::{Mat4, Vec2, Vec3, Vec4};
pub use importer::{read_mesh_file, MeshFileFormat};
pub use mesh::{Aabb, Mesh};
pub use off::{parse_off, read_off};
pub use openscad::{
    GenerationDiagnostics, MeshGeneration, OpenScadGenerator, OpenScadProject, OpenScadProjectFile,
};
pub use pipeline::{MeshInput, MeshLoader, MeshPipeline};
pub use rasterizer::{CpuRenderer, RenderSettings};
pub use service::{
    FrameRenderer, RenderEvent, RenderFailureStage, RenderOptions, RenderService, RenderedFrame,
};
