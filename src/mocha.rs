//! Catppuccin Mocha accent colours, for what the SDK `Theme` doesn't cover
//! (its default is already Mocha's base, surfaces and text).
//! <https://catppuccin.com/palette>

use waybar_gpui::gpui::{Rgba, rgb};

pub fn green() -> Rgba {
    rgb(0xa6e3a1)
}

pub fn yellow() -> Rgba {
    rgb(0xf9e2af)
}

pub fn red() -> Rgba {
    rgb(0xf38ba8)
}

pub fn overlay0() -> Rgba {
    rgb(0x6c7086)
}
