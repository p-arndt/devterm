//! The [`Renderer`](crate::Renderer)'s method implementations, split by responsibility:
//! `init` owns surface/device/pipeline setup plus resize, DPI and font-size state
//! transitions, while `draw` owns per-frame snapshot rendering.

mod draw;
mod init;
