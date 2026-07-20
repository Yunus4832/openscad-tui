//! Mesh generation and renderer-independent model preview primitives.

mod camera;
mod error;
mod framebuffer;
mod mesh;
mod off;
mod openscad;
mod rasterizer;
mod service;

pub use camera::{Camera, Projection, StandardView};
pub use error::{RenderError, Result};
pub use framebuffer::{Framebuffer, PixelSize, RgbaFrame};
pub use glam::{Mat4, Vec2, Vec3, Vec4};
pub use mesh::{Aabb, Mesh};
pub use off::{parse_off, read_off};
pub use openscad::{
    GenerationDiagnostics, MeshGeneration, MeshGenerator, OpenScadGenerator, OpenScadProject,
    OpenScadProjectFile,
};
pub use rasterizer::{CpuRenderer, RenderSettings};
pub use service::{FrameRenderer, RenderEvent, RenderFailureStage, RenderService, RenderedFrame};
