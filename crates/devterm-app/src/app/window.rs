//! Initial window placement and sizing.
//!
//! Winit's platform default window size is small and fixed, so instead the initial
//! window is sized as a fraction of the primary monitor (like Windows Terminal's
//! default launch size) and centered on it.

use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Icon, Window, WindowAttributes};

/// The app icon, baked into the binary so it's always available at runtime.
/// Regenerated (alongside `devterm.ico`) by `scripts/make-icon.ps1`.
const ICON_PNG: &[u8] = include_bytes!("../../../../assets/devterm-256.png");

/// Decode the embedded PNG into a winit [`Icon`] for the title bar / taskbar.
///
/// Winit paints a default icon unless given one; the icon embedded in the `.exe`
/// only covers Explorer/Start Menu/shortcuts, not the live window. Returns `None`
/// (and logs) on the practically-impossible decode failure so startup still proceeds.
fn window_icon() -> Option<Icon> {
    let image = match image::load_from_memory(ICON_PNG) {
        Ok(image) => image.into_rgba8(),
        Err(err) => {
            log::warn!("failed to decode window icon: {err}");
            return None;
        }
    };
    let (width, height) = image.dimensions();
    match Icon::from_rgba(image.into_raw(), width, height) {
        Ok(icon) => Some(icon),
        Err(err) => {
            log::warn!("failed to build window icon: {err}");
            None
        }
    }
}

/// Fraction of the monitor's width the window should occupy.
const WIDTH_FRACTION: f64 = 0.5;
/// Fraction of the monitor's height the window should occupy.
const HEIGHT_FRACTION: f64 = 0.55;
/// Floor on the initial window size, in physical pixels, for tiny/unusual monitors.
const MIN_SIZE: PhysicalSize<u32> = PhysicalSize::new(800, 600);

/// Build the window attributes for the app's window: titled and, when a monitor is
/// available, sized/centered per [`WIDTH_FRACTION`] and [`HEIGHT_FRACTION`].
pub fn initial_attributes(event_loop: &ActiveEventLoop, title: &str) -> WindowAttributes {
    // Borderless: DevTerm draws its own titlebar (tabs + window buttons) into the client
    // area like Windows Terminal, so the OS caption/frame is turned off. Resize borders and
    // window dragging are handled manually via winit's `drag_resize_window` / `drag_window`.
    let attributes = Window::default_attributes()
        .with_title(title)
        .with_decorations(false)
        .with_window_icon(window_icon());

    let Some(monitor) = event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next())
    else {
        return attributes;
    };

    let monitor_size = monitor.size();
    let width = ((monitor_size.width as f64) * WIDTH_FRACTION).round() as u32;
    let height = ((monitor_size.height as f64) * HEIGHT_FRACTION).round() as u32;
    let size = PhysicalSize::new(width.max(MIN_SIZE.width), height.max(MIN_SIZE.height));

    let x = monitor.position().x + ((monitor_size.width as i32) - size.width as i32) / 2;
    let y = monitor.position().y + ((monitor_size.height as i32) - size.height as i32) / 2;

    attributes
        .with_inner_size(size)
        .with_position(PhysicalPosition::new(x, y))
}
