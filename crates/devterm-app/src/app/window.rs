//! Initial window placement and sizing.
//!
//! Winit's platform default window size is small and fixed, so instead the initial
//! window is sized as a fraction of the primary monitor (like Windows Terminal's
//! default launch size) and centered on it.

use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes};

/// Fraction of the monitor's width the window should occupy.
const WIDTH_FRACTION: f64 = 0.5;
/// Fraction of the monitor's height the window should occupy.
const HEIGHT_FRACTION: f64 = 0.55;
/// Floor on the initial window size, in physical pixels, for tiny/unusual monitors.
const MIN_SIZE: PhysicalSize<u32> = PhysicalSize::new(800, 600);

/// Build the window attributes for the app's window: titled and, when a monitor is
/// available, sized/centered per [`WIDTH_FRACTION`] and [`HEIGHT_FRACTION`].
pub fn initial_attributes(event_loop: &ActiveEventLoop, title: &str) -> WindowAttributes {
    let attributes = Window::default_attributes().with_title(title);

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
