use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use image::{DynamicImage, RgbaImage};
use openscad_render::{
    Aabb, Camera, CpuRenderer, OpenScadGenerator, PixelSize, Projection, RenderEvent,
    RenderService, StandardView,
};
use ratatui::layout::Rect;
use ratatui_image::{picker::Picker, protocol::StatefulProtocol};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewMode {
    Source,
    Model,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPreviewStatus {
    Empty,
    Stale,
    Generating,
    Rasterizing,
    Ready { triangles: usize },
    Failed(String),
}

pub struct ModelPreview {
    pub mode: PreviewMode,
    pub status: ModelPreviewStatus,
    pub camera: Camera,
    picker: Picker,
    protocol: Option<StatefulProtocol>,
    service: Option<RenderService>,
    viewport: PixelSize,
    mesh_revision: u64,
    camera_revision: u64,
    bounds: Option<Aabb>,
    fitted_revision: Option<u64>,
    pub auto_rotate: bool,
    last_animation_tick: Instant,
}

impl Default for ModelPreview {
    fn default() -> Self {
        Self::new(Picker::from_fontsize((10, 20)))
    }
}

impl ModelPreview {
    pub fn new(picker: Picker) -> Self {
        Self {
            mode: PreviewMode::Source,
            status: ModelPreviewStatus::Empty,
            camera: Camera::default(),
            picker,
            protocol: None,
            service: None,
            viewport: PixelSize::new(640, 480).expect("constant viewport is valid"),
            mesh_revision: 0,
            camera_revision: 0,
            bounds: None,
            fitted_revision: None,
            auto_rotate: false,
            last_animation_tick: Instant::now(),
        }
    }

    pub fn set_picker(&mut self, picker: Picker) {
        self.picker = picker;
        self.protocol = None;
    }

    pub fn protocol_type(&self) -> ratatui_image::picker::ProtocolType {
        self.picker.protocol_type()
    }

    pub fn protocol_mut(&mut self) -> Option<&mut StatefulProtocol> {
        self.protocol.as_mut()
    }

    pub fn set_area(&mut self, area: Rect) {
        let width = u32::from(area.width.saturating_sub(2)) * u32::from(self.picker.font_size().0);
        let height =
            u32::from(area.height.saturating_sub(2)) * u32::from(self.picker.font_size().1);
        let Ok(size) = PixelSize::new(width, height) else {
            return;
        };
        if size == self.viewport {
            return;
        }
        self.viewport = size;
        if self.bounds.is_some() && self.service.is_some() {
            self.request_rasterize();
        }
    }

    pub fn render(&mut self, source: String, project_file: Option<&str>) -> Result<(), String> {
        self.mesh_revision = self.mesh_revision.wrapping_add(1);
        self.camera_revision = self.camera_revision.wrapping_add(1);
        self.bounds = None;
        self.fitted_revision = None;
        self.protocol = None;
        self.mode = PreviewMode::Model;
        self.status = ModelPreviewStatus::Generating;

        let working_directory = project_file
            .and_then(|file| Path::new(file).parent())
            .filter(|path| !path.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok());
        let mut generator =
            OpenScadGenerator::new("openscad").with_timeout(Duration::from_secs(120));
        if let Some(directory) = working_directory {
            generator = generator.with_working_directory(expand_tilde(directory));
        }
        let service = RenderService::new(Box::new(generator), Box::new(CpuRenderer::default()));
        service
            .generate(
                self.mesh_revision,
                self.camera_revision,
                source,
                self.camera,
                self.viewport,
            )
            .map_err(|error| error.to_string())?;
        self.service = Some(service);
        Ok(())
    }

    pub fn mark_stale(&mut self) {
        if !matches!(self.status, ModelPreviewStatus::Empty) {
            self.mesh_revision = self.mesh_revision.wrapping_add(1);
            self.status = ModelPreviewStatus::Stale;
        }
    }

    pub fn poll(&mut self) {
        while let Some(event) = self.service.as_ref().and_then(RenderService::try_recv) {
            match event {
                RenderEvent::Generating { mesh_revision }
                    if mesh_revision == self.mesh_revision =>
                {
                    self.status = ModelPreviewStatus::Generating;
                }
                RenderEvent::Rasterizing {
                    mesh_revision,
                    camera_revision,
                } if mesh_revision == self.mesh_revision
                    && camera_revision == self.camera_revision =>
                {
                    self.status = ModelPreviewStatus::Rasterizing;
                }
                RenderEvent::Ready(rendered)
                    if rendered.mesh_revision == self.mesh_revision
                        && rendered.camera_revision == self.camera_revision =>
                {
                    self.bounds = Some(rendered.bounds);
                    let size = rendered.frame.size();
                    if let Some(image) = RgbaImage::from_raw(
                        size.width,
                        size.height,
                        rendered.frame.pixels().to_vec(),
                    ) {
                        self.protocol = Some(
                            self.picker
                                .new_resize_protocol(DynamicImage::ImageRgba8(image)),
                        );
                        self.status = ModelPreviewStatus::Ready {
                            triangles: rendered.triangle_count,
                        };
                    } else {
                        self.status = ModelPreviewStatus::Failed(
                            "renderer returned an invalid RGBA buffer".to_string(),
                        );
                    }
                    if self.fitted_revision != Some(self.mesh_revision) {
                        self.camera
                            .fit(rendered.bounds, self.viewport.aspect_ratio());
                        self.fitted_revision = Some(self.mesh_revision);
                        self.request_rasterize();
                    }
                }
                RenderEvent::Failed {
                    mesh_revision,
                    camera_revision,
                    error,
                    ..
                } if mesh_revision == self.mesh_revision
                    && camera_revision == self.camera_revision =>
                {
                    self.status = ModelPreviewStatus::Failed(error.to_string());
                }
                _ => {}
            }
        }
    }

    pub fn tick(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.last_animation_tick);
        self.last_animation_tick = now;
        // Keep at most one camera frame in flight. If every UI tick submitted a newer
        // revision, a renderer slower than the tick rate would only ever finish stale
        // frames and the preview would appear frozen.
        if self.auto_rotate
            && self.bounds.is_some()
            && matches!(self.status, ModelPreviewStatus::Ready { .. })
            && elapsed <= Duration::from_secs(1)
        {
            self.camera.orbit(elapsed.as_secs_f32() * 0.6, 0.0);
            self.request_rasterize();
        }
    }

    pub fn set_projection(&mut self, orthographic: bool) -> Result<(), String> {
        let bounds = self.require_bounds()?;
        let projection = if orthographic {
            Projection::Orthographic { vertical_size: 2.0 }
        } else {
            Projection::Perspective {
                fov_y_radians: 45.0_f32.to_radians(),
            }
        };
        self.camera
            .set_projection(projection, bounds, self.viewport.aspect_ratio());
        self.request_rasterize();
        Ok(())
    }

    pub fn set_view(&mut self, view: StandardView) -> Result<(), String> {
        let bounds = self.require_bounds()?;
        self.camera
            .set_standard_view(view, bounds, self.viewport.aspect_ratio());
        self.request_rasterize();
        Ok(())
    }

    pub fn orbit(&mut self, yaw_degrees: f32, pitch_degrees: f32) -> Result<(), String> {
        self.require_bounds()?;
        self.camera
            .orbit(yaw_degrees.to_radians(), pitch_degrees.to_radians());
        self.request_rasterize();
        Ok(())
    }

    pub fn pan(&mut self, horizontal: f32, vertical: f32) -> Result<(), String> {
        self.require_bounds()?;
        self.camera.pan_fraction(horizontal, vertical);
        self.request_rasterize();
        Ok(())
    }

    pub fn zoom(&mut self, factor: f32) -> Result<(), String> {
        self.require_bounds()?;
        self.camera.zoom(factor);
        self.request_rasterize();
        Ok(())
    }

    pub fn fit(&mut self) -> Result<(), String> {
        let bounds = self.require_bounds()?;
        self.camera.fit(bounds, self.viewport.aspect_ratio());
        self.request_rasterize();
        Ok(())
    }

    fn require_bounds(&self) -> Result<Aabb, String> {
        self.bounds
            .ok_or_else(|| "render a model before changing the camera".to_string())
    }

    fn request_rasterize(&mut self) {
        let Some(service) = &self.service else {
            return;
        };
        self.camera_revision = self.camera_revision.wrapping_add(1);
        self.status = ModelPreviewStatus::Rasterizing;
        if let Err(error) = service.rasterize(self.camera_revision, self.camera, self.viewport) {
            self.status = ModelPreviewStatus::Failed(error.to_string());
        }
    }
}

fn expand_tilde(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(home) = dirs::home_dir() {
        if text == "~" {
            return home;
        }
        if let Some(relative) = text.strip_prefix("~/") {
            return home.join(relative);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_marks_an_existing_preview_stale() {
        let mut preview = ModelPreview::default();
        preview.status = ModelPreviewStatus::Ready { triangles: 12 };
        preview.mark_stale();
        assert_eq!(preview.status, ModelPreviewStatus::Stale);
    }

    #[test]
    fn camera_requires_a_generated_mesh() {
        let mut preview = ModelPreview::default();
        assert!(preview.zoom(0.8).is_err());
    }

    #[test]
    fn auto_rotation_waits_for_the_previous_frame() {
        let mut preview = ModelPreview::default();
        preview.auto_rotate = true;
        preview.bounds = Some(Aabb {
            min: openscad_render::Vec3::splat(-1.0),
            max: openscad_render::Vec3::splat(1.0),
        });
        preview.status = ModelPreviewStatus::Rasterizing;
        let yaw = preview.camera.yaw;
        preview.tick(preview.last_animation_tick + Duration::from_millis(33));
        assert_eq!(preview.camera.yaw, yaw);

        preview.status = ModelPreviewStatus::Ready { triangles: 12 };
        preview.tick(preview.last_animation_tick + Duration::from_millis(33));
        assert_ne!(preview.camera.yaw, yaw);
    }

    #[test]
    fn expands_home_directory_as_a_complete_path_component() {
        let home = dirs::home_dir().expect("test requires a home directory");
        assert_eq!(expand_tilde(PathBuf::from("~")), home);
        assert_eq!(expand_tilde(PathBuf::from("~/models")), home.join("models"));
    }
}
