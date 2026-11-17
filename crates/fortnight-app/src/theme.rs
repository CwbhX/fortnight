//! Theme and shared styles. Owner: agent E.
//!
//! Follows the system light/dark theme. One accent colour, `#ea580c`.
//! Generous spacing, no borders where whitespace will do. Other views use
//! the constants and style functions here so the app looks like one thing.

use iced::{Color, Theme};

/// The accent colour, `#ea580c`.
pub const ACCENT: Color = Color::from_rgb(
    0xea as f32 / 255.0,
    0x58 as f32 / 255.0,
    0x0c as f32 / 255.0,
);

/// Standard spacing unit, in logical pixels.
pub const SPACING: f32 = 12.0;

/// The app theme for light (`dark = false`) or dark mode, with the accent.
/// `main.rs` picks `dark` from the system setting.
pub fn theme_for(dark: bool) -> Theme {
    if dark { Theme::Dark } else { Theme::Light }
}
