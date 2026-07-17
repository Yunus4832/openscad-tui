use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    mpsc::{self, Receiver, SyncSender, TrySendError},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{fs::OpenOptions, io::Write};

use icy_sixel::{
    sixel_string, DiffusionMethod, MethodForLargest, MethodForRep, PixelFormat, Quality,
};
use image::{DynamicImage, RgbaImage};
use openscad_render::{
    Aabb, Camera, CpuRenderer, OpenScadGenerator, PixelSize, Projection, RenderEvent,
    RenderService, StandardView,
};
use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
use ratatui_image::{
    picker::Picker,
    protocol::{Protocol, StatefulProtocolType},
    Image, Resize,
};

use crate::kitty_protocol::CompressedKittyProtocol;

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

struct LatestRequestSender<T> {
    latest: Arc<Mutex<Option<T>>>,
    wake: SyncSender<()>,
}

struct LatestRequestReceiver<T> {
    latest: Arc<Mutex<Option<T>>>,
    wake: Receiver<()>,
}

fn latest_request_channel<T>() -> (LatestRequestSender<T>, LatestRequestReceiver<T>) {
    let latest = Arc::new(Mutex::new(None));
    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    (
        LatestRequestSender {
            latest: Arc::clone(&latest),
            wake: wake_tx,
        },
        LatestRequestReceiver {
            latest,
            wake: wake_rx,
        },
    )
}

impl<T> LatestRequestSender<T> {
    fn send(&self, request: T) -> Result<(), ()> {
        *self.latest.lock().expect("latest request mutex poisoned") = Some(request);
        match self.wake.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => Ok(()),
            Err(TrySendError::Disconnected(())) => {
                self.latest
                    .lock()
                    .expect("latest request mutex poisoned")
                    .take();
                Err(())
            }
        }
    }
}

impl<T> LatestRequestReceiver<T> {
    fn recv(&self) -> Result<T, ()> {
        loop {
            self.wake.recv().map_err(|_| ())?;
            if let Some(request) = self
                .latest
                .lock()
                .expect("latest request mutex poisoned")
                .take()
            {
                return Ok(request);
            }
        }
    }
}

struct ProtocolEncodeRequest {
    sequence: u64,
    picker: Picker,
    image: DynamicImage,
    area: Rect,
    kitty_image_id: u32,
    is_tmux: bool,
}

struct ProtocolEncodeResponse {
    sequence: u64,
    protocol_type: ratatui_image::picker::ProtocolType,
    protocol: Result<PreviewProtocol, String>,
    elapsed: Duration,
    encoded_bytes: usize,
    encoded_bytes_estimated: bool,
}

enum PreviewProtocol {
    Standard(Protocol),
    CompressedKitty(CompressedKittyProtocol),
}

impl PreviewProtocol {
    fn encoded_bytes(&self) -> usize {
        match self {
            Self::Standard(Protocol::Sixel(sixel)) => sixel.data.len(),
            Self::Standard(Protocol::ITerm2(iterm2)) => iterm2.data.len(),
            Self::Standard(Protocol::Kitty(_)) | Self::Standard(Protocol::Halfblocks(_)) => 0,
            Self::CompressedKitty(kitty) => kitty.encoded_bytes(),
        }
    }
}

pub(crate) struct PreviewImage<'a> {
    protocol: &'a mut PreviewProtocol,
}

impl Widget for PreviewImage<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        match self.protocol {
            PreviewProtocol::Standard(protocol) => Image::new(protocol).render(area, buffer),
            PreviewProtocol::CompressedKitty(protocol) => protocol.render(area, buffer),
        }
    }
}

struct SixelEncodeRequest {
    sequence: u64,
    image: DynamicImage,
    area: Rect,
    is_tmux: bool,
}

struct SixelEncodeResponse {
    sequence: u64,
    data: Result<String, String>,
    area: Rect,
    elapsed: Duration,
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
    picker: Picker,
    is_tmux: bool,
    kitty_image_id: u32,
    protocol_requests: LatestRequestSender<ProtocolEncodeRequest>,
    protocol_responses: Receiver<ProtocolEncodeResponse>,
    sixel_requests: LatestRequestSender<SixelEncodeRequest>,
    sixel_responses: Receiver<SixelEncodeResponse>,
    encode_sequence: u64,
    presented_sequence: u64,
    front_protocol: Option<PreviewProtocol>,
    last_image: Option<DynamicImage>,
    service: Option<RenderService>,
    viewport: PixelSize,
    mesh_revision: u64,
    camera_revision: u64,
    bounds: Option<Aabb>,
    fitted_revision: Option<u64>,
    pub auto_rotate: bool,
    pub metrics: RenderMetrics,
    last_animation_tick: Instant,
    last_presented_at: Option<Instant>,
    last_drawn_sequence: u64,
}

impl Default for ModelPreview {
    fn default() -> Self {
        Self::new(Picker::from_fontsize((10, 20)))
    }
}

impl ModelPreview {
    pub fn new(picker: Picker) -> Self {
        let is_tmux = picker_is_tmux(&picker);
        let (protocol_request_tx, protocol_request_rx) =
            latest_request_channel::<ProtocolEncodeRequest>();
        let (protocol_response_tx, protocol_response_rx) =
            mpsc::channel::<ProtocolEncodeResponse>();
        std::thread::Builder::new()
            .name("image-protocol-encode".to_string())
            .spawn(move || {
                while let Ok(request) = protocol_request_rx.recv() {
                    let started = Instant::now();
                    let protocol_type = request.picker.protocol_type();
                    let protocol = if protocol_type == ratatui_image::picker::ProtocolType::Kitty {
                        CompressedKittyProtocol::encode(
                            &request.image,
                            request.area,
                            request.kitty_image_id,
                            request.is_tmux,
                        )
                        .map(PreviewProtocol::CompressedKitty)
                    } else {
                        request
                            .picker
                            .new_protocol(request.image, request.area, Resize::Fit(None))
                            .map(PreviewProtocol::Standard)
                            .map_err(|error| error.to_string())
                    };
                    let encoded_bytes = protocol
                        .as_ref()
                        .map(PreviewProtocol::encoded_bytes)
                        .unwrap_or(0);
                    if protocol_response_tx
                        .send(ProtocolEncodeResponse {
                            sequence: request.sequence,
                            protocol_type,
                            protocol,
                            elapsed: started.elapsed(),
                            encoded_bytes,
                            encoded_bytes_estimated: false,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("failed to start image protocol encoding worker");
        let (sixel_request_tx, sixel_request_rx) = latest_request_channel::<SixelEncodeRequest>();
        let (sixel_response_tx, sixel_response_rx) = mpsc::channel::<SixelEncodeResponse>();
        std::thread::Builder::new()
            .name("sixel-fast-encode".to_string())
            .spawn(move || {
                while let Ok(request) = sixel_request_rx.recv() {
                    render_trace(|| {
                        format!(
                            "encode-start seq={} image={}",
                            request.sequence,
                            image_checksum(&request.image)
                        )
                    });
                    let started = Instant::now();
                    let rgb = request.image.to_rgb8();
                    // Keep one encoding profile for still images and camera motion. HIGH avoids
                    // the terminal compatibility issue observed with LOW, while disabling
                    // diffusion preserves the camera-motion performance baseline.
                    let data = sixel_string(
                        rgb.as_raw(),
                        rgb.width() as i32,
                        rgb.height() as i32,
                        PixelFormat::RGB888,
                        DiffusionMethod::None,
                        MethodForLargest::Auto,
                        MethodForRep::Auto,
                        Quality::HIGH,
                    )
                    .map(|mut data| {
                        if request.is_tmux {
                            data.insert_str(0, "\x1b\x1b");
                            data.insert_str(0, "\x1bPtmux;");
                            data.push_str("\x1b\\");
                        }
                        data
                    })
                    .map_err(|error| error.to_string());
                    let elapsed = started.elapsed();
                    let result_summary = match &data {
                        Ok(data) => format!(
                            "bytes={} checksum={:016x}",
                            data.len(),
                            byte_checksum(data.as_bytes())
                        ),
                        Err(error) => format!("error={error}"),
                    };
                    render_trace(|| {
                        format!(
                            "encode-finish seq={} elapsed_ms={:.2} {}",
                            request.sequence,
                            elapsed.as_secs_f64() * 1000.0,
                            result_summary
                        )
                    });
                    if sixel_response_tx
                        .send(SixelEncodeResponse {
                            sequence: request.sequence,
                            data,
                            area: request.area,
                            elapsed,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("failed to start fast Sixel encoding worker");
        Self {
            status: ModelPreviewStatus::Empty,
            camera: Camera::default(),
            picker,
            is_tmux,
            kitty_image_id: next_kitty_image_id(),
            protocol_requests: protocol_request_tx,
            protocol_responses: protocol_response_rx,
            sixel_requests: sixel_request_tx,
            sixel_responses: sixel_response_rx,
            encode_sequence: 0,
            presented_sequence: 0,
            front_protocol: None,
            last_image: None,
            service: None,
            viewport: PixelSize::new(640, 480).expect("constant viewport is valid"),
            mesh_revision: 0,
            camera_revision: 0,
            bounds: None,
            fitted_revision: None,
            auto_rotate: false,
            metrics: RenderMetrics::default(),
            last_animation_tick: Instant::now(),
            last_presented_at: None,
            last_drawn_sequence: 0,
        }
    }

    pub fn set_picker(&mut self, picker: Picker) {
        render_trace(|| format!("picker-set protocol={:?}", picker.protocol_type()));
        self.is_tmux = picker_is_tmux(&picker);
        self.picker = picker;
        self.rebuild_protocol();
    }

    pub fn protocol_type(&self) -> ratatui_image::picker::ProtocolType {
        self.picker.protocol_type()
    }

    pub(crate) fn image_widget(&mut self) -> Option<PreviewImage<'_>> {
        if self.front_protocol.is_some() && self.last_drawn_sequence != self.presented_sequence {
            self.last_drawn_sequence = self.presented_sequence;
            self.record_presented_frame();
            render_trace(|| {
                format!(
                    "ui-widget seq={} bytes={}",
                    self.presented_sequence, self.metrics.encoded_bytes
                )
            });
        }
        self.front_protocol
            .as_mut()
            .map(|protocol| PreviewImage { protocol })
    }

    pub fn prepare_for_display(&mut self) {
        // A terminal clear removes the graphics layer. Rebuilding the fixed protocol forces the
        // cached image to be transmitted again without rerunning OpenSCAD.
        self.rebuild_protocol();
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
        self.encode_sequence = self.encode_sequence.wrapping_add(1);
        self.presented_sequence = self.encode_sequence;
        self.bounds = None;
        self.fitted_revision = None;
        self.front_protocol = None;
        self.last_image = None;
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
        while let Ok(encoded) = self.sixel_responses.try_recv() {
            if self.picker.protocol_type() != ratatui_image::picker::ProtocolType::Sixel
                || encoded.sequence < self.presented_sequence
            {
                render_trace(|| {
                    format!(
                        "encode-drop seq={} already_presented={}",
                        encoded.sequence, self.presented_sequence
                    )
                });
                continue;
            }
            match encoded.data {
                Ok(data) => {
                    self.presented_sequence = encoded.sequence;
                    self.metrics.encode_time = encoded.elapsed;
                    self.metrics.encoded_bytes = data.len();
                    self.metrics.encoded_bytes_estimated = false;
                    self.front_protocol = Some(PreviewProtocol::Standard(Protocol::Sixel(
                        ratatui_image::protocol::sixel::Sixel {
                            data,
                            area: encoded.area,
                            is_tmux: self.is_tmux,
                        },
                    )));
                    render_trace(|| {
                        format!(
                            "encode-present seq={} bytes={} elapsed_ms={:.2}",
                            encoded.sequence,
                            self.metrics.encoded_bytes,
                            encoded.elapsed.as_secs_f64() * 1000.0
                        )
                    });
                }
                Err(error) => self.status = ModelPreviewStatus::Failed(error),
            }
        }
        while let Ok(encoded) = self.protocol_responses.try_recv() {
            if encoded.protocol_type != self.picker.protocol_type()
                || encoded.sequence < self.presented_sequence
            {
                render_trace(|| {
                    format!(
                        "protocol-encode-drop seq={} protocol={:?} already_presented={}",
                        encoded.sequence, encoded.protocol_type, self.presented_sequence
                    )
                });
                continue;
            }
            match encoded.protocol {
                Ok(protocol) => {
                    self.presented_sequence = encoded.sequence;
                    self.front_protocol = Some(protocol);
                    self.metrics.encode_time = encoded.elapsed;
                    self.metrics.encoded_bytes = encoded.encoded_bytes;
                    self.metrics.encoded_bytes_estimated = encoded.encoded_bytes_estimated;
                    render_trace(|| {
                        format!(
                            "protocol-encode-present seq={} protocol={:?} bytes={} elapsed_ms={:.2}",
                            encoded.sequence,
                            encoded.protocol_type,
                            encoded.encoded_bytes,
                            encoded.elapsed.as_secs_f64() * 1000.0
                        )
                    });
                }
                Err(error) => self.status = ModelPreviewStatus::Failed(error),
            }
        }
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
                    let size = rendered.frame.size();
                    if let Some(image) = RgbaImage::from_raw(
                        size.width,
                        size.height,
                        rendered.frame.pixels().to_vec(),
                    ) {
                        let image = DynamicImage::ImageRgba8(image);
                        self.last_image = Some(image.clone());
                        self.queue_image_encode(image);
                        if is_latest {
                            self.status = ModelPreviewStatus::Ready {
                                triangles: rendered.triangle_count,
                            };
                        }
                    } else {
                        self.status = ModelPreviewStatus::Failed(
                            "renderer returned an invalid RGBA buffer".to_string(),
                        );
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
                    self.status = ModelPreviewStatus::Failed(error.to_string());
                }
                _ => {}
            }
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
        self.request_rasterize_at(self.viewport);
    }

    fn request_rasterize_at(&mut self, size: PixelSize) {
        let Some(service) = &self.service else {
            return;
        };
        self.camera_revision = self.camera_revision.wrapping_add(1);
        self.status = ModelPreviewStatus::Rasterizing;
        if let Err(error) = service.rasterize(self.camera_revision, self.camera, size) {
            self.status = ModelPreviewStatus::Failed(error.to_string());
        }
    }

    fn rebuild_protocol(&mut self) {
        if let Some(image) = self.last_image.as_ref().cloned() {
            self.queue_image_encode(image);
        }
    }

    fn queue_image_encode(&mut self, image: DynamicImage) {
        self.encode_sequence = self.encode_sequence.wrapping_add(1);
        let sequence = self.encode_sequence;
        let area = protocol_area(&image, self.picker.font_size());
        if self.picker.protocol_type() == ratatui_image::picker::ProtocolType::Sixel {
            render_trace(|| {
                format!(
                    "encode-queue seq={} camera_rev={} image={}",
                    sequence,
                    self.camera_revision,
                    image_checksum(&image)
                )
            });
            let _ = self.sixel_requests.send(SixelEncodeRequest {
                sequence,
                image,
                area,
                is_tmux: self.is_tmux,
            });
        } else {
            render_trace(|| {
                format!(
                    "protocol-encode-queue seq={} protocol={:?} image={}",
                    sequence,
                    self.picker.protocol_type(),
                    image_checksum(&image)
                )
            });
            let _ = self.protocol_requests.send(ProtocolEncodeRequest {
                sequence,
                picker: self.picker,
                image,
                area,
                kitty_image_id: self.kitty_image_id,
                is_tmux: self.is_tmux,
            });
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

fn picker_is_tmux(picker: &Picker) -> bool {
    let mut probe_picker = *picker;
    probe_picker.set_protocol_type(ratatui_image::picker::ProtocolType::Sixel);
    let probe = probe_picker.new_resize_protocol(DynamicImage::ImageRgba8(RgbaImage::new(1, 1)));
    matches!(
        probe.protocol_type(),
        StatefulProtocolType::Sixel(sixel) if sixel.is_tmux
    )
}

fn next_kitty_image_id() -> u32 {
    static NEXT_ID: AtomicU32 = AtomicU32::new(1);
    std::process::id().rotate_left(16) ^ NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn protocol_area(image: &DynamicImage, font_size: (u16, u16)) -> Rect {
    Rect::new(
        0,
        0,
        image
            .width()
            .div_ceil(u32::from(font_size.0))
            .min(u32::from(u16::MAX)) as u16,
        image
            .height()
            .div_ceil(u32::from(font_size.1))
            .min(u32::from(u16::MAX)) as u16,
    )
}

fn image_checksum(image: &DynamicImage) -> String {
    let rgba = image.to_rgba8();
    format!(
        "{}x{}:{:016x}",
        rgba.width(),
        rgba.height(),
        byte_checksum(rgba.as_raw())
    )
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
        while Instant::now() < deadline && preview.front_protocol.is_none() {
            preview.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(preview.front_protocol.is_some());
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
    fn stopping_auto_rotation_does_not_queue_a_special_final_frame() {
        let mut preview = ModelPreview::default();
        preview.last_image = Some(DynamicImage::ImageRgba8(RgbaImage::new(16, 12)));
        let initial_sequence = preview.encode_sequence;

        preview.set_auto_rotate(true);
        preview.set_auto_rotate(false);

        assert_eq!(preview.encode_sequence, initial_sequence);
    }

    #[test]
    fn orbit_and_pan_request_intermediate_frames() {
        let mut preview = ModelPreview::default();
        preview.bounds = Some(Aabb {
            min: openscad_render::Vec3::splat(-1.0),
            max: openscad_render::Vec3::splat(1.0),
        });
        preview.service = Some(RenderService::new(
            Box::new(OpenScadGenerator::new("unused-in-this-test")),
            Box::new(CpuRenderer::default()),
        ));
        let initial_revision = preview.camera_revision;

        preview.orbit(3.0, -2.0).unwrap();
        assert_eq!(preview.camera_revision, initial_revision + 1);
        preview.pan(0.01, -0.01).unwrap();
        assert_eq!(preview.camera_revision, initial_revision + 2);
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
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let mut preview = ModelPreview::new(picker);
        preview.last_image = Some(DynamicImage::ImageRgba8(RgbaImage::new(2, 2)));
        preview.front_protocol = None;

        preview.prepare_for_display();
        wait_for_front_protocol(&mut preview);

        assert!(matches!(
            preview.front_protocol,
            Some(PreviewProtocol::CompressedKitty(_))
        ));
    }

    #[test]
    fn image_encoding_runs_in_background_and_reports_metrics() {
        let mut picker = Picker::from_fontsize((1, 1));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let mut preview = ModelPreview::new(picker);

        let started = Instant::now();
        preview.queue_image_encode(DynamicImage::ImageRgba8(RgbaImage::new(64, 48)));
        assert!(started.elapsed() < Duration::from_millis(20));
        wait_for_front_protocol(&mut preview);

        assert!(!preview.metrics.encode_time.is_zero());
        assert!(preview.metrics.encoded_bytes < 64 * 48 * 4);
        assert!(matches!(
            preview.front_protocol,
            Some(PreviewProtocol::CompressedKitty(_))
        ));
    }

    #[test]
    fn latest_request_channel_coalesces_pending_frames() {
        let (sender, receiver) = latest_request_channel();
        sender.send(1_u8).unwrap();
        sender.send(2_u8).unwrap();
        sender.send(3_u8).unwrap();

        assert_eq!(receiver.recv(), Ok(3));
    }

    #[test]
    fn kitty_front_buffer_remains_visible_while_next_frame_encodes() {
        let mut picker = Picker::from_fontsize((1, 1));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let mut preview = ModelPreview::new(picker);
        preview.queue_image_encode(DynamicImage::ImageRgba8(RgbaImage::new(64, 48)));
        wait_for_front_protocol(&mut preview);
        let presented = preview.presented_sequence;

        preview.queue_image_encode(DynamicImage::ImageRgba8(RgbaImage::new(128, 96)));

        assert_eq!(preview.presented_sequence, presented);
        assert!(matches!(
            preview.front_protocol,
            Some(PreviewProtocol::CompressedKitty(_))
        ));
    }

    #[test]
    fn sixel_front_buffer_remains_visible_while_next_frame_encodes() {
        let mut picker = Picker::from_fontsize((1, 1));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Sixel);
        let mut preview = ModelPreview::new(picker);
        preview.queue_image_encode(DynamicImage::ImageRgba8(RgbaImage::new(64, 48)));

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && preview.front_protocol.is_none() {
            preview.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(preview.front_protocol.is_some());
        let front = match preview.front_protocol.as_ref().unwrap() {
            PreviewProtocol::Standard(Protocol::Sixel(sixel)) => sixel.data.clone(),
            _ => unreachable!(),
        };

        preview.set_auto_rotate(true);
        preview.queue_image_encode(DynamicImage::ImageRgba8(RgbaImage::new(128, 96)));

        let current = match preview.front_protocol.as_ref().unwrap() {
            PreviewProtocol::Standard(Protocol::Sixel(sixel)) => &sixel.data,
            _ => unreachable!(),
        };
        assert_eq!(current, &front);
    }

    #[test]
    fn sixel_encoding_changes_when_pixels_change() {
        use image::Rgba;

        let encode = |offset: u32| {
            let mut image = RgbaImage::from_pixel(64, 48, Rgba([20, 24, 32, 255]));
            for y in 8..32 {
                for x in offset..offset + 20 {
                    image.put_pixel(x, y, Rgba([135, 180, 220, 255]));
                }
            }
            let rgb = DynamicImage::ImageRgba8(image).to_rgb8();
            sixel_string(
                rgb.as_raw(),
                rgb.width() as i32,
                rgb.height() as i32,
                PixelFormat::RGB888,
                DiffusionMethod::None,
                MethodForLargest::Auto,
                MethodForRep::Auto,
                Quality::HIGH,
            )
            .unwrap()
        };

        assert_ne!(encode(8), encode(24));
    }
}
