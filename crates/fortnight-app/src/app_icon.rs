//! The app icon (`assets/icon-light.svg`, `assets/icon-dark.svg`), drawn at
//! run time and shown where the platform shows app icons: the Dock on macOS
//! (which ignores window icons), the window and taskbar icon elsewhere.
//!
//! The `.app` bundle's own icon (`cargo xtask bundle`) is always the light
//! one; the running app replaces it in the Dock.

use fortnight_core::domain::AppIcon;
use iced::Task;
use resvg::{tiny_skia, usvg};

const LIGHT: &[u8] = include_bytes!("../../../assets/icon-light.svg");
const DARK: &[u8] = include_bytes!("../../../assets/icon-dark.svg");

/// Whether to show the dark icon. `system_dark` is the system appearance,
/// `None` while unknown (then light).
pub fn is_dark(choice: AppIcon, system_dark: Option<bool>) -> bool {
    match choice {
        AppIcon::Light => false,
        AppIcon::Dark => true,
        AppIcon::System => system_dark.unwrap_or(false),
    }
}

/// Shows the light or dark icon.
pub fn apply<T: Send + 'static>(dark: bool) -> Task<T> {
    #[cfg(target_os = "macos")]
    {
        macos::set_dock_icon(dark);
        Task::none()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let Some(icon) = window_icon(dark) else {
            return Task::none();
        };
        iced::window::latest().and_then(move |id| iced::window::set_icon(id, icon.clone()))
    }
}

/// Draws the icon `size` pixels square, leaving out `inset` SVG units on
/// each side of the 1024-unit canvas. Premultiplied RGBA.
fn render(dark: bool, size: u32, inset: f32) -> Option<tiny_skia::Pixmap> {
    let svg = if dark { DARK } else { LIGHT };
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
    let scale = size as f32 / (tree.size().width() - 2.0 * inset);
    let transform = tiny_skia::Transform::from_translate(-inset, -inset).post_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some(pixmap)
}

/// Windows and Linux: the macOS canvas has a wide margin for the Dock's
/// grid; trim most of it so the icon fills the title bar and taskbar slot.
#[cfg(not(target_os = "macos"))]
fn window_icon(dark: bool) -> Option<iced::window::Icon> {
    const SIZE: u32 = 256;
    let pixmap = render(dark, SIZE, 64.0)?;
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    iced::window::icon::from_rgba(rgba, SIZE, SIZE).ok()
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    /// Sets the Dock icon for as long as the app runs. Does nothing off the
    /// main thread (AppKit requires it; iced runs `update` there).
    pub fn set_dock_icon(dark: bool) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(png) = super::render(dark, 1024, 0.0).and_then(|p| p.encode_png().ok()) else {
            return;
        };
        let data = NSData::with_bytes(&png);
        let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: on the main thread, with a valid image.
        unsafe { app.setApplicationIconImage(Some(&image)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_and_system_appearance() {
        assert!(!is_dark(AppIcon::System, None));
        assert!(!is_dark(AppIcon::System, Some(false)));
        assert!(is_dark(AppIcon::System, Some(true)));
        assert!(is_dark(AppIcon::Dark, Some(false)));
        assert!(!is_dark(AppIcon::Light, Some(true)));
    }

    #[test]
    fn both_icons_render() {
        for dark in [false, true] {
            let pixmap = render(dark, 64, 0.0).expect("the icon renders");
            // Transparent corner, opaque centre.
            assert_eq!(pixmap.pixel(0, 0).unwrap().alpha(), 0);
            assert_eq!(pixmap.pixel(32, 32).unwrap().alpha(), 255);
        }
    }
}
