//! Screen capture and frame dump for testing capture backends.

mod dump;

#[cfg(windows)]
pub(crate) mod d3d11_duplication;

#[cfg(windows)]
pub use d3d11_duplication::dxgi_output_desktop_rect_for_global_index;

#[cfg(windows)]
pub(crate) mod windows;

use std::path::Path;
#[cfg(windows)]
use std::time::Duration;

use anyhow::Result;
#[cfg(windows)]
use tracing::{debug, warn};

pub use dump::{prepare_dump_dir, write_frame_as_bmp};

/// Pixel format of captured frame data.
#[derive(Clone, Copy, Debug)]
pub enum PixelFormat {
    Bgra8,
}

/// A single captured frame: dimensions, stride, format, and raw pixel data.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtyRect {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveRect {
    pub source_x: u32,
    pub source_y: u32,
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

#[derive(Clone, Debug, Default)]
pub struct FrameMetadata {
    pub dirty_rects: Vec<DirtyRect>,
    pub move_rects: Vec<MoveRect>,
    pub accumulated_frames: u32,
    pub rects_coalesced: bool,
}

/// Result of a capture attempt -- distinguishes timeout from real errors.
pub enum CaptureResult {
    /// A new frame was captured successfully.
    Frame(Frame),
    /// No new frame was available (desktop unchanged). Not an error --
    /// the caller should reuse the previous frame.
    Timeout,
    /// The DXGI duplication was invalidated (desktop switch, mode change).
    /// The backend must be recreated.
    AccessLost,
}

/// Backend that can capture screen frames.
pub trait CaptureBackend: Send {
    fn capture_frame(&mut self) -> Result<Frame>;
    /// Try to capture; returns `CaptureResult` so the caller can distinguish
    /// a harmless timeout from a real failure.
    fn try_capture_frame(&mut self) -> Result<CaptureResult>;
    fn set_capture_source_index(&mut self, _index: usize) -> Result<()> {
        anyhow::bail!("capture output switching is not supported by this backend")
    }
}

/// Runs the capture-dump loop: capture frames and write them as BMPs to `output_dir`.
/// Respects `fps` (sleep between frames), optional `duration_secs` and `max_frames`.
pub fn run_capture_dump(
    output_dir: &Path,
    fps: u32,
    duration_secs: Option<u64>,
    max_frames: Option<u64>,
) -> Result<()> {
    #[cfg(not(windows))]
    {
        let _ = (output_dir, fps, duration_secs, max_frames);
        anyhow::bail!("capture-dump is only supported on Windows");
    }

    #[cfg(windows)]
    {
        run_capture_dump_impl(output_dir, fps, duration_secs, max_frames)
    }
}

#[cfg(windows)]
fn run_capture_dump_impl(
    output_dir: &Path,
    fps: u32,
    duration_secs: Option<u64>,
    max_frames: Option<u64>,
) -> Result<()> {
    use std::time::Instant;

    prepare_dump_dir(output_dir)?;
    let mut backend =
        windows::DxgiBackend::new().map_err(|e| anyhow::anyhow!("DXGI init: {}", e))?;

    let interval = if fps > 0 {
        Duration::from_secs_f64(1.0 / fps as f64)
    } else {
        Duration::from_millis(100)
    };

    let deadline = duration_secs.map(|s| Instant::now() + Duration::from_secs(s));
    let mut frame_count: u64 = 0;

    debug!(
        output_dir = %output_dir.display(),
        fps = fps,
        "capture-dump started (Ctrl+C to stop)"
    );

    loop {
        if let Some(max) = max_frames {
            if frame_count >= max {
                debug!(frame_count, "max_frames reached");
                break;
            }
        }
        if let Some(d) = deadline {
            if Instant::now() >= d {
                debug!(frame_count, "duration reached");
                break;
            }
        }

        match backend.capture_frame() {
            Ok(frame) => {
                let name = format!("frame_{:06}.bmp", frame_count);
                let path = output_dir.join(&name);
                if let Err(e) = write_frame_as_bmp(&frame, &path) {
                    warn!(path = %path.display(), error = %e, "failed to write BMP");
                } else {
                    debug!(path = %path.display(), frame_count, "wrote capture-dump frame");
                }
                frame_count += 1;
            }
            Err(e) => {
                warn!(error = %e, "capture_frame failed");
            }
        }

        std::thread::sleep(interval);
    }

    debug!(frame_count, "capture-dump finished");
    Ok(())
}
