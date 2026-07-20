//! High-throughput terminal presentation for continuously changing RGBA frames.

use std::fmt::Write as FmtWrite;
use std::io::Write as IoWrite;
use std::sync::{
    mpsc::{self, Receiver, SyncSender, TrySendError},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose, Engine};
use flate2::{write::ZlibEncoder, Compression};
use icy_sixel::{
    sixel_string, DiffusionMethod, MethodForLargest, MethodForRep, PixelFormat, Quality,
};
use jpeg_encoder::{ColorType as JpegColorType, Encoder as JpegEncoder};
use openscad_render::{PixelSize, RgbaFrame};
use ratatui::{buffer::Buffer, layout::Rect, style::Color, widgets::Widget};

mod protocol;

pub use protocol::{DisplayProtocol, ParseDisplayProtocolError};

const KITTY_CHUNK_BYTES: usize = 3072;
// Paul Bourke's commonly used ASCII density ramp, ordered from sparse to dense. Background is
// handled separately, so the leading space from the original sequence is intentionally omitted.
const ASCII_RAMP: &[u8] =
    br##".'`^",:;Il!i><~+_-?][}{1)(|\/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$"##;
const TEXT_BACKGROUND_DISTANCE_SQUARED: u32 = 16 * 16 * 3;
const TEXT_MIN_CONTRAST: u16 = 72;
const MIN_ROBUST_TONE_RANGE: u8 = 8;
const BRAILLE_MIN_STRENGTH: f32 = 1.0 / 8.0;
const BRAILLE_BASE: u32 = 0x2800;
const BRAILLE_DOTS: [(u32, u32, u8, u8); 8] = [
    (0, 0, 0x01, 0),
    (0, 1, 0x02, 6),
    (0, 2, 0x04, 1),
    (1, 0, 0x08, 4),
    (1, 1, 0x10, 2),
    (1, 2, 0x20, 5),
    (0, 3, 0x40, 7),
    (1, 3, 0x80, 3),
];

#[derive(Debug, Clone, Copy)]
pub struct PresentationContext {
    pub cells: Rect,
    pub font_size: (u16, u16),
    pub is_tmux: bool,
    pub kitty_image_id: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct PresentationUpdate {
    pub sequence: u64,
    pub encode_time: Duration,
    pub encoded_bytes: usize,
    pub encoded_bytes_estimated: bool,
}

struct LatestSender<T> {
    latest: Arc<Mutex<Option<T>>>,
    wake: SyncSender<()>,
}

struct LatestReceiver<T> {
    latest: Arc<Mutex<Option<T>>>,
    wake: Receiver<()>,
}

fn latest_channel<T>() -> (LatestSender<T>, LatestReceiver<T>) {
    let latest = Arc::new(Mutex::new(None));
    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    (
        LatestSender {
            latest: Arc::clone(&latest),
            wake: wake_tx,
        },
        LatestReceiver {
            latest,
            wake: wake_rx,
        },
    )
}

impl<T> LatestSender<T> {
    fn send(&self, value: T) -> Result<(), ()> {
        *self.latest.lock().expect("latest frame mutex poisoned") = Some(value);
        match self.wake.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => Ok(()),
            Err(TrySendError::Disconnected(())) => Err(()),
        }
    }

    fn clear(&self) {
        self.latest
            .lock()
            .expect("latest frame mutex poisoned")
            .take();
    }
}

impl<T> LatestReceiver<T> {
    fn recv(&self) -> Result<T, ()> {
        loop {
            self.wake.recv().map_err(|_| ())?;
            if let Some(value) = self
                .latest
                .lock()
                .expect("latest frame mutex poisoned")
                .take()
            {
                return Ok(value);
            }
        }
    }
}

struct EncodeRequest {
    sequence: u64,
    generation: u64,
    protocol: DisplayProtocol,
    frame: Arc<RgbaFrame>,
    context: PresentationContext,
}

struct EncodeResponse {
    sequence: u64,
    generation: u64,
    protocol: DisplayProtocol,
    frame: Result<EncodedFrame, String>,
    elapsed: Duration,
}

pub struct TerminalPresenter {
    protocol: DisplayProtocol,
    context: PresentationContext,
    requests: LatestSender<EncodeRequest>,
    responses: Receiver<EncodeResponse>,
    sequence: u64,
    generation: u64,
    presented_sequence: u64,
    front: Option<EncodedFrame>,
    cached: Option<Arc<RgbaFrame>>,
}

impl TerminalPresenter {
    pub fn new(protocol: DisplayProtocol, context: PresentationContext) -> Self {
        let (request_tx, request_rx) = latest_channel::<EncodeRequest>();
        let (response_tx, response_rx) = mpsc::channel::<EncodeResponse>();
        std::thread::Builder::new()
            .name("terminal-frame-encode".to_string())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    let started = Instant::now();
                    let frame = encode_frame(request.protocol, &request.frame, request.context);
                    if response_tx
                        .send(EncodeResponse {
                            sequence: request.sequence,
                            generation: request.generation,
                            protocol: request.protocol,
                            frame,
                            elapsed: started.elapsed(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("failed to start terminal frame encoder");
        Self {
            protocol,
            context,
            requests: request_tx,
            responses: response_rx,
            sequence: 0,
            generation: 0,
            presented_sequence: 0,
            front: None,
            cached: None,
        }
    }

    pub fn protocol(&self) -> DisplayProtocol {
        self.protocol
    }

    pub fn target_size(&self) -> Option<PixelSize> {
        self.protocol
            .target_size(self.context.cells, self.context.font_size)
    }

    pub fn set_cells(&mut self, cells: Rect) -> bool {
        if self.context.cells == cells {
            return false;
        }
        self.context.cells = cells;
        self.generation = self.generation.wrapping_add(1);
        if self.target_size().is_some() {
            self.reencode_cached();
        }
        true
    }

    pub fn set_font_size(&mut self, font_size: (u16, u16)) {
        if self.context.font_size != font_size {
            self.context.font_size = font_size;
            self.generation = self.generation.wrapping_add(1);
            self.reencode_cached();
        }
    }

    pub fn set_tmux(&mut self, is_tmux: bool) {
        if self.context.is_tmux != is_tmux {
            self.context.is_tmux = is_tmux;
            self.generation = self.generation.wrapping_add(1);
            self.reencode_cached();
        }
    }

    pub fn set_protocol(&mut self, protocol: DisplayProtocol) {
        if self.protocol == protocol {
            return;
        }
        self.protocol = protocol;
        self.generation = self.generation.wrapping_add(1);
        self.front = None;
        if self.target_size().is_some() {
            self.reencode_cached();
        }
    }

    pub fn submit(&mut self, frame: Arc<RgbaFrame>) {
        self.cached = Some(Arc::clone(&frame));
        self.sequence = self.sequence.wrapping_add(1);
        let _ = self.requests.send(EncodeRequest {
            sequence: self.sequence,
            generation: self.generation,
            protocol: self.protocol,
            frame,
            context: self.context,
        });
    }

    pub fn reencode_cached(&mut self) {
        if let Some(frame) = self.cached.as_ref().map(Arc::clone) {
            self.submit(frame);
        }
    }

    pub fn clear(&mut self) {
        self.sequence = self.sequence.wrapping_add(1);
        self.generation = self.generation.wrapping_add(1);
        self.requests.clear();
        self.front = None;
        self.cached = None;
    }

    pub fn poll(&mut self) -> Result<Option<PresentationUpdate>, String> {
        let mut latest = None;
        while let Ok(response) = self.responses.try_recv() {
            if let Some(update) = self.apply_response(response)? {
                latest = Some(update);
            }
        }
        Ok(latest)
    }

    fn apply_response(
        &mut self,
        response: EncodeResponse,
    ) -> Result<Option<PresentationUpdate>, String> {
        if response.generation != self.generation
            || response.protocol != self.protocol
            || response.sequence < self.presented_sequence
        {
            return Ok(None);
        }
        let frame = response.frame?;
        let (encoded_bytes, encoded_bytes_estimated) = frame.encoded_size();
        self.presented_sequence = response.sequence;
        self.front = Some(frame);
        Ok(Some(PresentationUpdate {
            sequence: response.sequence,
            encode_time: response.elapsed,
            encoded_bytes,
            encoded_bytes_estimated,
        }))
    }

    pub fn image(&mut self) -> Option<TerminalImage<'_>> {
        self.front.as_mut().map(|frame| TerminalImage { frame })
    }

    pub fn presented_sequence(&self) -> u64 {
        self.presented_sequence
    }

    #[cfg(test)]
    fn has_front(&self) -> bool {
        self.front.is_some()
    }
}

pub struct TerminalImage<'a> {
    frame: &'a mut EncodedFrame,
}

impl Widget for TerminalImage<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        self.frame.render(area, buffer);
    }
}

enum EncodedFrame {
    Kitty(KittyFrame),
    Escape(EscapeFrame),
    Cells(CellFrame),
}

impl EncodedFrame {
    fn encoded_size(&self) -> (usize, bool) {
        match self {
            Self::Kitty(frame) => (frame.encoded_bytes, false),
            Self::Escape(frame) => (frame.data.len(), false),
            Self::Cells(frame) => (frame.cells.len() * 12, true),
        }
    }

    fn render(&mut self, area: Rect, buffer: &mut Buffer) {
        match self {
            Self::Kitty(frame) => frame.render(area, buffer),
            Self::Escape(frame) => frame.render(area, buffer),
            Self::Cells(frame) => frame.render(area, buffer),
        }
    }
}

struct KittyFrame {
    transmit: Option<String>,
    placement: String,
    area: Rect,
    encoded_bytes: usize,
}

impl KittyFrame {
    fn render(&mut self, area: Rect, buffer: &mut Buffer) {
        let symbol = self
            .transmit
            .take()
            .unwrap_or_else(|| self.placement.clone());
        render_escape(&symbol, self.area, area, buffer);
    }
}

struct EscapeFrame {
    data: String,
    area: Rect,
}

impl EscapeFrame {
    fn render(&self, area: Rect, buffer: &mut Buffer) {
        render_escape(&self.data, self.area, area, buffer);
    }
}

#[derive(Clone)]
struct EncodedCell {
    symbol: char,
    foreground: Color,
    background: Color,
}

struct CellFrame {
    width: u16,
    height: u16,
    cells: Vec<EncodedCell>,
}

impl CellFrame {
    fn render(&self, area: Rect, buffer: &mut Buffer) {
        let width = self.width.min(area.width);
        let height = self.height.min(area.height);
        for y in 0..height {
            for x in 0..width {
                let encoded =
                    &self.cells[usize::from(y) * usize::from(self.width) + usize::from(x)];
                if let Some(cell) = buffer.cell_mut((area.x + x, area.y + y)) {
                    cell.set_char(encoded.symbol)
                        .set_fg(encoded.foreground)
                        .set_bg(encoded.background);
                }
            }
        }
    }
}

fn encode_frame(
    protocol: DisplayProtocol,
    frame: &RgbaFrame,
    context: PresentationContext,
) -> Result<EncodedFrame, String> {
    let target = protocol
        .target_size(context.cells, context.font_size)
        .ok_or_else(|| "terminal preview area is empty".to_string())?;
    match protocol {
        DisplayProtocol::Kitty => encode_kitty(frame, target, context).map(EncodedFrame::Kitty),
        DisplayProtocol::Sixel => encode_sixel(frame, target, context).map(EncodedFrame::Escape),
        DisplayProtocol::Iterm2 => encode_iterm2(frame, target, context).map(EncodedFrame::Escape),
        DisplayProtocol::Halfblocks => {
            Ok(EncodedFrame::Cells(encode_halfblocks(frame, context.cells)))
        }
        DisplayProtocol::Braille => Ok(EncodedFrame::Cells(encode_braille(frame, context.cells))),
        DisplayProtocol::Ascii => Ok(EncodedFrame::Cells(encode_ascii(frame, context.cells))),
    }
}

fn encode_kitty(
    frame: &RgbaFrame,
    target: PixelSize,
    context: PresentationContext,
) -> Result<KittyFrame, String> {
    let rgb = resized_rgb(frame, target);
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&rgb).map_err(|error| error.to_string())?;
    let compressed = encoder.finish().map_err(|error| error.to_string())?;
    let transmit = kitty_transmit(&compressed, target, context);
    let encoded_bytes = transmit.len();
    Ok(KittyFrame {
        transmit: Some(transmit),
        placement: kitty_placement(context),
        area: context.cells,
        encoded_bytes,
    })
}

fn kitty_transmit(compressed: &[u8], size: PixelSize, context: PresentationContext) -> String {
    let (start, escape, end) = tmux_escape(context.is_tmux);
    let chunks = compressed.chunks(KITTY_CHUNK_BYTES);
    let chunk_count = chunks.len();
    let mut data = String::from(start);
    for (index, chunk) in chunks.enumerate() {
        let payload = general_purpose::STANDARD.encode(chunk);
        data.push_str(escape);
        if index == 0 {
            let more = usize::from(chunk_count > 1);
            write!(
                data,
                "_Gq=2,i={},p=1,a=T,f=24,t=d,o=z,s={},v={},c={},r={},C=1,m={more};{payload}",
                context.kitty_image_id,
                size.width,
                size.height,
                context.cells.width,
                context.cells.height
            )
            .expect("String write cannot fail");
        } else {
            let more = usize::from(index + 1 < chunk_count);
            write!(data, "_Gq=2,m={more};{payload}").expect("String write cannot fail");
        }
        data.push_str(escape);
        data.push('\\');
    }
    data.push_str(end);
    data
}

fn kitty_placement(context: PresentationContext) -> String {
    let (start, escape, end) = tmux_escape(context.is_tmux);
    format!(
        "{start}{escape}_Gq=2,i={},p=1,a=p,c={},r={},C=1;{escape}\\{end}",
        context.kitty_image_id, context.cells.width, context.cells.height
    )
}

fn encode_sixel(
    frame: &RgbaFrame,
    target: PixelSize,
    context: PresentationContext,
) -> Result<EscapeFrame, String> {
    let rgb = resized_rgb(frame, target);
    let mut data = sixel_string(
        &rgb,
        target.width as i32,
        target.height as i32,
        PixelFormat::RGB888,
        DiffusionMethod::None,
        MethodForLargest::Auto,
        MethodForRep::Auto,
        Quality::HIGH,
    )
    .map_err(|error| error.to_string())?;
    if context.is_tmux {
        data = wrap_tmux(&data);
    }
    Ok(EscapeFrame {
        data,
        area: context.cells,
    })
}

fn encode_iterm2(
    frame: &RgbaFrame,
    target: PixelSize,
    context: PresentationContext,
) -> Result<EscapeFrame, String> {
    let rgb = resized_rgb(frame, target);
    let width = u16::try_from(target.width)
        .map_err(|_| format!("iTerm2 frame width {} exceeds JPEG limits", target.width))?;
    let height = u16::try_from(target.height)
        .map_err(|_| format!("iTerm2 frame height {} exceeds JPEG limits", target.height))?;
    let mut jpeg_data = Vec::new();
    JpegEncoder::new(&mut jpeg_data, 80)
        .encode(&rgb, width, height, JpegColorType::Rgb)
        .map_err(|error| error.to_string())?;
    let encoded_size = jpeg_data.len();
    let payload = general_purpose::STANDARD.encode(jpeg_data);
    let sequence = format!(
        "\x1b]1337;File=size={encoded_size};inline=1;width={};height={};preserveAspectRatio=0:{payload}\x07",
        context.cells.width, context.cells.height,
    );
    Ok(EscapeFrame {
        data: if context.is_tmux {
            wrap_tmux(&sequence)
        } else {
            sequence
        },
        area: context.cells,
    })
}

fn encode_halfblocks(frame: &RgbaFrame, cells_area: Rect) -> CellFrame {
    let width = cells_area.width;
    let height = cells_area.height;
    let target = PixelSize::new(u32::from(width), u32::from(height) * 2)
        .expect("non-empty presentation context has a valid target");
    let mut cells = Vec::with_capacity(usize::from(width) * usize::from(height));
    for y in 0..height {
        for x in 0..width {
            let top = sampled_rgba(frame, u32::from(x), u32::from(y) * 2, target);
            let bottom = sampled_rgba(frame, u32::from(x), u32::from(y) * 2 + 1, target);
            cells.push(EncodedCell {
                symbol: '▀',
                foreground: Color::Rgb(top[0], top[1], top[2]),
                background: Color::Rgb(bottom[0], bottom[1], bottom[2]),
            });
        }
    }
    CellFrame {
        width,
        height,
        cells,
    }
}

fn encode_ascii(frame: &RgbaFrame, cells_area: Rect) -> CellFrame {
    let width = cells_area.width;
    let height = cells_area.height;
    let target = PixelSize::new(u32::from(width), u32::from(height))
        .expect("non-empty presentation context has a valid target");
    let background = frame.background();
    let background_color = Color::Rgb(background[0], background[1], background[2]);
    let tone_curve = TextToneCurve::from_frame(frame, background);
    let mut cells = Vec::with_capacity(usize::from(width) * usize::from(height));
    for y in 0..height {
        for x in 0..width {
            let sample = sample_text_pixel(frame, u32::from(x), u32::from(y), target, background);
            if sample.coverage == 0.0 {
                cells.push(EncodedCell {
                    symbol: ' ',
                    foreground: Color::Reset,
                    background: background_color,
                });
                continue;
            }

            // Coverage defines silhouette strength while normalized surface luminance retains
            // face shading. The non-space ramp keeps even a partially covered CAD edge visible.
            let shade = tone_curve.map(luminance(&sample.color));
            let density = sample.coverage * (0.35 + shade * 0.65);
            let ramp_index = (density * (ASCII_RAMP.len() - 1) as f32)
                .round()
                .clamp(0.0, (ASCII_RAMP.len() - 1) as f32) as usize;
            let foreground = contrasting_foreground(sample.color, background);
            cells.push(EncodedCell {
                symbol: ASCII_RAMP[ramp_index] as char,
                foreground: Color::Rgb(foreground[0], foreground[1], foreground[2]),
                background: background_color,
            });
        }
    }
    CellFrame {
        width,
        height,
        cells,
    }
}

fn encode_braille(frame: &RgbaFrame, cells_area: Rect) -> CellFrame {
    let width = cells_area.width;
    let height = cells_area.height;
    let target = PixelSize::new(u32::from(width) * 2, u32::from(height) * 4)
        .expect("non-empty presentation context has a valid Braille target");
    let background = frame.background();
    let background_color = Color::Rgb(background[0], background[1], background[2]);
    let tone_curve = TextToneCurve::from_frame(frame, background);
    let samples = (0..target.height)
        .flat_map(|y| {
            (0..target.width).map(move |x| sample_text_pixel(frame, x, y, target, background))
        })
        .collect::<Vec<_>>();
    let mut cells = Vec::with_capacity(usize::from(width) * usize::from(height));
    for cell_y in 0..height {
        for cell_x in 0..width {
            let mut pattern = 0_u8;
            let mut channels = [0_u32; 4];
            let mut visible_dots = 0_u32;
            for (dot_x, dot_y, bit, threshold_level) in BRAILLE_DOTS {
                let sample_x = u32::from(cell_x) * 2 + dot_x;
                let sample_y = u32::from(cell_y) * 4 + dot_y;
                let sample = &samples[(sample_y * target.width + sample_x) as usize];
                if sample.coverage < 0.25 {
                    continue;
                }
                for (sum, channel) in channels.iter_mut().zip(sample.color) {
                    *sum += u32::from(channel);
                }
                visible_dots += 1;
                let tone = tone_curve.map(luminance(&sample.color));
                // Reserve the full one-to-eight-dot range for surface tone. Silhouette samples
                // remain forced on below, so using one dot for the darkest interior does not
                // break the model's outer contour.
                let strength = BRAILLE_MIN_STRENGTH + tone * (1.0 - BRAILLE_MIN_STRENGTH);
                let threshold = (f32::from(threshold_level) + 0.5) / 8.0;
                if is_text_silhouette_edge(&samples, target, sample_x, sample_y)
                    || strength >= threshold
                {
                    pattern |= bit;
                }
            }
            if pattern == 0 {
                cells.push(EncodedCell {
                    symbol: ' ',
                    foreground: Color::Reset,
                    background: background_color,
                });
                continue;
            }
            let color = channels.map(|sum| (sum / visible_dots) as u8);
            let foreground = contrasting_foreground(color, background);
            cells.push(EncodedCell {
                symbol: char::from_u32(BRAILLE_BASE + u32::from(pattern))
                    .expect("Braille bit patterns are valid Unicode scalars"),
                foreground: Color::Rgb(foreground[0], foreground[1], foreground[2]),
                background: background_color,
            });
        }
    }
    CellFrame {
        width,
        height,
        cells,
    }
}

struct TextSample {
    color: [u8; 4],
    coverage: f32,
}

#[derive(Debug, Clone, Copy)]
struct TextToneCurve {
    low: u8,
    high: u8,
}

impl TextToneCurve {
    fn from_frame(frame: &RgbaFrame, background: [u8; 4]) -> Self {
        let mut histogram = [0_u32; 256];
        let mut count = 0_u32;
        for pixel in frame.pixels().chunks_exact(4) {
            if is_text_background(pixel, background) {
                continue;
            }
            histogram[usize::from(luminance(pixel))] += 1;
            count += 1;
        }
        if count == 0 {
            return Self { low: 0, high: 255 };
        }
        let last = count - 1;
        let robust_low = histogram_percentile(&histogram, last * 5 / 100);
        let robust_high = histogram_percentile(&histogram, (last * 95).div_ceil(100));
        if robust_high.saturating_sub(robust_low) >= MIN_ROBUST_TONE_RANGE {
            Self {
                low: robust_low,
                high: robust_high,
            }
        } else {
            // Flat-shaded CAD faces often dominate more than 90% of the image, collapsing both
            // percentiles onto the same color. Fall back to the actual occupied range so smaller
            // faces still affect character or dot density.
            Self {
                low: histogram_percentile(&histogram, 0),
                high: histogram_percentile(&histogram, last),
            }
        }
    }

    fn map(self, value: u16) -> f32 {
        if self.high <= self.low {
            // With a genuinely single-tone model there is no dynamic range to stretch. A neutral
            // density preserves its shape without falsely rendering every Braille cell as solid.
            return 0.5;
        }
        ((f32::from(value) - f32::from(self.low)) / f32::from(self.high.saturating_sub(self.low)))
            .clamp(0.0, 1.0)
    }
}

fn histogram_percentile(histogram: &[u32; 256], rank: u32) -> u8 {
    let mut seen = 0_u32;
    for (value, count) in histogram.iter().enumerate() {
        seen += count;
        if seen > rank {
            return value as u8;
        }
    }
    u8::MAX
}

fn is_text_silhouette_edge(samples: &[TextSample], size: PixelSize, x: u32, y: u32) -> bool {
    [
        (x.wrapping_sub(1), y),
        (x + 1, y),
        (x, y.wrapping_sub(1)),
        (x, y + 1),
    ]
    .into_iter()
    .any(|(neighbor_x, neighbor_y)| {
        neighbor_x >= size.width
            || neighbor_y >= size.height
            || samples[(neighbor_y * size.width + neighbor_x) as usize].coverage < 0.25
    })
}

fn sample_text_pixel(
    frame: &RgbaFrame,
    x: u32,
    y: u32,
    target: PixelSize,
    background: [u8; 4],
) -> TextSample {
    let source = frame.size();
    let x_start = (u64::from(x) * u64::from(source.width) / u64::from(target.width)) as u32;
    let y_start = (u64::from(y) * u64::from(source.height) / u64::from(target.height)) as u32;
    let x_end = ((u64::from(x + 1) * u64::from(source.width)).div_ceil(u64::from(target.width))
        as u32)
        .max(x_start + 1)
        .min(source.width);
    let y_end = ((u64::from(y + 1) * u64::from(source.height)).div_ceil(u64::from(target.height))
        as u32)
        .max(y_start + 1)
        .min(source.height);
    let mut channels = [0_u32; 4];
    let mut foreground_pixels = 0_u32;
    let total_pixels = (x_end - x_start) * (y_end - y_start);
    for source_y in y_start..y_end {
        for source_x in x_start..x_end {
            let pixel = rgba_at(frame, source_x, source_y);
            if is_text_background(&pixel, background) {
                continue;
            }
            for (sum, channel) in channels.iter_mut().zip(pixel) {
                *sum += u32::from(channel);
            }
            foreground_pixels += 1;
        }
    }
    if foreground_pixels == 0 {
        return TextSample {
            color: background,
            coverage: 0.0,
        };
    }
    TextSample {
        color: channels.map(|sum| (sum / foreground_pixels) as u8),
        coverage: foreground_pixels as f32 / total_pixels as f32,
    }
}

fn is_text_background(pixel: &[u8], background: [u8; 4]) -> bool {
    pixel[..3]
        .iter()
        .zip(background[..3].iter())
        .map(|(channel, background)| u32::from(channel.abs_diff(*background)).pow(2))
        .sum::<u32>()
        <= TEXT_BACKGROUND_DISTANCE_SQUARED
}

fn luminance(pixel: &[u8]) -> u16 {
    (u16::from(pixel[0]) * 54 + u16::from(pixel[1]) * 183 + u16::from(pixel[2]) * 19) / 256
}

fn contrasting_foreground(mut color: [u8; 4], background: [u8; 4]) -> [u8; 4] {
    let foreground_luminance = luminance(&color);
    let background_luminance = luminance(&background);
    if foreground_luminance.abs_diff(background_luminance) >= TEXT_MIN_CONTRAST {
        return color;
    }
    let target = if background_luminance < 128 {
        (background_luminance + TEXT_MIN_CONTRAST).min(255)
    } else {
        background_luminance.saturating_sub(TEXT_MIN_CONTRAST)
    };
    if foreground_luminance == 0 {
        color[..3].fill(target as u8);
    } else {
        let scale = f32::from(target) / f32::from(foreground_luminance);
        for channel in &mut color[..3] {
            *channel = (f32::from(*channel) * scale).round().clamp(0.0, 255.0) as u8;
        }
    }
    color
}

fn rgba_at(frame: &RgbaFrame, x: u32, y: u32) -> [u8; 4] {
    let index = (y as usize * frame.size().width as usize + x as usize) * 4;
    frame.pixels()[index..index + 4]
        .try_into()
        .expect("validated RGBA frame has complete pixels")
}

fn sampled_rgba(frame: &RgbaFrame, x: u32, y: u32, target: PixelSize) -> [u8; 4] {
    let source = frame.size();
    let source_x = (u64::from(x) * u64::from(source.width) / u64::from(target.width)) as u32;
    let source_y = (u64::from(y) * u64::from(source.height) / u64::from(target.height)) as u32;
    rgba_at(
        frame,
        source_x.min(source.width - 1),
        source_y.min(source.height - 1),
    )
}

fn resized_rgb(frame: &RgbaFrame, target: PixelSize) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(target.width as usize * target.height as usize * 3);
    if frame.size() == target {
        for pixel in frame.pixels().chunks_exact(4) {
            rgb.extend_from_slice(&pixel[..3]);
        }
        return rgb;
    }
    for y in 0..target.height {
        for x in 0..target.width {
            let pixel = sampled_rgba(frame, x, y, target);
            rgb.extend_from_slice(&pixel[..3]);
        }
    }
    rgb
}

fn render_escape(symbol: &str, frame_area: Rect, area: Rect, buffer: &mut Buffer) {
    if frame_area.width > area.width || frame_area.height > area.height {
        return;
    }
    let render_area = Rect::new(area.x, area.y, frame_area.width, frame_area.height);
    if let Some(cell) = buffer.cell_mut((render_area.x, render_area.y)) {
        cell.set_symbol(symbol);
    }
    let mut first = true;
    for y in render_area.top()..render_area.bottom() {
        for x in render_area.left()..render_area.right() {
            if first {
                first = false;
            } else if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_skip(true);
            }
        }
    }
}

fn tmux_escape(is_tmux: bool) -> (&'static str, &'static str, &'static str) {
    if is_tmux {
        ("\x1bPtmux;", "\x1b\x1b", "\x1b\\")
    } else {
        ("", "\x1b", "")
    }
}

fn wrap_tmux(sequence: &str) -> String {
    format!("\x1bPtmux;{}\x1b\\", sequence.replace('\x1b', "\x1b\x1b"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(size: PixelSize) -> Arc<RgbaFrame> {
        Arc::new(RgbaFrame::new(size, [40, 80, 120, 255]))
    }

    fn context() -> PresentationContext {
        PresentationContext {
            cells: Rect::new(0, 0, 8, 4),
            font_size: (10, 20),
            is_tmux: false,
            kitty_image_id: 42,
        }
    }

    #[test]
    fn protocols_request_aspect_correct_raster_resolutions() {
        assert_eq!(
            DisplayProtocol::Halfblocks.target_size(context().cells, (10, 20)),
            PixelSize::new(8, 8).ok()
        );
        assert_eq!(
            DisplayProtocol::Ascii.target_size(context().cells, (10, 20)),
            PixelSize::new(16, 16).ok()
        );
        assert_eq!(
            DisplayProtocol::Braille.target_size(context().cells, (10, 20)),
            PixelSize::new(16, 16).ok()
        );
        assert_eq!(
            DisplayProtocol::Kitty.target_size(context().cells, (10, 20)),
            PixelSize::new(80, 80).ok()
        );
        assert_eq!(
            DisplayProtocol::Iterm2.target_size(context().cells, (10, 20)),
            PixelSize::new(40, 40).ok()
        );
    }

    #[test]
    fn text_protocol_cycle_and_density_ramp_include_braille() {
        assert_eq!(DisplayProtocol::Halfblocks.next(), DisplayProtocol::Braille);
        assert_eq!(DisplayProtocol::Braille.next(), DisplayProtocol::Ascii);
        assert!(ASCII_RAMP.len() > 60);
        assert_eq!(ASCII_RAMP.first(), Some(&b'.'));
        assert_eq!(ASCII_RAMP.last(), Some(&b'$'));
    }

    #[test]
    fn display_protocol_names_parse_from_their_canonical_values() {
        for name in DisplayProtocol::NAMES {
            let protocol = name.parse::<DisplayProtocol>().unwrap();
            assert_eq!(protocol.as_str(), *name);
            assert_eq!(protocol.to_string(), *name);
        }
        assert_eq!(" ITERM ".parse(), Ok(DisplayProtocol::Iterm2));
        assert!("png".parse::<DisplayProtocol>().is_err());
    }

    #[test]
    fn ascii_target_size_tracks_terminal_cell_aspect_ratio() {
        let cells = context().cells;
        assert_eq!(
            DisplayProtocol::Ascii.target_size(cells, (8, 16)),
            PixelSize::new(16, 16).ok()
        );
        assert_eq!(
            DisplayProtocol::Ascii.target_size(cells, (10, 15)),
            PixelSize::new(16, 12).ok()
        );
        assert_eq!(
            DisplayProtocol::Braille.target_size(cells, (10, 15)),
            PixelSize::new(16, 12).ok()
        );
        assert_eq!(
            DisplayProtocol::Ascii.target_size(cells, (10, 10)),
            PixelSize::new(16, 8).ok()
        );
    }

    #[test]
    fn ascii_separates_background_and_strengthens_low_contrast_faces() {
        let size = PixelSize::new(4, 4).unwrap();
        let background = [20, 24, 32, 255];
        let mut source = RgbaFrame::new(size, background);
        for y in 0..4 {
            for x in 2..4 {
                let index = (y * 4 + x) * 4;
                source.pixels_mut()[index..index + 4].copy_from_slice(&[38, 50, 62, 255]);
            }
        }
        let encoded = encode_ascii(&source, Rect::new(0, 0, 2, 1));

        assert_eq!(encoded.cells[0].symbol, ' ');
        assert_eq!(encoded.cells[0].background, Color::Rgb(20, 24, 32));
        assert_ne!(encoded.cells[1].symbol, ' ');
        let Color::Rgb(red, green, blue) = encoded.cells[1].foreground else {
            panic!("ASCII model cell should use an RGB foreground");
        };
        assert!(luminance(&[red, green, blue, 255]) >= 90);
    }

    #[test]
    fn braille_maps_rgba_samples_to_unicode_dot_positions() {
        let background = [20, 24, 32, 255];
        let mut source = RgbaFrame::new(PixelSize::new(2, 4).unwrap(), background);
        for (x, y) in [(0, 0), (1, 1)] {
            let index = (y * 2 + x) * 4;
            source.pixels_mut()[index..index + 4].copy_from_slice(&[135, 180, 220, 255]);
        }

        let encoded = encode_braille(&source, Rect::new(0, 0, 1, 1));

        assert_eq!(encoded.cells[0].symbol, '\u{2811}');
        assert_eq!(encoded.cells[0].background, Color::Rgb(20, 24, 32));
        assert_eq!(encoded.cells[0].foreground, Color::Rgb(135, 180, 220));
    }

    #[test]
    fn text_tone_curve_stretches_a_narrow_luminance_range() {
        let background = [20, 20, 20, 255];
        let mut source = RgbaFrame::new(PixelSize::new(4, 4).unwrap(), background);
        source.pixels_mut()[20..24].copy_from_slice(&[80, 80, 80, 255]);
        source.pixels_mut()[24..28].copy_from_slice(&[120, 120, 120, 255]);

        let curve = TextToneCurve::from_frame(&source, background);

        assert_eq!(curve.low, 80);
        assert_eq!(curve.high, 120);
        assert_eq!(curve.map(80), 0.0);
        assert!((curve.map(100) - 0.5).abs() < f32::EPSILON);
        assert_eq!(curve.map(120), 1.0);
    }

    #[test]
    fn braille_uses_ordered_dither_for_interior_face_tones() {
        let background = [20, 24, 32, 255];
        let mut source = RgbaFrame::new(PixelSize::new(6, 12).unwrap(), background);
        for y in 1..11 {
            for x in 1..5 {
                let color = if x <= 2 {
                    [60, 80, 100, 255]
                } else {
                    [135, 180, 220, 255]
                };
                let index = (y * 6 + x) * 4;
                source.pixels_mut()[index..index + 4].copy_from_slice(&color);
            }
        }

        let encoded = encode_braille(&source, Rect::new(0, 0, 3, 3));
        let center = &encoded.cells[4];

        assert_eq!(center.symbol, '\u{28b9}');
    }

    #[test]
    fn tone_curve_falls_back_when_a_dominant_face_collapses_percentiles() {
        let background = [20, 20, 20, 255];
        let mut source = RgbaFrame::new(PixelSize::new(12, 12).unwrap(), background);
        for y in 1..11 {
            for x in 1..11 {
                let index = (y * 12 + x) * 4;
                source.pixels_mut()[index..index + 4].copy_from_slice(&[120, 120, 120, 255]);
            }
        }
        source.pixels_mut()[52..56].copy_from_slice(&[60, 60, 60, 255]);

        let curve = TextToneCurve::from_frame(&source, background);

        assert_eq!(curve.low, 60);
        assert_eq!(curve.high, 120);
        assert_eq!(curve.map(60), 0.0);
        assert_eq!(curve.map(120), 1.0);
    }

    #[test]
    fn braille_tone_strength_spans_one_to_eight_dots() {
        let dot_count = |tone: f32| {
            let strength = BRAILLE_MIN_STRENGTH + tone * (1.0 - BRAILLE_MIN_STRENGTH);
            BRAILLE_DOTS
                .iter()
                .filter(|(_, _, _, level)| strength >= (f32::from(*level) + 0.5) / 8.0)
                .count()
        };

        assert_eq!(dot_count(0.0), 1);
        assert_eq!(dot_count(0.5), 5);
        assert_eq!(dot_count(1.0), 8);
    }

    #[test]
    fn latest_channel_coalesces_pending_frames() {
        let (sender, receiver) = latest_channel();
        sender.send(1).unwrap();
        sender.send(2).unwrap();
        sender.send(3).unwrap();
        assert_eq!(receiver.recv(), Ok(3));
    }

    #[test]
    fn clear_invalidates_an_old_encoder_response() {
        let mut presenter = TerminalPresenter::new(DisplayProtocol::Halfblocks, context());
        let stale_generation = presenter.generation;
        let stale_sequence = presenter.sequence;

        presenter.clear();
        let result = presenter.apply_response(EncodeResponse {
            sequence: stale_sequence,
            generation: stale_generation,
            protocol: DisplayProtocol::Halfblocks,
            frame: Err("stale encoder failure".to_string()),
            elapsed: Duration::ZERO,
        });

        assert!(matches!(result, Ok(None)));
        assert!(!presenter.has_front());
    }

    #[test]
    fn protocol_switch_invalidates_an_old_encoder_response() {
        let mut presenter = TerminalPresenter::new(DisplayProtocol::Halfblocks, context());
        let stale_generation = presenter.generation;

        presenter.set_protocol(DisplayProtocol::Ascii);
        let result = presenter.apply_response(EncodeResponse {
            sequence: presenter.sequence,
            generation: stale_generation,
            protocol: DisplayProtocol::Halfblocks,
            frame: Err("stale backend failure".to_string()),
            elapsed: Duration::ZERO,
        });

        assert!(matches!(result, Ok(None)));
        assert_eq!(presenter.protocol(), DisplayProtocol::Ascii);
    }

    #[test]
    fn text_backends_use_frame_background_metadata_when_corners_are_covered() {
        let background = [20, 24, 32, 255];
        let mut source = RgbaFrame::new(PixelSize::new(2, 2).unwrap(), background);
        source.pixels_mut().fill(160);

        let encoded = encode_ascii(&source, Rect::new(0, 0, 1, 1));

        assert_eq!(encoded.cells[0].background, Color::Rgb(20, 24, 32));
        assert_ne!(encoded.cells[0].symbol, ' ');
    }

    #[test]
    fn front_frame_remains_visible_while_new_frame_encodes() {
        let mut presenter = TerminalPresenter::new(DisplayProtocol::Halfblocks, context());
        presenter.submit(frame(PixelSize::new(8, 8).unwrap()));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !presenter.has_front() && Instant::now() < deadline {
            presenter.poll().unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(presenter.has_front());
        presenter.submit(frame(PixelSize::new(8, 8).unwrap()));
        assert!(presenter.has_front());
    }

    #[test]
    fn halfblocks_map_two_rgba_rows_to_one_terminal_row() {
        let encoded = encode_halfblocks(
            &frame(PixelSize::new(80, 80).unwrap()),
            Rect::new(0, 0, 8, 4),
        );
        assert_eq!((encoded.width, encoded.height), (8, 4));
        assert_eq!(encoded.cells.len(), 32);
    }

    #[test]
    fn every_backend_encodes_a_small_rgba_frame() {
        let source = frame(PixelSize::new(8, 8).unwrap());
        let context = PresentationContext {
            font_size: (1, 2),
            ..context()
        };
        for protocol in [
            DisplayProtocol::Kitty,
            DisplayProtocol::Sixel,
            DisplayProtocol::Iterm2,
            DisplayProtocol::Halfblocks,
            DisplayProtocol::Braille,
            DisplayProtocol::Ascii,
        ] {
            let encoded = encode_frame(protocol, &source, context)
                .unwrap_or_else(|error| panic!("{protocol:?} failed: {error}"));
            assert!(encoded.encoded_size().0 > 0);
        }
    }

    #[test]
    fn sixel_encoding_changes_when_pixels_change() {
        let encode = |offset: u32| {
            let size = PixelSize::new(64, 48).unwrap();
            let mut source = RgbaFrame::new(size, [20, 24, 32, 255]);
            for y in 8..32 {
                for x in offset..offset + 20 {
                    let index = (y * size.width + x) as usize * 4;
                    source.pixels_mut()[index..index + 4].copy_from_slice(&[135, 180, 220, 255]);
                }
            }
            encode_sixel(&source, size, context()).unwrap().data
        };

        assert_ne!(encode(8), encode(24));
    }

    #[test]
    fn kitty_cad_frame_meets_compression_and_encode_budget() {
        let size = PixelSize::new(640, 480).unwrap();
        let mut source = RgbaFrame::new(size, [20, 24, 32, 255]);
        for y in 100..380 {
            for x in 160..480 {
                let index = (y * 640 + x) * 4;
                source.pixels_mut()[index..index + 4].copy_from_slice(&[135, 180, 220, 255]);
            }
        }
        let context = PresentationContext {
            cells: Rect::new(0, 0, 64, 24),
            ..context()
        };
        let started = Instant::now();
        let encoded = encode_kitty(&source, size, context).unwrap();
        let budget = if cfg!(debug_assertions) {
            Duration::from_millis(250)
        } else {
            Duration::from_millis(30)
        };

        assert!(encoded.encoded_bytes < 640 * 480 * 3 / 10);
        assert!(started.elapsed() <= budget);
    }

    #[test]
    fn iterm2_cad_frame_uses_compact_low_latency_jpeg() {
        let source_size = PixelSize::new(640, 480).unwrap();
        let mut source = RgbaFrame::new(source_size, [20, 24, 32, 255]);
        for y in 100..380 {
            for x in 160..480 {
                let index = (y * 640 + x) * 4;
                source.pixels_mut()[index..index + 4].copy_from_slice(&[135, 180, 220, 255]);
            }
        }
        let context = PresentationContext {
            cells: Rect::new(0, 0, 64, 24),
            ..context()
        };
        let target = DisplayProtocol::Iterm2
            .target_size(context.cells, context.font_size)
            .unwrap();
        let started = Instant::now();
        let encoded = encode_iterm2(&source, target, context).unwrap();
        let budget = if cfg!(debug_assertions) {
            Duration::from_millis(250)
        } else {
            Duration::from_millis(30)
        };

        assert_eq!(target, PixelSize::new(320, 240).unwrap());
        assert!(encoded.data.len() < 640 * 480 * 3 / 10);
        assert!(started.elapsed() <= budget);
        assert!(encoded.data.contains("File=size="));
    }
}
