use crate::{RenderError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelSize {
    pub width: u32,
    pub height: u32,
}

impl PixelSize {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(RenderError::InvalidPixelSize { width, height });
        }
        Self::rgba_len(width, height)?;
        Ok(Self { width, height })
    }

    pub fn aspect_ratio(self) -> f32 {
        self.width as f32 / self.height as f32
    }

    fn rgba_len(width: u32, height: u32) -> Result<usize> {
        let pixels = (width as usize)
            .checked_mul(height as usize)
            .and_then(|count| count.checked_mul(4))
            .ok_or(RenderError::PixelBufferOverflow { width, height })?;
        Ok(pixels)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaFrame {
    size: PixelSize,
    pixels: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Framebuffer {
    color: RgbaFrame,
    depth: Vec<f32>,
}

impl Framebuffer {
    pub fn new(size: PixelSize, color: [u8; 4]) -> Self {
        let pixel_count = size.width as usize * size.height as usize;
        Self {
            color: RgbaFrame::new(size, color),
            depth: vec![f32::INFINITY; pixel_count],
        }
    }

    pub fn size(&self) -> PixelSize {
        self.color.size()
    }

    pub fn clear(&mut self, color: [u8; 4]) {
        self.color.clear(color);
        self.depth.fill(f32::INFINITY);
    }

    pub fn write_pixel(&mut self, x: u32, y: u32, depth: f32, color: [u8; 4]) -> bool {
        let size = self.size();
        if x >= size.width || y >= size.height || !depth.is_finite() {
            return false;
        }
        let index = y as usize * size.width as usize + x as usize;
        if depth >= self.depth[index] {
            return false;
        }
        self.depth[index] = depth;
        self.color.pixels_mut()[index * 4..index * 4 + 4].copy_from_slice(&color);
        true
    }

    pub fn depth_at(&self, x: u32, y: u32) -> Option<f32> {
        let size = self.size();
        (x < size.width && y < size.height)
            .then(|| self.depth[y as usize * size.width as usize + x as usize])
    }

    pub fn into_color(self) -> RgbaFrame {
        self.color
    }
}

impl RgbaFrame {
    pub fn new(size: PixelSize, color: [u8; 4]) -> Self {
        let mut pixels = vec![
            0;
            PixelSize::rgba_len(size.width, size.height)
                .expect("validated PixelSize cannot overflow")
        ];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&color);
        }
        Self { size, pixels }
    }

    pub fn size(&self) -> PixelSize {
        self.size
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    pub fn clear(&mut self, color: [u8; 4]) {
        for pixel in self.pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_size_rejects_zero_dimensions() {
        assert_eq!(
            PixelSize::new(0, 10),
            Err(RenderError::InvalidPixelSize {
                width: 0,
                height: 10
            })
        );
    }

    #[test]
    fn frame_allocates_and_clears_rgba_pixels() {
        let mut frame = RgbaFrame::new(PixelSize::new(2, 3).unwrap(), [1, 2, 3, 4]);
        assert_eq!(frame.pixels().len(), 24);
        assert!(frame
            .pixels()
            .chunks_exact(4)
            .all(|pixel| pixel == [1, 2, 3, 4]));

        frame.clear([5, 6, 7, 8]);
        assert!(frame
            .pixels()
            .chunks_exact(4)
            .all(|pixel| pixel == [5, 6, 7, 8]));
    }

    #[test]
    fn framebuffer_depth_test_keeps_the_nearest_pixel() {
        let mut buffer = Framebuffer::new(PixelSize::new(2, 2).unwrap(), [0, 0, 0, 255]);
        assert!(buffer.write_pixel(1, 1, 0.8, [10, 0, 0, 255]));
        assert!(!buffer.write_pixel(1, 1, 0.9, [20, 0, 0, 255]));
        assert!(buffer.write_pixel(1, 1, 0.2, [30, 0, 0, 255]));
        assert_eq!(buffer.depth_at(1, 1), Some(0.2));
        let frame = buffer.into_color();
        assert_eq!(&frame.pixels()[12..16], &[30, 0, 0, 255]);
    }
}
