use std::fmt::Write as FmtWrite;
use std::io::Write as IoWrite;

use base64::{engine::general_purpose, Engine};
use flate2::{write::ZlibEncoder, Compression};
use image::DynamicImage;
use ratatui::{buffer::Buffer, layout::Rect};
use ratatui_image::picker::cap_parser::Parser;

const KITTY_CHUNK_BYTES: usize = 3072;

pub(crate) struct CompressedKittyProtocol {
    transmit: Option<String>,
    placement: String,
    area: Rect,
    encoded_bytes: usize,
}

impl CompressedKittyProtocol {
    pub(crate) fn encode(
        image: &DynamicImage,
        area: Rect,
        image_id: u32,
        is_tmux: bool,
    ) -> Result<Self, String> {
        let rgb = image.to_rgb8();
        let compressed = compress_rgb(rgb.as_raw())?;
        let transmit = transmit_rgb_zlib(
            &compressed,
            rgb.width(),
            rgb.height(),
            area,
            image_id,
            is_tmux,
        );
        let encoded_bytes = transmit.len();
        let placement = placement_sequence(area, image_id, is_tmux);
        Ok(Self {
            transmit: Some(transmit),
            placement,
            area,
            encoded_bytes,
        })
    }

    pub(crate) fn encoded_bytes(&self) -> usize {
        self.encoded_bytes
    }

    pub(crate) fn render(&mut self, area: Rect, buffer: &mut Buffer) {
        if self.area.width > area.width || self.area.height > area.height {
            return;
        }
        let render_area = Rect::new(area.x, area.y, self.area.width, self.area.height);
        let symbol = self
            .transmit
            .take()
            .unwrap_or_else(|| self.placement.clone());
        buffer
            .cell_mut(render_area)
            .map(|cell| cell.set_symbol(&symbol));

        let mut first = true;
        for y in render_area.top()..render_area.bottom() {
            for x in render_area.left()..render_area.right() {
                if first {
                    first = false;
                } else {
                    buffer.cell_mut((x, y)).map(|cell| cell.set_skip(true));
                }
            }
        }
    }
}

fn compress_rgb(rgb: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(rgb).map_err(|error| error.to_string())?;
    encoder.finish().map_err(|error| error.to_string())
}

fn transmit_rgb_zlib(
    compressed: &[u8],
    width: u32,
    height: u32,
    area: Rect,
    image_id: u32,
    is_tmux: bool,
) -> String {
    let (start, escape, end) = Parser::escape_tmux(is_tmux);
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
                "_Gq=2,i={image_id},p=1,a=T,f=24,t=d,o=z,s={width},v={height},c={},r={},C=1,m={more};{payload}",
                area.width, area.height
            )
            .expect("writing to String cannot fail");
        } else {
            let more = usize::from(index + 1 < chunk_count);
            write!(data, "_Gq=2,m={more};{payload}").expect("writing to String cannot fail");
        }
        data.push_str(escape);
        data.push('\\');
    }
    data.push_str(end);
    data
}

fn placement_sequence(area: Rect, image_id: u32, is_tmux: bool) -> String {
    let (start, escape, end) = Parser::escape_tmux(is_tmux);
    format!(
        "{start}{escape}_Gq=2,i={image_id},p=1,a=p,c={},r={},C=1;{escape}\\{end}",
        area.width, area.height
    )
}

#[cfg(test)]
mod tests {
    use super::{compress_rgb, CompressedKittyProtocol};
    use flate2::read::ZlibDecoder;
    use image::{DynamicImage, Rgba, RgbaImage};
    use ratatui::{buffer::Buffer, layout::Rect};
    use std::io::Read;
    use std::time::{Duration, Instant};

    #[test]
    fn compressed_rgb_round_trips_without_losing_color_data() {
        let rgb: Vec<u8> = (0..4096).map(|index| (index * 31) as u8).collect();
        let compressed = compress_rgb(&rgb).unwrap();
        let mut decoded = Vec::new();
        ZlibDecoder::new(compressed.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, rgb);
    }

    #[test]
    fn cad_like_frame_compresses_below_raw_transfer_baseline() {
        let mut image = RgbaImage::from_pixel(640, 480, Rgba([20, 24, 32, 255]));
        for y in 100..380 {
            for x in 160..480 {
                image.put_pixel(x, y, Rgba([135, 180, 220, 255]));
            }
        }
        let image = DynamicImage::ImageRgba8(image);
        let raw_rgb_bytes = 640 * 480 * 3;
        let started = Instant::now();
        let protocol =
            CompressedKittyProtocol::encode(&image, Rect::new(0, 0, 64, 24), 42, false).unwrap();
        let elapsed = started.elapsed();
        let budget = if cfg!(debug_assertions) {
            Duration::from_millis(250)
        } else {
            Duration::from_millis(30)
        };

        assert!(protocol.encoded_bytes() < raw_rgb_bytes / 10);
        assert!(
            elapsed <= budget,
            "compressed Kitty encoding took {elapsed:?}, budget is {budget:?}"
        );
    }

    #[test]
    fn transmission_uses_rgb_zlib_and_a_stable_placement() {
        let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(4, 3, Rgba([10, 20, 30, 255])));
        let protocol =
            CompressedKittyProtocol::encode(&image, Rect::new(0, 0, 8, 5), 42, false).unwrap();
        let transmit = protocol.transmit.as_deref().unwrap();

        assert!(transmit.contains("i=42,p=1,a=T,f=24,t=d,o=z,s=4,v=3,c=8,r=5"));
        assert!(protocol.placement.contains("i=42,p=1,a=p,c=8,r=5"));
    }

    #[test]
    fn render_transmits_once_then_reuses_the_same_placement() {
        let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(4, 3, Rgba([10, 20, 30, 255])));
        let area = Rect::new(2, 1, 8, 5);
        let mut protocol = CompressedKittyProtocol::encode(&image, area, 42, false).unwrap();
        let mut first = Buffer::empty(Rect::new(0, 0, 20, 10));
        protocol.render(area, &mut first);
        assert!(first[(2, 1)].symbol().contains("a=T"));
        assert!(first[(3, 1)].skip);

        let mut second = Buffer::empty(Rect::new(0, 0, 20, 10));
        protocol.render(area, &mut second);
        assert!(second[(2, 1)].symbol().contains("a=p"));
        assert!(!second[(2, 1)].symbol().contains("a=T"));
    }
}
