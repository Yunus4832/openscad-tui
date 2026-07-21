use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{fs::OpenOptions, io::Write};

use openscad_render::{
    Aabb, Camera, CpuRenderer, OpenScadGenerator, OpenScadProject, PixelSize, Projection,
    RenderEvent, RenderOptions, RenderService, StandardView,
};
use openscad_terminal::{DisplayProtocol, PresentationContext, TerminalImage, TerminalPresenter};
use ratatui::layout::Rect;
use ratatui_image::picker::{Picker, ProtocolType};

const AUTO_ROTATE_FRAME_INTERVAL: Duration = Duration::from_millis(33);

#[derive(Debug, Clone, Default)]
pub struct RenderMetrics {
    pub generation_time: Duration,
    pub raster_time: Duration,
    pub encode_time: Duration,
    pub ui_draw_time: Duration,
    pub encoded_bytes: usize,
    pub encoded_bytes_estimated: bool,
    pub presented_fps: f32,
    pub frame_size: Option<PixelSize>,
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
    pub status: ModelPreviewStatus,
    pub camera: Camera,
    auto_protocol: DisplayProtocol,
    presenter: TerminalPresenter,
    service: Option<RenderService>,
    viewport: PixelSize,
    mesh_revision: u64,
    camera_revision: u64,
    bounds: Option<Aabb>,
    fitted_revision: Option<u64>,
    pub auto_rotate: bool,
    pub axes_visible: bool,
    pub metrics: RenderMetrics,
    last_animation_tick: Instant,
    last_presented_at: Option<Instant>,
    last_drawn_sequence: u64,
    presentation_error: Option<String>,
    last_error_details: Option<String>,
}

impl Default for ModelPreview {
    fn default() -> Self {
        Self::new(Picker::from_fontsize((10, 20)))
    }
}

impl ModelPreview {
    pub fn new(picker: Picker) -> Self {
        let is_tmux = picker_is_tmux();
        let auto_protocol = display_protocol(picker.protocol_type());
        let presenter = TerminalPresenter::new(
            auto_protocol,
            PresentationContext {
                cells: Rect::new(0, 0, 64, 24),
                font_size: picker.font_size(),
                is_tmux,
                kitty_image_id: next_kitty_image_id(),
            },
        );
        Self {
            status: ModelPreviewStatus::Empty,
            camera: Camera::default(),
            auto_protocol,
            presenter,
            service: None,
            viewport: PixelSize::new(640, 480).expect("constant viewport is valid"),
            mesh_revision: 0,
            camera_revision: 0,
            bounds: None,
            fitted_revision: None,
            auto_rotate: false,
            axes_visible: true,
            metrics: RenderMetrics::default(),
            last_animation_tick: Instant::now(),
            last_presented_at: None,
            last_drawn_sequence: 0,
            presentation_error: None,
            last_error_details: None,
        }
    }

    pub fn set_picker(&mut self, picker: Picker) {
        render_trace(|| format!("picker-set protocol={:?}", picker.protocol_type()));
        self.auto_protocol = display_protocol(picker.protocol_type());
        self.presenter.set_font_size(picker.font_size());
        self.presenter.set_tmux(picker_is_tmux());
        self.presenter.set_protocol(self.auto_protocol);
        self.presentation_error = None;
        if let Some(size) = self.presenter.target_size() {
            self.viewport = size;
        }
    }

    pub fn protocol_type(&self) -> DisplayProtocol {
        self.presenter.protocol()
    }

    pub fn presentation_error(&self) -> Option<&str> {
        self.presentation_error.as_deref()
    }

    pub fn diagnostics(&self) -> Option<&str> {
        self.presentation_error
            .as_deref()
            .or(self.last_error_details.as_deref())
    }

    pub fn set_protocol_type(&mut self, protocol: DisplayProtocol) {
        self.presenter.set_protocol(protocol);
        self.presentation_error = None;
        if let Some(size) = self.presenter.target_size() {
            self.viewport = size;
        }
    }

    pub fn reset_protocol_type(&mut self) {
        self.presenter.set_protocol(self.auto_protocol);
        self.presentation_error = None;
        if let Some(size) = self.presenter.target_size() {
            self.viewport = size;
        }
    }

    pub(crate) fn image_widget(&mut self) -> Option<TerminalImage<'_>> {
        let sequence = self.presenter.presented_sequence();
        if sequence != 0 && self.last_drawn_sequence != sequence {
            self.last_drawn_sequence = sequence;
            self.record_presented_frame();
        }
        self.presenter.image()
    }

    pub fn prepare_for_display(&mut self) {
        // A terminal clear removes the graphics layer. Rebuilding the fixed protocol forces the
        // cached image to be transmitted again without rerunning OpenSCAD.
        self.presenter.reencode_cached();
    }

    pub fn set_area(&mut self, area: Rect) {
        let cells = Rect::new(
            0,
            0,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        self.presenter.set_cells(cells);
        let Some(size) = self.presenter.target_size() else {
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

    pub fn render(
        &mut self,
        source: String,
        project_file: Option<&str>,
        project: Option<OpenScadProject>,
    ) -> Result<(), String> {
        self.mesh_revision = self.mesh_revision.wrapping_add(1);
        self.camera_revision = self.camera_revision.wrapping_add(1);
        self.bounds = None;
        self.fitted_revision = None;
        self.presenter.clear();
        self.status = ModelPreviewStatus::Generating;
        self.last_error_details = None;

        let working_directory = project_file
            .and_then(|file| Path::new(file).parent())
            .filter(|path| !path.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok());
        let mut generator =
            OpenScadGenerator::new("openscad").with_timeout(Duration::from_secs(120));
        if let Some(project) = project {
            generator = generator.with_project(project);
        }
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
                self.render_options(),
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
        let presentation = self.presenter.poll();
        self.handle_presentation_result(presentation);
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
                RenderEvent::Ready(rendered) if rendered.mesh_revision == self.mesh_revision => {
                    let is_latest = rendered.camera_revision == self.camera_revision;
                    render_trace(|| {
                        format!(
                            "raster-ready mesh_rev={} camera_rev={} expected_rev={} latest={} yaw={:.6} pixels={:016x}",
                            rendered.mesh_revision,
                            rendered.camera_revision,
                            self.camera_revision,
                            is_latest,
                            self.camera.yaw,
                            byte_checksum(rendered.frame.pixels())
                        )
                    });
                    self.metrics.generation_time = rendered.generation_time;
                    self.metrics.raster_time = rendered.raster_time;
                    self.metrics.frame_size = Some(rendered.frame.size());
                    self.bounds = Some(rendered.bounds);
                    self.presenter.submit(Arc::new(rendered.frame));
                    if is_latest {
                        self.status = ModelPreviewStatus::Ready {
                            triangles: rendered.triangle_count,
                        };
                    }
                    if is_latest && self.fitted_revision != Some(self.mesh_revision) {
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
                    self.last_error_details = Some(error.to_string());
                    self.status = ModelPreviewStatus::Failed(error.summary());
                }
                _ => {}
            }
        }
    }

    fn handle_presentation_result(
        &mut self,
        result: Result<Option<openscad_terminal::PresentationUpdate>, String>,
    ) {
        match result {
            Ok(Some(update)) => {
                self.metrics.encode_time = update.encode_time;
                self.metrics.encoded_bytes = update.encoded_bytes;
                self.metrics.encoded_bytes_estimated = update.encoded_bytes_estimated;
                self.presentation_error = None;
            }
            Ok(None) => {}
            Err(error) => self.presentation_error = Some(error),
        }
    }

    pub fn tick(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.last_animation_tick);
        // Keep at most one camera frame in flight. If every UI tick submitted a newer
        // revision, a renderer slower than the tick rate would only ever finish stale
        // frames and the preview would appear frozen.
        if self.auto_rotate
            && self.bounds.is_some()
            && matches!(self.status, ModelPreviewStatus::Ready { .. })
            && elapsed >= AUTO_ROTATE_FRAME_INTERVAL
        {
            let animation_elapsed = elapsed.min(Duration::from_secs(1));
            self.last_animation_tick = now;
            self.camera
                .orbit(animation_elapsed.as_secs_f32() * 0.6, 0.0);
            self.camera
                .update_clip_planes(self.bounds.expect("bounds checked above"));
            render_trace(|| {
                format!(
                    "auto-tick elapsed_ms={:.2} yaw={:.6} next_camera_rev={}",
                    animation_elapsed.as_secs_f64() * 1000.0,
                    self.camera.yaw,
                    self.camera_revision.wrapping_add(1)
                )
            });
            self.request_rasterize();
        }
    }

    pub fn set_auto_rotate(&mut self, enabled: bool) {
        render_trace(|| {
            format!(
                "auto-set enabled={} yaw={:.6} camera_rev={} status={:?}",
                enabled, self.camera.yaw, self.camera_revision, self.status
            )
        });
        self.auto_rotate = enabled;
        self.last_animation_tick = Instant::now();
    }

    pub fn set_axes_visible(&mut self, visible: bool) {
        if self.axes_visible == visible {
            return;
        }
        self.axes_visible = visible;
        if self.bounds.is_some() && self.service.is_some() {
            self.request_rasterize();
        }
    }

    pub fn stop_auto_rotate(&mut self) {
        render_trace(|| {
            format!(
                "auto-stop yaw={:.6} camera_rev={}",
                self.camera.yaw, self.camera_revision
            )
        });
        self.auto_rotate = false;
        self.last_animation_tick = Instant::now();
    }

    pub fn record_ui_draw(&mut self, elapsed: Duration) {
        self.metrics.ui_draw_time = elapsed;
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
        let bounds = self.require_bounds()?;
        self.camera
            .orbit(yaw_degrees.to_radians(), pitch_degrees.to_radians());
        self.camera.update_clip_planes(bounds);
        self.request_rasterize();
        Ok(())
    }

    pub fn pan(&mut self, horizontal: f32, vertical: f32) -> Result<(), String> {
        let bounds = self.require_bounds()?;
        self.camera.pan_fraction(horizontal, vertical);
        self.camera.update_clip_planes(bounds);
        self.request_rasterize();
        Ok(())
    }

    pub fn zoom(&mut self, factor: f32) -> Result<(), String> {
        let bounds = self.require_bounds()?;
        self.camera.zoom(factor);
        self.camera.update_clip_planes(bounds);
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
        self.request_rasterize_at(self.viewport);
    }

    fn request_rasterize_at(&mut self, size: PixelSize) {
        let Some(service) = &self.service else {
            return;
        };
        self.camera_revision = self.camera_revision.wrapping_add(1);
        self.status = ModelPreviewStatus::Rasterizing;
        if let Err(error) = service.rasterize(
            self.camera_revision,
            self.camera,
            size,
            self.render_options(),
        ) {
            self.last_error_details = Some(error.to_string());
            self.status = ModelPreviewStatus::Failed(error.summary());
        }
    }

    fn render_options(&self) -> RenderOptions {
        RenderOptions {
            axes: self.axes_visible,
        }
    }

    fn record_presented_frame(&mut self) {
        let now = Instant::now();
        if let Some(previous) = self.last_presented_at.replace(now) {
            let seconds = now.duration_since(previous).as_secs_f32();
            if seconds > 0.0 {
                self.metrics.presented_fps = 1.0 / seconds;
            }
        }
    }
}

fn picker_is_tmux() -> bool {
    std::env::var_os("TMUX").is_some()
}

fn display_protocol(protocol: ProtocolType) -> DisplayProtocol {
    match protocol {
        ProtocolType::Kitty => DisplayProtocol::Kitty,
        ProtocolType::Sixel => DisplayProtocol::Sixel,
        ProtocolType::Iterm2 => DisplayProtocol::Iterm2,
        ProtocolType::Halfblocks => DisplayProtocol::Halfblocks,
    }
}

fn next_kitty_image_id() -> u32 {
    static NEXT_ID: AtomicU32 = AtomicU32::new(1);
    std::process::id().rotate_left(16) ^ NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn byte_checksum(bytes: &[u8]) -> u64 {
    let step = (bytes.len() / 2048).max(1);
    bytes
        .iter()
        .step_by(step)
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

fn render_trace(message: impl FnOnce() -> String) {
    static TRACE_FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    let Some(file) = TRACE_FILE
        .get_or_init(|| {
            let path = std::env::var_os("OPENSCAD_TUI_RENDER_TRACE")?;
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
                .map(Mutex::new)
        })
        .as_ref()
    else {
        return;
    };
    let message = message();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("unnamed");
    if let Ok(mut file) = file.lock() {
        let _ = writeln!(file, "{timestamp} [{thread_name}] {}", message);
        let _ = file.flush();
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

    fn wait_for_front_protocol(preview: &mut ModelPreview) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && preview.presenter.presented_sequence() == 0 {
            preview.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_ne!(preview.presenter.presented_sequence(), 0);
    }

    #[test]
    fn editing_marks_an_existing_preview_stale() {
        let mut preview = ModelPreview {
            status: ModelPreviewStatus::Ready { triangles: 12 },
            ..ModelPreview::default()
        };
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
        preview.set_auto_rotate(true);
        preview.bounds = Some(Aabb {
            min: openscad_render::Vec3::splat(-1.0),
            max: openscad_render::Vec3::splat(1.0),
        });
        preview.status = ModelPreviewStatus::Rasterizing;
        let yaw = preview.camera.yaw;
        preview.tick(preview.last_animation_tick + AUTO_ROTATE_FRAME_INTERVAL);
        assert_eq!(preview.camera.yaw, yaw);

        preview.status = ModelPreviewStatus::Ready { triangles: 12 };
        preview.tick(preview.last_animation_tick + AUTO_ROTATE_FRAME_INTERVAL);
        assert_ne!(preview.camera.yaw, yaw);
    }

    #[test]
    fn auto_rotation_is_rate_limited() {
        let mut preview = ModelPreview::default();
        preview.set_auto_rotate(true);
        preview.bounds = Some(Aabb {
            min: openscad_render::Vec3::splat(-1.0),
            max: openscad_render::Vec3::splat(1.0),
        });
        preview.status = ModelPreviewStatus::Ready { triangles: 12 };
        let yaw = preview.camera.yaw;

        preview.tick(preview.last_animation_tick + Duration::from_millis(32));

        assert_eq!(preview.camera.yaw, yaw);
    }

    #[test]
    fn stopping_auto_rotation_changes_only_rotation_state() {
        let mut preview = ModelPreview::default();
        preview.set_auto_rotate(true);
        preview.set_auto_rotate(false);
        assert!(!preview.auto_rotate);
    }

    #[test]
    fn presentation_failure_recovers_without_overwriting_model_status() {
        let mut preview = ModelPreview {
            status: ModelPreviewStatus::Ready { triangles: 12 },
            ..ModelPreview::default()
        };

        preview.handle_presentation_result(Err("unsupported display protocol".to_string()));
        assert_eq!(preview.status, ModelPreviewStatus::Ready { triangles: 12 });
        assert_eq!(
            preview.presentation_error(),
            Some("unsupported display protocol")
        );

        preview.handle_presentation_result(Ok(Some(openscad_terminal::PresentationUpdate {
            sequence: 1,
            encode_time: Duration::from_millis(2),
            encoded_bytes: 128,
            encoded_bytes_estimated: false,
        })));
        assert_eq!(preview.status, ModelPreviewStatus::Ready { triangles: 12 });
        assert_eq!(preview.presentation_error(), None);
        assert_eq!(preview.metrics.encoded_bytes, 128);
    }

    #[test]
    fn orbit_and_pan_request_intermediate_frames() {
        let mut preview = ModelPreview {
            bounds: Some(Aabb {
                min: openscad_render::Vec3::splat(-1.0),
                max: openscad_render::Vec3::splat(1.0),
            }),
            service: Some(RenderService::new(
                Box::new(OpenScadGenerator::new("unused-in-this-test")),
                Box::new(CpuRenderer::default()),
            )),
            ..ModelPreview::default()
        };
        let initial_revision = preview.camera_revision;

        preview.orbit(3.0, -2.0).unwrap();
        assert_eq!(preview.camera_revision, initial_revision + 1);
        preview.pan(0.01, -0.01).unwrap();
        assert_eq!(preview.camera_revision, initial_revision + 2);
    }

    #[test]
    fn toggling_axes_rasterizes_the_cached_mesh_only() {
        let mut preview = ModelPreview {
            bounds: Some(Aabb {
                min: openscad_render::Vec3::splat(-1.0),
                max: openscad_render::Vec3::splat(1.0),
            }),
            service: Some(RenderService::new(
                Box::new(OpenScadGenerator::new("unused-in-this-test")),
                Box::new(CpuRenderer::default()),
            )),
            ..ModelPreview::default()
        };
        let mesh_revision = preview.mesh_revision;
        let camera_revision = preview.camera_revision;

        preview.set_axes_visible(false);

        assert!(!preview.axes_visible);
        assert_eq!(preview.mesh_revision, mesh_revision);
        assert_eq!(preview.camera_revision, camera_revision + 1);
        assert_eq!(preview.status, ModelPreviewStatus::Rasterizing);
    }

    #[test]
    fn expands_home_directory_as_a_complete_path_component() {
        let home = dirs::home_dir().expect("test requires a home directory");
        assert_eq!(expand_tilde(PathBuf::from("~")), home);
        assert_eq!(expand_tilde(PathBuf::from("~/models")), home.join("models"));
    }

    #[test]
    fn showing_model_rebuilds_protocol_from_cached_image() {
        let mut picker = Picker::from_fontsize((1, 1));
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut preview = ModelPreview::new(picker);
        preview
            .presenter
            .submit(Arc::new(openscad_render::RgbaFrame::new(
                PixelSize::new(64, 48).unwrap(),
                [20, 24, 32, 255],
            )));

        preview.prepare_for_display();
        wait_for_front_protocol(&mut preview);
        assert!(preview.image_widget().is_some());
    }

    #[test]
    fn image_encoding_runs_in_background_and_reports_metrics() {
        let mut picker = Picker::from_fontsize((1, 1));
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut preview = ModelPreview::new(picker);

        let started = Instant::now();
        preview
            .presenter
            .submit(Arc::new(openscad_render::RgbaFrame::new(
                PixelSize::new(64, 48).unwrap(),
                [20, 24, 32, 255],
            )));
        assert!(started.elapsed() < Duration::from_millis(20));
        wait_for_front_protocol(&mut preview);

        assert!(!preview.metrics.encode_time.is_zero());
        assert!(preview.metrics.encoded_bytes < 64 * 48 * 4);
    }
}
