//! Media helpers for the Horcrux Console host encoder (horcrux-vm's
//! `console::hxc`).
//!
//! * [`x264`]: software H.264 through the system's libx264, loaded at run
//!   time (`dlopen`), so link adaptation can change the bitrate of a running
//!   encoder (`x264_encoder_reconfig`) and ask for a keyframe without
//!   restarting anything. Nodes without libx264 keep the FFmpeg program.
//! * [`yuv`]: packed RGB (QEMU scanout formats) to planar I420, with
//!   area-average scaling.
//! * [`decoder`]: H.264 decoding for the Horcrux desktop client through the
//!   operating system's own decoder: [`avcodec`] (the system's FFmpeg
//!   libraries, loaded at run time) on Linux and other Unix systems,
//!   `mfdec` (Media Foundation, DXVA where the GPU has it) on Windows,
//!   `vtdec` (VideoToolbox) on macOS. No codec library is shipped.
//! * [`flexfec`]: FlexFEC (flexfec-03) repair packets for the RTP video
//!   stream.
//!
//! This crate is built with full optimisation even in debug builds (see the
//! workspace `[profile.*.package.horcrux-media]`): it converts every frame.
#[cfg(unix)]
pub mod avcodec;
pub mod decoder;
pub mod flexfec;
#[cfg(windows)]
pub mod mfdec;
#[cfg(target_os = "macos")]
pub mod vtdec;
#[cfg(unix)]
pub mod x264;
pub mod yuv;
