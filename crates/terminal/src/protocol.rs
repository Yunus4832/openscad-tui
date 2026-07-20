use std::fmt::{self, Display};
use std::str::FromStr;

use openscad_render::PixelSize;
use ratatui::layout::Rect;

const TEXT_HORIZONTAL_SAMPLES: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayProtocol {
    Kitty,
    Sixel,
    Iterm2,
    Halfblocks,
    Braille,
    Ascii,
}

impl DisplayProtocol {
    pub const NAMES: &'static [&'static str] =
        &["kitty", "sixel", "iterm2", "halfblocks", "braille", "ascii"];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Kitty => "kitty",
            Self::Sixel => "sixel",
            Self::Iterm2 => "iterm2",
            Self::Halfblocks => "halfblocks",
            Self::Braille => "braille",
            Self::Ascii => "ascii",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Kitty => Self::Sixel,
            Self::Sixel => Self::Iterm2,
            Self::Iterm2 => Self::Halfblocks,
            Self::Halfblocks => Self::Braille,
            Self::Braille => Self::Ascii,
            Self::Ascii => Self::Kitty,
        }
    }

    pub fn target_size(self, cells: Rect, font_size: (u16, u16)) -> Option<PixelSize> {
        let (width, height) = match self {
            Self::Halfblocks => (u32::from(cells.width), u32::from(cells.height) * 2),
            Self::Braille | Self::Ascii => {
                text_target_size(cells, font_size, TEXT_HORIZONTAL_SAMPLES)
            }
            Self::Kitty | Self::Sixel => (
                u32::from(cells.width) * u32::from(font_size.0),
                u32::from(cells.height) * u32::from(font_size.1),
            ),
            // iTerm2 has no persistent image ID, so every animation frame is a complete image
            // upload. Half linear resolution cuts encoding and transport work to roughly 25%.
            Self::Iterm2 => (
                (u32::from(cells.width) * u32::from(font_size.0))
                    .div_ceil(2)
                    .max(u32::from(cells.width)),
                (u32::from(cells.height) * u32::from(font_size.1))
                    .div_ceil(2)
                    .max(u32::from(cells.height)),
            ),
        };
        PixelSize::new(width, height).ok()
    }
}

impl Display for DisplayProtocol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseDisplayProtocolError(String);

impl Display for ParseDisplayProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unsupported terminal display protocol {:?}",
            self.0
        )
    }
}

impl std::error::Error for ParseDisplayProtocolError {}

impl FromStr for DisplayProtocol {
    type Err = ParseDisplayProtocolError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "kitty" => Ok(Self::Kitty),
            "sixel" => Ok(Self::Sixel),
            "iterm2" | "iterm" => Ok(Self::Iterm2),
            "halfblocks" => Ok(Self::Halfblocks),
            "braille" => Ok(Self::Braille),
            "ascii" => Ok(Self::Ascii),
            _ => Err(ParseDisplayProtocolError(value.to_string())),
        }
    }
}

fn text_target_size(cells: Rect, font_size: (u16, u16), horizontal_samples: u32) -> (u32, u32) {
    let font_width = u32::from(font_size.0).max(1);
    let font_height = u32::from(font_size.1).max(1);
    (
        u32::from(cells.width) * horizontal_samples,
        (u32::from(cells.height) * font_height * horizontal_samples).div_ceil(font_width),
    )
}
