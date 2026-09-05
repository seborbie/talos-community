//! VP8 encoding and IVF dump for capture-encode pipeline.

#[cfg(windows)]
mod dirty_rect_classifier;
mod ivf;
#[cfg(any(windows, target_os = "macos"))]
mod libyuv_convert;
#[cfg(windows)]
mod mf_h264;
#[cfg(any(windows, target_os = "macos"))]
mod vp8;

#[cfg(windows)]
use std::collections::{HashMap, VecDeque};
use std::env;
use std::path::Path;
use std::sync::mpsc;
#[cfg(windows)]
use std::sync::OnceLock;
#[cfg(windows)]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[cfg(windows)]
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

use anyhow::Result;
#[cfg(windows)]
use anyhow::{ensure, Context};
#[cfg(windows)]
use dirty_rect_classifier::{DirtyRectClassifier, DirtyRectClassifierFrameSummary};
#[cfg(windows)]
use talos_protocol::{
    build_display_atlas_h264, build_display_frame_begin, build_display_frame_end,
    build_display_keyframe, build_display_record, DisplayAtlasRect, DisplayStreamDescriptor,
    RemoteDesktopDisplayProfile, DISPLAY_ATLAS_H264_FLAG_KEYFRAME, DISPLAY_RECORD_MOVE_RECT,
    DISPLAY_STREAM_META_TYPE, DISPLAY_STREAM_MODE_LEGACY_CAPTURE,
    DISPLAY_STREAM_MODE_MODERN_CAPTURE, DISPLAY_STREAM_MODE_SCREENSHOT_ONLY,
    HELPER_PIPE_HANDSHAKE_MAGIC, HELPER_PIPE_MAX_AUTH_TOKEN_LEN, HELPER_PIPE_PROTOCOL_VERSION,
    REMOTE_DESKTOP_PROFILE_EXPERIMENTAL, REMOTE_DESKTOP_PROFILE_LEGACY,
    REMOTE_DESKTOP_PROFILE_MODERN_CPU, REMOTE_DESKTOP_PROFILE_MODERN_GPU,
    REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY,
};
#[cfg(not(windows))]
use talos_protocol::{
    DISPLAY_STREAM_MODE_LEGACY_CAPTURE, DISPLAY_STREAM_MODE_MODERN_CAPTURE,
    DISPLAY_STREAM_MODE_SCREENSHOT_ONLY,
};
#[cfg(windows)]
use tracing::{debug, warn};

pub use ivf::{build_header, IvfWriter};
#[cfg(any(windows, target_os = "macos"))]
pub use libyuv_convert::{bgra_bytes_to_i420, bgra_to_i420, bgra_to_i420_scaled, i420_to_nv12};
#[cfg(any(windows, target_os = "macos"))]
pub use vp8::{EncodeTuning, Vp8Encoder};

/// Scale factor for streaming encode: 1 = full resolution (1:1), 2 = half, 4 = quarter.
#[cfg(windows)]
const STREAM_ENCODE_SCALE: u32 = 1;
#[cfg(windows)]
const DIRTY_RECT_STATS_LOG_INTERVAL: u64 = 60;
#[cfg(windows)]
static H264_DIRTY_RECT_SUPPORTED: OnceLock<bool> = OnceLock::new();
#[cfg(windows)]
const MODERN_CAPTURE_STARTUP_FAILURE_LIMIT: u32 = 3;
#[cfg(windows)]
const MODERN_CAPTURE_RUNTIME_FAILURE_LOG_INTERVAL: u32 = 30;
#[cfg(windows)]
const CPU_SYNTH_TILE_SIZE: u32 = 32;
#[cfg(windows)]
const CPU_MOVE_ANCHOR_SIZE: u32 = 16;
#[cfg(windows)]
const CPU_MOVE_ANCHOR_STRIDE: u32 = 8;
#[cfg(windows)]
const CPU_MOVE_MAX_SIGNATURE_MATCHES: usize = 8;
#[cfg(windows)]
const CPU_MOVE_MAX_CHANGED_TILES_FOR_DETECTION: usize = 2048;
#[cfg(windows)]
const CPU_MOVE_MAX_SEED_TILES: usize = 128;
#[cfg(windows)]
const CPU_MOVE_MAX_RECTS_PER_FRAME: usize = 8;
#[cfg(windows)]
const CPU_MOVE_MIN_AREA: u32 = CPU_MOVE_ANCHOR_SIZE * CPU_MOVE_ANCHOR_SIZE;
#[cfg(windows)]
const CPU_SYNTH_MAX_DIRTY_RECTS: usize = 64;

#[cfg(windows)]
#[derive(Clone, Copy)]
enum DirtyRectClassifierState {
    Uninitialized,
    Disabled { reason: &'static str },
}

#[cfg(windows)]
impl DirtyRectClassifierState {
    fn skip_reason(self) -> Option<&'static str> {
        match self {
            Self::Uninitialized => None,
            Self::Disabled { reason } => Some(reason),
        }
    }
}

#[cfg(windows)]
struct DirtyRectClassifierContext {
    state: DirtyRectClassifierState,
    classifier: Option<DirtyRectClassifier>,
}

#[cfg(windows)]
impl DirtyRectClassifierContext {
    fn new() -> Self {
        Self {
            state: DirtyRectClassifierState::Uninitialized,
            classifier: None,
        }
    }

    fn reset(&mut self) {
        self.state = DirtyRectClassifierState::Uninitialized;
        self.classifier = None;
    }
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct DirtyRectFrameLogStats {
    frame_id: u64,
    dirty_rect_count: usize,
    move_rect_count: usize,
    raw_bytes: usize,
    compressed_bytes: usize,
    compress_time: Duration,
    total_time: Duration,
    accumulated_frames: u32,
    rects_coalesced: bool,
    keyframe: bool,
    synthetic_rects: bool,
}

#[cfg(windows)]
struct DirtyRectClassifierLogSummary {
    path: &'static str,
    classifier_backend: &'static str,
    dirty_rect_count: usize,
    classified_rect_count: usize,
    tile_count: usize,
    text_ui_rect_count: usize,
    photo_video_rect_count: usize,
    mixed_rect_count: usize,
    text_ui_tile_count: usize,
    photo_video_tile_count: usize,
    mixed_tile_count: usize,
    classifier_time: Duration,
    skip_reason: Option<&'static str>,
}

#[cfg(windows)]
impl DirtyRectClassifierLogSummary {
    fn skipped(path: &'static str, dirty_rect_count: usize, skip_reason: &'static str) -> Self {
        Self {
            path,
            classifier_backend: "skipped",
            dirty_rect_count,
            classified_rect_count: 0,
            tile_count: 0,
            text_ui_rect_count: 0,
            photo_video_rect_count: 0,
            mixed_rect_count: 0,
            text_ui_tile_count: 0,
            photo_video_tile_count: 0,
            mixed_tile_count: 0,
            classifier_time: Duration::ZERO,
            skip_reason: Some(skip_reason),
        }
    }

    fn gpu(path: &'static str, summary: DirtyRectClassifierFrameSummary) -> Self {
        Self {
            path,
            classifier_backend: "gpu_compute",
            dirty_rect_count: summary.dirty_rect_count,
            classified_rect_count: summary.classified_rect_count,
            tile_count: summary.tile_count,
            text_ui_rect_count: summary.text_ui_rect_count,
            photo_video_rect_count: summary.photo_video_rect_count,
            mixed_rect_count: summary.mixed_rect_count,
            text_ui_tile_count: summary.text_ui_tile_count,
            photo_video_tile_count: summary.photo_video_tile_count,
            mixed_tile_count: summary.mixed_tile_count,
            classifier_time: summary.classifier_time,
            skip_reason: None,
        }
    }
}

#[cfg(windows)]
fn should_log_dirty_rect_frame_stats(stats: DirtyRectFrameLogStats) -> bool {
    stats.frame_id <= 1
        || stats.frame_id.is_multiple_of(DIRTY_RECT_STATS_LOG_INTERVAL)
        || stats.accumulated_frames > 1
        || stats.rects_coalesced
        || stats.move_rect_count > 0
        || stats.dirty_rect_count.saturating_add(stats.move_rect_count) >= 16
        || stats.synthetic_rects
        || stats.total_time >= Duration::from_millis(12)
        || stats.compress_time >= Duration::from_millis(8)
}

#[cfg(windows)]
fn log_dirty_rect_frame_stats_channel(stats: DirtyRectFrameLogStats) {
    if !should_log_dirty_rect_frame_stats(stats) {
        return;
    }
    debug!(
        frame_id = stats.frame_id,
        kind = if stats.keyframe { "keyframe" } else { "delta" },
        dirty_rect_count = stats.dirty_rect_count,
        move_rect_count = stats.move_rect_count,
        raw_bytes = stats.raw_bytes,
        compressed_bytes = stats.compressed_bytes,
        compress_ms = stats.compress_time.as_secs_f64() * 1000.0,
        total_ms = stats.total_time.as_secs_f64() * 1000.0,
        accumulated_frames = stats.accumulated_frames,
        rects_coalesced = stats.rects_coalesced,
        synthetic_rects = stats.synthetic_rects,
        "dirty rect frame stats"
    );
}

#[cfg(windows)]
fn log_dirty_rect_frame_stats_pipe(pipe_name: &str, stats: DirtyRectFrameLogStats) {
    if !should_log_dirty_rect_frame_stats(stats) {
        return;
    }
    helper_io_log(
        "dirty_rect_frame_stats",
        serde_json::json!({
            "pipe_name": pipe_name,
            "frame_id": stats.frame_id,
            "kind": if stats.keyframe { "keyframe" } else { "delta" },
            "dirty_rect_count": stats.dirty_rect_count,
            "move_rect_count": stats.move_rect_count,
            "raw_bytes": stats.raw_bytes,
            "compressed_bytes": stats.compressed_bytes,
            "compress_ms": stats.compress_time.as_secs_f64() * 1000.0,
            "total_ms": stats.total_time.as_secs_f64() * 1000.0,
            "accumulated_frames": stats.accumulated_frames,
            "rects_coalesced": stats.rects_coalesced,
            "synthetic_rects": stats.synthetic_rects,
        }),
    );
}

#[cfg(windows)]
fn log_dirty_rect_classifier_summary_channel(summary: &DirtyRectClassifierLogSummary) {
    debug!(
        path = summary.path,
        classifier_backend = summary.classifier_backend,
        dirty_rect_count = summary.dirty_rect_count,
        classified_rect_count = summary.classified_rect_count,
        tile_count = summary.tile_count,
        text_ui_rect_count = summary.text_ui_rect_count,
        photo_video_rect_count = summary.photo_video_rect_count,
        mixed_rect_count = summary.mixed_rect_count,
        text_ui_tile_count = summary.text_ui_tile_count,
        photo_video_tile_count = summary.photo_video_tile_count,
        mixed_tile_count = summary.mixed_tile_count,
        classifier_ms = summary.classifier_time.as_secs_f64() * 1000.0,
        skip_reason = summary.skip_reason.unwrap_or(""),
        "dirty rect classifier summary"
    );
}

#[cfg(windows)]
fn log_dirty_rect_classifier_summary_pipe(
    pipe_name: &str,
    summary: &DirtyRectClassifierLogSummary,
) {
    helper_io_log(
        "dirty_rect_classifier_summary",
        serde_json::json!({
            "pipe_name": pipe_name,
            "path": summary.path,
            "classifier_backend": summary.classifier_backend,
            "dirty_rect_count": summary.dirty_rect_count,
            "classified_rect_count": summary.classified_rect_count,
            "tile_count": summary.tile_count,
            "text_ui_rect_count": summary.text_ui_rect_count,
            "photo_video_rect_count": summary.photo_video_rect_count,
            "mixed_rect_count": summary.mixed_rect_count,
            "text_ui_tile_count": summary.text_ui_tile_count,
            "photo_video_tile_count": summary.photo_video_tile_count,
            "mixed_tile_count": summary.mixed_tile_count,
            "classifier_ms": summary.classifier_time.as_secs_f64() * 1000.0,
            "skip_reason": summary.skip_reason.unwrap_or(""),
        }),
    );
}

#[cfg(windows)]
fn helper_log_details(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(key, value)| {
                let rendered = serde_json::to_string(value)
                    .unwrap_or_else(|_| "\"<unserializable>\"".to_string());
                format!("{key}={rendered}")
            })
            .collect::<Vec<_>>()
            .join(" "),
        other => {
            serde_json::to_string(other).unwrap_or_else(|_| "\"<unserializable>\"".to_string())
        }
    }
}

#[cfg(windows)]
fn helper_io_log(event: &str, mut data: serde_json::Value) {
    const TARGET: &str = "talos_worker_helper";

    // Attach session correlation fields if present (set by talos_worker_helper.exe).
    if let serde_json::Value::Object(ref mut map) = data {
        if let Ok(v) = std::env::var("RMM_HELPER_RMM_SESSION_ID") {
            if !v.trim().is_empty() {
                map.insert("rmm_session_id".to_string(), serde_json::Value::String(v));
            }
        }
        if let Ok(v) = std::env::var("RMM_HELPER_SESSION_SEQ") {
            if let Ok(n) = v.parse::<u64>() {
                map.insert(
                    "session_seq".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(n)),
                );
            }
        }
        if let Ok(v) = std::env::var("RMM_HELPER_PIPE_INSTANCE") {
            if let Ok(n) = v.parse::<u64>() {
                map.insert(
                    "pipe_instance".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(n)),
                );
            }
        }
    }

    let details = helper_log_details(&data);
    let is_warn = event.contains("_err") || event.contains("failed") || event.contains("missing");
    if details.is_empty() {
        if is_warn {
            warn!(target: TARGET, event = %event, "helper pipeline event");
        } else {
            debug!(target: TARGET, event = %event, "helper pipeline event");
        }
    } else if is_warn {
        warn!(target: TARGET, event = %event, details = %details, "helper pipeline event");
    } else {
        debug!(target: TARGET, event = %event, details = %details, "helper pipeline event");
    }
}

#[cfg(windows)]
fn log_dxgi_backend_init_failed(
    event: &'static str,
    pipe_name: Option<&str>,
    attempt: u32,
    error: &dyn std::fmt::Display,
) {
    if let Some(pipe_name) = pipe_name {
        warn!(
            log_event = event,
            pipe_name,
            attempt,
            error = %error,
            "dxgi backend init failed"
        );
    } else {
        warn!(
            log_event = event,
            attempt,
            error = %error,
            "dxgi backend init failed"
        );
    }
}

/// Chunk of IVF stream: metadata (debug info), header (32 bytes), or one frame (4 + 8 + payload).
#[derive(Clone)]
pub enum IvfChunk {
    /// JSON metadata sent before IVF header: bitrate, preset, cpu_used, encoding_fps, agent_monitor_hz.
    Metadata(Vec<u8>),
    Header([u8; 32]),
    Frame(Vec<u8>),
    DisplayKeyframe(Vec<u8>),
    DisplayDelta(Vec<u8>),
}

/// Sends IVF stream into a channel for an async task to forward over QUIC or relay.
/// Same byte layout as IvfWriter: header then per-frame 4-byte len + 8-byte pts + payload.
pub struct IvfStreamSender {
    tx: mpsc::Sender<IvfChunk>,
}

impl IvfStreamSender {
    pub fn new(tx: mpsc::Sender<IvfChunk>) -> Self {
        Self { tx }
    }

    /// Sends JSON metadata before header. Format: {"bitrate_kbps":u32,"preset":str,"cpu_used":i32,"encoding_fps":u32,"agent_monitor_hz":Option<u32>}
    pub fn write_metadata(&self, metadata: &[u8]) -> Result<()> {
        self.tx
            .send(IvfChunk::Metadata(metadata.to_vec()))
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    pub fn write_header(&self, width: u32, height: u32, fps: u32) -> Result<()> {
        let h = build_header(width, height, fps);
        self.tx
            .send(IvfChunk::Header(h))
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    pub fn write_frame(&self, payload: &[u8], pts: u64) -> Result<()> {
        let mut buf = Vec::with_capacity(4 + 8 + payload.len());
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(&pts.to_le_bytes());
        buf.extend_from_slice(payload);
        self.tx
            .send(IvfChunk::Frame(buf))
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    pub fn write_display_delta(&self, payload: &[u8]) -> Result<()> {
        self.tx
            .send(IvfChunk::DisplayDelta(payload.to_vec()))
            .map_err(|e| anyhow::anyhow!("{}", e))
    }
}

#[cfg(windows)]
struct PipeStreamSender {
    handle: winapi::um::winnt::HANDLE,
}

#[cfg(windows)]
impl PipeStreamSender {
    fn new(handle: winapi::um::winnt::HANDLE) -> Self {
        Self { handle }
    }

    fn write_chunk(&self, tag: u8, payload: &[u8]) -> Result<()> {
        let len = payload.len() as u32;
        write_all(self.handle, &[tag])?;
        write_all(self.handle, &len.to_le_bytes())?;
        write_all(self.handle, payload)?;
        Ok(())
    }

    fn write_metadata(&self, metadata: &[u8]) -> Result<()> {
        self.write_chunk(0, metadata)
    }

    fn write_header(&self, width: u32, height: u32, fps: u32) -> Result<()> {
        let h = build_header(width, height, fps);
        self.write_chunk(1, &h)
    }

    fn write_frame(&self, payload: &[u8], pts: u64) -> Result<()> {
        let mut buf = Vec::with_capacity(4 + 8 + payload.len());
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(&pts.to_le_bytes());
        buf.extend_from_slice(payload);
        self.write_chunk(2, &buf)
    }

    fn write_display_delta(&self, payload: &[u8]) -> Result<()> {
        self.write_chunk(5, payload)
    }
}

#[cfg(windows)]
fn write_all(handle: winapi::um::winnt::HANDLE, buf: &[u8]) -> Result<()> {
    use winapi::shared::minwindef::DWORD;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::fileapi::WriteFile;

    let mut written_total: usize = 0;
    while written_total < buf.len() {
        let mut written: DWORD = 0;
        let ok = unsafe {
            WriteFile(
                handle,
                buf[written_total..].as_ptr() as *const _,
                (buf.len() - written_total) as DWORD,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = unsafe { GetLastError() } as i32;
            anyhow::bail!("WriteFile failed: {}", err);
        }
        if written == 0 {
            anyhow::bail!("WriteFile wrote 0 bytes");
        }
        written_total += written as usize;
    }
    Ok(())
}

#[cfg(windows)]
fn open_named_pipe_writer(pipe_name: &str) -> Result<winapi::um::winnt::HANDLE> {
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::fileapi::{CreateFileW, OPEN_EXISTING};
    use winapi::um::handleapi::INVALID_HANDLE_VALUE;
    use winapi::um::winnt::{FILE_ATTRIBUTE_NORMAL, GENERIC_WRITE};

    let pipe_wide: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();
    for _ in 0..60 {
        let handle = unsafe {
            CreateFileW(
                pipe_wide.as_ptr(),
                GENERIC_WRITE,
                0,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            return Ok(handle);
        }
        let err = unsafe { GetLastError() };
        if err == winapi::shared::winerror::ERROR_PIPE_BUSY
            || err == winapi::shared::winerror::ERROR_FILE_NOT_FOUND
        {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        anyhow::bail!("CreateFileW pipe failed: {}", err);
    }
    anyhow::bail!("timed out connecting to pipe");
}

/// Encoder quality preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Grayscale,
    Low,
    Medium,
    High,
    Maximum,
}

impl Preset {
    pub fn grayscale_chroma(self) -> bool {
        matches!(self, Preset::Grayscale)
    }
}

impl std::str::FromStr for Preset {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "grayscale" => Ok(Preset::Grayscale),
            "low" => Ok(Preset::Low),
            "medium" => Ok(Preset::Medium),
            "high" => Ok(Preset::High),
            "maximum" => Ok(Preset::Maximum),
            _ => anyhow::bail!("invalid preset: expected grayscale, low, medium, high, or maximum"),
        }
    }
}

impl Preset {
    pub fn as_str(self) -> &'static str {
        match self {
            Preset::Grayscale => "grayscale",
            Preset::Low => "low",
            Preset::Medium => "medium",
            Preset::High => "high",
            Preset::Maximum => "maximum",
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
#[derive(Clone, Copy, Debug)]
pub struct EncodeTuning {
    pub preset: Preset,
    pub bitrate_override_kbps: Option<u32>,
    pub cpu_used: Option<i32>,
}

#[cfg(not(any(windows, target_os = "macos")))]
impl EncodeTuning {
    pub fn bitrate_kbps(self) -> u32 {
        self.bitrate_override_kbps
            .unwrap_or_else(|| match self.preset {
                Preset::Grayscale => 200,
                Preset::Low => 500,
                Preset::Medium => 1500,
                Preset::High => 4000,
                Preset::Maximum => 16_000,
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayStreamMode {
    LegacyCapture,
    ModernCapture,
    ScreenshotOnly,
}

impl DisplayStreamMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LegacyCapture => DISPLAY_STREAM_MODE_LEGACY_CAPTURE,
            Self::ModernCapture => DISPLAY_STREAM_MODE_MODERN_CAPTURE,
            Self::ScreenshotOnly => DISPLAY_STREAM_MODE_SCREENSHOT_ONLY,
        }
    }
}

#[cfg(windows)]
pub fn parse_display_stream_mode(value: Option<&str>) -> DisplayStreamMode {
    match value
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "modern_capture" => DisplayStreamMode::ModernCapture,
        "screenshot_only" => DisplayStreamMode::ScreenshotOnly,
        "legacy_capture" | "video_vp8" => DisplayStreamMode::LegacyCapture,
        _ => DisplayStreamMode::LegacyCapture,
    }
}

#[cfg(windows)]
fn display_stream_mode_for_processing_mode(
    processing_mode: crate::display_processing::DisplayProcessingMode,
    preferred_stream_mode: DisplayStreamMode,
) -> DisplayStreamMode {
    if preferred_stream_mode == DisplayStreamMode::ScreenshotOnly {
        DisplayStreamMode::ScreenshotOnly
    } else if processing_mode.is_legacy() {
        DisplayStreamMode::LegacyCapture
    } else if matches!(
        processing_mode,
        crate::display_processing::DisplayProcessingMode::Gpu
    ) {
        DisplayStreamMode::ModernCapture
    } else {
        preferred_stream_mode
    }
}

#[cfg(windows)]
fn profile_id_for_processing_mode(
    processing_mode: crate::display_processing::DisplayProcessingMode,
) -> Option<&'static str> {
    match processing_mode {
        crate::display_processing::DisplayProcessingMode::Legacy => {
            Some(REMOTE_DESKTOP_PROFILE_LEGACY)
        }
        crate::display_processing::DisplayProcessingMode::Gpu => {
            Some(REMOTE_DESKTOP_PROFILE_MODERN_GPU)
        }
        crate::display_processing::DisplayProcessingMode::Auto => None,
    }
}

#[cfg(windows)]
pub fn display_stream_mode_for_profile(profile_id: &str) -> DisplayStreamMode {
    match profile_id.trim().to_ascii_lowercase().as_str() {
        REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY => DisplayStreamMode::ScreenshotOnly,
        REMOTE_DESKTOP_PROFILE_EXPERIMENTAL | REMOTE_DESKTOP_PROFILE_MODERN_GPU => {
            DisplayStreamMode::ModernCapture
        }
        _ => DisplayStreamMode::LegacyCapture,
    }
}

#[cfg(windows)]
pub fn display_processing_mode_for_profile(profile_id: &str) -> &'static str {
    match profile_id.trim().to_ascii_lowercase().as_str() {
        REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY => REMOTE_DESKTOP_PROFILE_LEGACY,
        REMOTE_DESKTOP_PROFILE_EXPERIMENTAL | REMOTE_DESKTOP_PROFILE_MODERN_GPU => {
            REMOTE_DESKTOP_PROFILE_MODERN_GPU
        }
        REMOTE_DESKTOP_PROFILE_MODERN_CPU => REMOTE_DESKTOP_PROFILE_LEGACY,
        _ => REMOTE_DESKTOP_PROFILE_LEGACY,
    }
}

#[cfg(windows)]
fn profile_supported(profiles: &[RemoteDesktopDisplayProfile], profile_id: &str) -> bool {
    profiles.iter().any(|profile| profile.id == profile_id)
}

#[cfg(windows)]
fn normalize_requested_display_profile(profile_id: &str) -> String {
    match profile_id.trim().to_ascii_lowercase().as_str() {
        REMOTE_DESKTOP_PROFILE_EXPERIMENTAL => REMOTE_DESKTOP_PROFILE_MODERN_GPU.to_string(),
        REMOTE_DESKTOP_PROFILE_MODERN_CPU => REMOTE_DESKTOP_PROFILE_LEGACY.to_string(),
        other => other.to_string(),
    }
}

#[cfg(windows)]
pub fn advertised_display_profiles_for_effective_processing_mode(
    context: &'static str,
) -> Vec<RemoteDesktopDisplayProfile> {
    let processing_mode = crate::display_processing::effective_display_processing_mode(context);
    display_profiles_for_processing_mode(processing_mode)
}

#[cfg(windows)]
fn display_profiles_for_processing_mode(
    processing_mode: crate::display_processing::DisplayProcessingMode,
) -> Vec<RemoteDesktopDisplayProfile> {
    match processing_mode {
        crate::display_processing::DisplayProcessingMode::Legacy => {
            vec![
                RemoteDesktopDisplayProfile::legacy(),
                RemoteDesktopDisplayProfile::screenshot_only(),
            ]
        }
        crate::display_processing::DisplayProcessingMode::Gpu => {
            vec![
                RemoteDesktopDisplayProfile::modern_gpu(),
                RemoteDesktopDisplayProfile::screenshot_only(),
            ]
        }
        crate::display_processing::DisplayProcessingMode::Auto => {
            vec![
                RemoteDesktopDisplayProfile::modern_gpu(),
                RemoteDesktopDisplayProfile::legacy(),
                RemoteDesktopDisplayProfile::screenshot_only(),
            ]
        }
    }
}

#[cfg(windows)]
pub fn selected_display_profile_for_effective_processing_mode(
    context: &'static str,
    requested_profile: Option<&str>,
) -> String {
    let processing_mode = crate::display_processing::effective_display_processing_mode(context);
    if let Some(forced) = profile_id_for_processing_mode(processing_mode) {
        return forced.to_string();
    }

    let profiles = advertised_display_profiles_for_effective_processing_mode(context);
    if let Some(requested) = requested_profile {
        let requested = normalize_requested_display_profile(requested);
        if profile_supported(&profiles, &requested) {
            return requested;
        }
    }
    profiles
        .first()
        .map(|profile| profile.id.clone())
        .unwrap_or_else(|| REMOTE_DESKTOP_PROFILE_LEGACY.to_string())
}

#[cfg(windows)]
pub fn h264_dirty_rect_stream_supported() -> bool {
    *H264_DIRTY_RECT_SUPPORTED.get_or_init(|| match mf_h264::probe_h264_dirty_rect_support() {
        Ok(()) => {
            debug!(
                probe_width = 64,
                probe_height = 64,
                probe_fps = 15,
                "h264 dirty-rect capability probe succeeded"
            );
            true
        }
        Err(err) => {
            warn!(
                probe_width = 64,
                probe_height = 64,
                probe_fps = 15,
                error = %err,
                "h264 dirty-rect capability probe failed; falling back to legacy_capture default"
            );
            false
        }
    })
}

#[cfg(windows)]
pub fn effective_display_processing_mode_label(context: &'static str) -> &'static str {
    crate::display_processing::effective_display_processing_mode(context).as_str()
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClippedMoveRect {
    src_x: u32,
    src_y: u32,
    dst_x: u32,
    dst_y: u32,
    width: u32,
    height: u32,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug)]
struct PackedAtlasRect {
    source: crate::capture::DirtyRect,
    atlas: DisplayAtlasRect,
}

#[cfg(windows)]
fn apply_move_rect_to_frame_shadow(
    frame: &mut crate::capture::Frame,
    rect: ClippedMoveRect,
) -> Option<()> {
    if frame.stride < frame.width.saturating_mul(4) {
        return None;
    }
    let bytes_per_pixel = 4usize;
    let stride = frame.stride as usize;
    let row_bytes = rect.width as usize * bytes_per_pixel;
    let mut scratch = vec![0u8; row_bytes * rect.height as usize];

    for row in 0..rect.height as usize {
        let src_start = (rect.src_y as usize + row)
            .checked_mul(stride)?
            .checked_add(rect.src_x as usize * bytes_per_pixel)?;
        let src_end = src_start.checked_add(row_bytes)?;
        let scratch_start = row * row_bytes;
        let scratch_end = scratch_start + row_bytes;
        if src_end > frame.data.len() {
            return None;
        }
        scratch[scratch_start..scratch_end].copy_from_slice(&frame.data[src_start..src_end]);
    }

    for row in 0..rect.height as usize {
        let scratch_start = row * row_bytes;
        let scratch_end = scratch_start + row_bytes;
        let dst_start = (rect.dst_y as usize + row)
            .checked_mul(stride)?
            .checked_add(rect.dst_x as usize * bytes_per_pixel)?;
        let dst_end = dst_start.checked_add(row_bytes)?;
        if dst_end > frame.data.len() {
            return None;
        }
        frame.data[dst_start..dst_end].copy_from_slice(&scratch[scratch_start..scratch_end]);
    }
    Some(())
}

#[cfg(windows)]
#[cfg_attr(not(test), allow(dead_code))]
fn apply_dirty_rect_to_frame_shadow(
    frame: &mut crate::capture::Frame,
    rect: crate::capture::DirtyRect,
    raw_rect: &[u8],
) -> Option<()> {
    let width = rect.right.checked_sub(rect.left)?;
    let height = rect.bottom.checked_sub(rect.top)?;
    let row_bytes = width as usize * 4;
    if row_bytes == 0 || raw_rect.len() != row_bytes * height as usize {
        return None;
    }
    let stride = frame.stride as usize;
    let left = rect.left as usize * 4;
    for row in 0..height as usize {
        let src_start = row * row_bytes;
        let src_end = src_start + row_bytes;
        let dst_start = (rect.top as usize + row)
            .checked_mul(stride)?
            .checked_add(left)?;
        let dst_end = dst_start.checked_add(row_bytes)?;
        if dst_end > frame.data.len() {
            return None;
        }
        frame.data[dst_start..dst_end].copy_from_slice(&raw_rect[src_start..src_end]);
    }
    Some(())
}

#[cfg(windows)]
fn clip_dirty_rect(
    rect: crate::capture::DirtyRect,
    frame_width: u32,
    frame_height: u32,
) -> Option<crate::capture::DirtyRect> {
    let left = rect.left.min(frame_width);
    let top = rect.top.min(frame_height);
    let right = rect.right.min(frame_width);
    let bottom = rect.bottom.min(frame_height);
    if right <= left || bottom <= top {
        None
    } else {
        Some(crate::capture::DirtyRect {
            left,
            top,
            right,
            bottom,
        })
    }
}

#[cfg(windows)]
fn clip_move_rect(
    rect: crate::capture::MoveRect,
    frame_width: u32,
    frame_height: u32,
) -> Option<ClippedMoveRect> {
    let width = rect.right.checked_sub(rect.left)?;
    let height = rect.bottom.checked_sub(rect.top)?;
    if width == 0 || height == 0 {
        return None;
    }
    let dst_right = rect.left.checked_add(width)?;
    let dst_bottom = rect.top.checked_add(height)?;
    let src_right = rect.source_x.checked_add(width)?;
    let src_bottom = rect.source_y.checked_add(height)?;
    if dst_right > frame_width
        || dst_bottom > frame_height
        || src_right > frame_width
        || src_bottom > frame_height
    {
        return None;
    }
    Some(ClippedMoveRect {
        src_x: rect.source_x,
        src_y: rect.source_y,
        dst_x: rect.left,
        dst_y: rect.top,
        width,
        height,
    })
}

#[cfg(windows)]
fn synthesize_dirty_rects_from_frame_diff(
    previous: Option<&crate::capture::Frame>,
    current: &crate::capture::Frame,
) -> Vec<crate::capture::DirtyRect> {
    let Some(previous) = previous else {
        return Vec::new();
    };

    if current.width == 0 || current.height == 0 {
        return Vec::new();
    }

    if previous.width != current.width
        || previous.height != current.height
        || previous.stride != current.stride
        || previous.data.len() != current.data.len()
    {
        return vec![crate::capture::DirtyRect {
            left: 0,
            top: 0,
            right: current.width,
            bottom: current.height,
        }];
    }

    let bytes_per_pixel = 4usize;
    let row_bytes = current.width as usize * bytes_per_pixel;
    let stride = current.stride as usize;
    if row_bytes == 0 || stride < row_bytes {
        return vec![crate::capture::DirtyRect {
            left: 0,
            top: 0,
            right: current.width,
            bottom: current.height,
        }];
    }

    let mut changed = false;
    let mut left = current.width;
    let mut top = current.height;
    let mut right = 0u32;
    let mut bottom = 0u32;

    for row in 0..current.height as usize {
        let start = row * stride;
        let end = start + row_bytes;
        if end > current.data.len() || end > previous.data.len() {
            return vec![crate::capture::DirtyRect {
                left: 0,
                top: 0,
                right: current.width,
                bottom: current.height,
            }];
        }
        let current_row = &current.data[start..end];
        let previous_row = &previous.data[start..end];
        if current_row == previous_row {
            continue;
        }

        let mut first_diff = None;
        for col in 0..current.width as usize {
            let pixel = col * bytes_per_pixel;
            if current_row[pixel..pixel + bytes_per_pixel]
                != previous_row[pixel..pixel + bytes_per_pixel]
            {
                first_diff = Some(col as u32);
                break;
            }
        }

        let Some(first_diff) = first_diff else {
            continue;
        };

        let mut last_diff = first_diff;
        for col in (first_diff as usize..current.width as usize).rev() {
            let pixel = col * bytes_per_pixel;
            if current_row[pixel..pixel + bytes_per_pixel]
                != previous_row[pixel..pixel + bytes_per_pixel]
            {
                last_diff = col as u32;
                break;
            }
        }

        changed = true;
        left = left.min(first_diff);
        top = top.min(row as u32);
        right = right.max(last_diff.saturating_add(1));
        bottom = bottom.max((row as u32).saturating_add(1));
    }

    if !changed {
        return Vec::new();
    }

    vec![crate::capture::DirtyRect {
        left,
        top,
        right,
        bottom,
    }]
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PixelDisplacement {
    dx: i32,
    dy: i32,
}

#[cfg(windows)]
struct TileDiffAnalysis {
    tiles_x: usize,
    tiles_y: usize,
    changed_tiles: Vec<bool>,
    changed_tile_count: usize,
}

#[cfg(windows)]
fn full_frame_dirty_rects(width: u32, height: u32) -> Vec<crate::capture::DirtyRect> {
    if width == 0 || height == 0 {
        Vec::new()
    } else {
        vec![crate::capture::DirtyRect {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        }]
    }
}

#[cfg(windows)]
fn dirty_rect_area(rect: crate::capture::DirtyRect) -> u32 {
    rect.right
        .saturating_sub(rect.left)
        .saturating_mul(rect.bottom.saturating_sub(rect.top))
}

#[cfg(windows)]
fn translate_dirty_rect(
    rect: crate::capture::DirtyRect,
    dx: i32,
    dy: i32,
) -> Option<crate::capture::DirtyRect> {
    let left = rect.left as i64 + dx as i64;
    let top = rect.top as i64 + dy as i64;
    let right = rect.right as i64 + dx as i64;
    let bottom = rect.bottom as i64 + dy as i64;
    if left < 0 || top < 0 || right <= left || bottom <= top {
        return None;
    }
    Some(crate::capture::DirtyRect {
        left: u32::try_from(left).ok()?,
        top: u32::try_from(top).ok()?,
        right: u32::try_from(right).ok()?,
        bottom: u32::try_from(bottom).ok()?,
    })
}

#[cfg(windows)]
fn dirty_rects_overlap(a: crate::capture::DirtyRect, b: crate::capture::DirtyRect) -> bool {
    a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
}

#[cfg(windows)]
fn frame_rects_equal(
    source_frame: &crate::capture::Frame,
    source_rect: crate::capture::DirtyRect,
    target_frame: &crate::capture::Frame,
    target_rect: crate::capture::DirtyRect,
) -> bool {
    let source_width = source_rect.right.saturating_sub(source_rect.left);
    let source_height = source_rect.bottom.saturating_sub(source_rect.top);
    let target_width = target_rect.right.saturating_sub(target_rect.left);
    let target_height = target_rect.bottom.saturating_sub(target_rect.top);
    if source_width == 0
        || source_height == 0
        || source_width != target_width
        || source_height != target_height
        || source_frame.stride < source_frame.width.saturating_mul(4)
        || target_frame.stride < target_frame.width.saturating_mul(4)
    {
        return false;
    }

    let row_bytes = source_width as usize * 4;
    let source_stride = source_frame.stride as usize;
    let target_stride = target_frame.stride as usize;
    for row in 0..source_height as usize {
        let source_start = (source_rect.top as usize + row)
            .checked_mul(source_stride)
            .and_then(|offset| offset.checked_add(source_rect.left as usize * 4));
        let target_start = (target_rect.top as usize + row)
            .checked_mul(target_stride)
            .and_then(|offset| offset.checked_add(target_rect.left as usize * 4));
        let (Some(source_start), Some(target_start)) = (source_start, target_start) else {
            return false;
        };
        let Some(source_end) = source_start.checked_add(row_bytes) else {
            return false;
        };
        let Some(target_end) = target_start.checked_add(row_bytes) else {
            return false;
        };
        if source_end > source_frame.data.len() || target_end > target_frame.data.len() {
            return false;
        }
        if source_frame.data[source_start..source_end]
            != target_frame.data[target_start..target_end]
        {
            return false;
        }
    }

    true
}

#[cfg(windows)]
fn sample_positions(length: u32) -> Vec<u32> {
    if length == 0 {
        return Vec::new();
    }
    let candidates = [
        0,
        length / 3,
        length.saturating_mul(2) / 3,
        length.saturating_sub(1),
    ];
    let mut positions = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if !positions.contains(&candidate) {
            positions.push(candidate);
        }
    }
    positions
}

#[cfg(windows)]
fn frame_rect_signature(
    frame: &crate::capture::Frame,
    rect: crate::capture::DirtyRect,
) -> Option<u64> {
    let width = rect.right.checked_sub(rect.left)?;
    let height = rect.bottom.checked_sub(rect.top)?;
    if width == 0 || height == 0 || frame.stride < frame.width.saturating_mul(4) {
        return None;
    }

    let row_positions = sample_positions(height);
    let col_positions = sample_positions(width);
    if row_positions.is_empty() || col_positions.is_empty() {
        return None;
    }

    let stride = frame.stride as usize;
    let mut hash = 0xcbf29ce484222325u64;
    hash ^= width as u64;
    hash = hash.wrapping_mul(0x100000001b3);
    hash ^= height as u64;
    hash = hash.wrapping_mul(0x100000001b3);

    for row in row_positions {
        for col in &col_positions {
            let offset = (rect.top as usize + row as usize)
                .checked_mul(stride)?
                .checked_add((rect.left as usize + *col as usize) * 4)?;
            let end = offset.checked_add(4)?;
            if end > frame.data.len() {
                return None;
            }
            let pixel = u32::from_le_bytes(frame.data[offset..end].try_into().ok()?);
            hash ^= pixel as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }

    Some(hash)
}

#[cfg(windows)]
fn tile_grid_dimensions(frame_width: u32, frame_height: u32, tile_size: u32) -> (usize, usize) {
    if frame_width == 0 || frame_height == 0 || tile_size == 0 {
        return (0, 0);
    }
    (
        frame_width.div_ceil(tile_size) as usize,
        frame_height.div_ceil(tile_size) as usize,
    )
}

#[cfg(windows)]
fn tile_rect_for_coords(
    tile_x: usize,
    tile_y: usize,
    tile_size: u32,
    frame_width: u32,
    frame_height: u32,
) -> Option<crate::capture::DirtyRect> {
    let left = u32::try_from(tile_x).ok()?.checked_mul(tile_size)?;
    let top = u32::try_from(tile_y).ok()?.checked_mul(tile_size)?;
    if left >= frame_width || top >= frame_height {
        return None;
    }
    Some(crate::capture::DirtyRect {
        left,
        top,
        right: frame_width.min(left.saturating_add(tile_size)),
        bottom: frame_height.min(top.saturating_add(tile_size)),
    })
}

#[cfg(windows)]
fn tile_rect_for_index(
    tile_index: usize,
    tiles_x: usize,
    tile_size: u32,
    frame_width: u32,
    frame_height: u32,
) -> Option<crate::capture::DirtyRect> {
    if tiles_x == 0 {
        return None;
    }
    let tile_x = tile_index % tiles_x;
    let tile_y = tile_index / tiles_x;
    tile_rect_for_coords(tile_x, tile_y, tile_size, frame_width, frame_height)
}

#[cfg(windows)]
fn compute_tile_diff_analysis(
    previous: &crate::capture::Frame,
    current: &crate::capture::Frame,
    tile_size: u32,
) -> TileDiffAnalysis {
    let (tiles_x, tiles_y) = tile_grid_dimensions(current.width, current.height, tile_size);
    let mut changed_tiles = vec![false; tiles_x.saturating_mul(tiles_y)];
    let mut changed_tile_count = 0usize;

    for tile_y in 0..tiles_y {
        for tile_x in 0..tiles_x {
            let tile_index = tile_y * tiles_x + tile_x;
            let Some(tile_rect) =
                tile_rect_for_coords(tile_x, tile_y, tile_size, current.width, current.height)
            else {
                continue;
            };
            let changed = !frame_rects_equal(previous, tile_rect, current, tile_rect);
            changed_tiles[tile_index] = changed;
            if changed {
                changed_tile_count = changed_tile_count.saturating_add(1);
            }
        }
    }

    TileDiffAnalysis {
        tiles_x,
        tiles_y,
        changed_tiles,
        changed_tile_count,
    }
}

#[cfg(windows)]
fn collect_connected_tile_components(
    changed_tiles: &[bool],
    tiles_x: usize,
    tiles_y: usize,
) -> Vec<Vec<usize>> {
    let mut visited = vec![false; changed_tiles.len()];
    let mut components = Vec::new();

    for tile_index in 0..changed_tiles.len() {
        if !changed_tiles[tile_index] || visited[tile_index] {
            continue;
        }

        let mut queue = VecDeque::from([tile_index]);
        visited[tile_index] = true;
        let mut component = Vec::new();
        while let Some(current_index) = queue.pop_front() {
            component.push(current_index);
            let tile_x = current_index % tiles_x;
            let tile_y = current_index / tiles_x;
            let neighbors = [
                tile_x.checked_sub(1).map(|x| (x, tile_y)),
                (tile_x + 1 < tiles_x).then_some((tile_x + 1, tile_y)),
                tile_y.checked_sub(1).map(|y| (tile_x, y)),
                (tile_y + 1 < tiles_y).then_some((tile_x, tile_y + 1)),
            ];
            for neighbor in neighbors.into_iter().flatten() {
                let neighbor_index = neighbor.1 * tiles_x + neighbor.0;
                if !changed_tiles[neighbor_index] || visited[neighbor_index] {
                    continue;
                }
                visited[neighbor_index] = true;
                queue.push_back(neighbor_index);
            }
        }
        components.push(component);
    }

    components
}

#[cfg(windows)]
fn component_bounding_rect(
    component: &[usize],
    tiles_x: usize,
    tile_size: u32,
    frame_width: u32,
    frame_height: u32,
) -> Option<crate::capture::DirtyRect> {
    let first_index = *component.first()?;
    let first_rect =
        tile_rect_for_index(first_index, tiles_x, tile_size, frame_width, frame_height)?;
    let mut left = first_rect.left;
    let mut top = first_rect.top;
    let mut right = first_rect.right;
    let mut bottom = first_rect.bottom;

    for tile_index in component.iter().copied().skip(1) {
        let tile_rect =
            tile_rect_for_index(tile_index, tiles_x, tile_size, frame_width, frame_height)?;
        left = left.min(tile_rect.left);
        top = top.min(tile_rect.top);
        right = right.max(tile_rect.right);
        bottom = bottom.max(tile_rect.bottom);
    }

    Some(crate::capture::DirtyRect {
        left,
        top,
        right,
        bottom,
    })
}

#[cfg(windows)]
fn dirty_rects_from_tile_mask(
    changed_tiles: &[bool],
    tiles_x: usize,
    tiles_y: usize,
    frame_width: u32,
    frame_height: u32,
) -> Vec<crate::capture::DirtyRect> {
    let components = collect_connected_tile_components(changed_tiles, tiles_x, tiles_y);
    if components.is_empty() {
        return Vec::new();
    }
    if components.len() > CPU_SYNTH_MAX_DIRTY_RECTS {
        return full_frame_dirty_rects(frame_width, frame_height);
    }

    components
        .into_iter()
        .filter_map(|component| {
            component_bounding_rect(
                &component,
                tiles_x,
                CPU_SYNTH_TILE_SIZE,
                frame_width,
                frame_height,
            )
        })
        .collect()
}

#[cfg(windows)]
fn build_previous_anchor_signature_map(
    previous: &crate::capture::Frame,
) -> HashMap<u64, Vec<(u32, u32)>> {
    let mut signatures = HashMap::new();
    if previous.width < CPU_MOVE_ANCHOR_SIZE || previous.height < CPU_MOVE_ANCHOR_SIZE {
        return signatures;
    }

    let max_x = previous.width - CPU_MOVE_ANCHOR_SIZE;
    let max_y = previous.height - CPU_MOVE_ANCHOR_SIZE;
    for anchor_y in (0..=max_y).step_by(CPU_MOVE_ANCHOR_STRIDE as usize) {
        for anchor_x in (0..=max_x).step_by(CPU_MOVE_ANCHOR_STRIDE as usize) {
            let anchor_rect = crate::capture::DirtyRect {
                left: anchor_x,
                top: anchor_y,
                right: anchor_x + CPU_MOVE_ANCHOR_SIZE,
                bottom: anchor_y + CPU_MOVE_ANCHOR_SIZE,
            };
            let Some(signature) = frame_rect_signature(previous, anchor_rect) else {
                continue;
            };
            let bucket = signatures.entry(signature).or_insert_with(Vec::new);
            if bucket.len() < CPU_MOVE_MAX_SIGNATURE_MATCHES + 1 {
                bucket.push((anchor_x, anchor_y));
            }
        }
    }

    signatures
}

#[cfg(windows)]
fn anchor_origins_for_rect(rect: crate::capture::DirtyRect) -> Vec<(u32, u32)> {
    let width = rect.right.saturating_sub(rect.left);
    let height = rect.bottom.saturating_sub(rect.top);
    if width < CPU_MOVE_ANCHOR_SIZE || height < CPU_MOVE_ANCHOR_SIZE {
        return Vec::new();
    }

    let max_x = rect.right - CPU_MOVE_ANCHOR_SIZE;
    let max_y = rect.bottom - CPU_MOVE_ANCHOR_SIZE;
    let candidates = [
        (rect.left, rect.top),
        (
            rect.left + (width - CPU_MOVE_ANCHOR_SIZE) / 2,
            rect.top + (height - CPU_MOVE_ANCHOR_SIZE) / 2,
        ),
        (max_x, max_y),
    ];
    let mut origins = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if !origins.contains(&candidate) {
            origins.push(candidate);
        }
    }
    origins
}

#[cfg(windows)]
fn estimate_move_displacement_for_tile(
    previous: &crate::capture::Frame,
    current: &crate::capture::Frame,
    tile_rect: crate::capture::DirtyRect,
    previous_anchor_signatures: &HashMap<u64, Vec<(u32, u32)>>,
) -> Option<PixelDisplacement> {
    let mut votes = HashMap::new();
    let max_anchor_x = previous.width.checked_sub(CPU_MOVE_ANCHOR_SIZE)?;
    let max_anchor_y = previous.height.checked_sub(CPU_MOVE_ANCHOR_SIZE)?;
    let refine_radius = CPU_MOVE_ANCHOR_STRIDE.saturating_sub(1);

    for (anchor_x, anchor_y) in anchor_origins_for_rect(tile_rect) {
        let current_anchor = crate::capture::DirtyRect {
            left: anchor_x,
            top: anchor_y,
            right: anchor_x + CPU_MOVE_ANCHOR_SIZE,
            bottom: anchor_y + CPU_MOVE_ANCHOR_SIZE,
        };
        let Some(signature) = frame_rect_signature(current, current_anchor) else {
            continue;
        };
        let Some(matches) = previous_anchor_signatures.get(&signature) else {
            continue;
        };
        if matches.len() > CPU_MOVE_MAX_SIGNATURE_MATCHES {
            continue;
        }

        for &(approx_source_x, approx_source_y) in matches {
            let min_source_x = approx_source_x.saturating_sub(refine_radius);
            let min_source_y = approx_source_y.saturating_sub(refine_radius);
            let max_source_x = approx_source_x
                .saturating_add(refine_radius)
                .min(max_anchor_x);
            let max_source_y = approx_source_y
                .saturating_add(refine_radius)
                .min(max_anchor_y);

            for source_y in min_source_y..=max_source_y {
                for source_x in min_source_x..=max_source_x {
                    let source_anchor = crate::capture::DirtyRect {
                        left: source_x,
                        top: source_y,
                        right: source_x + CPU_MOVE_ANCHOR_SIZE,
                        bottom: source_y + CPU_MOVE_ANCHOR_SIZE,
                    };
                    if !frame_rects_equal(previous, source_anchor, current, current_anchor) {
                        continue;
                    }

                    let displacement = PixelDisplacement {
                        dx: source_x as i32 - anchor_x as i32,
                        dy: source_y as i32 - anchor_y as i32,
                    };
                    if displacement.dx == 0 && displacement.dy == 0 {
                        continue;
                    }
                    *votes.entry(displacement).or_insert(0usize) += 1;
                }
            }
        }
    }

    let mut best_displacement = None;
    let mut best_votes = 0usize;
    let mut tied = false;
    for (displacement, count) in votes {
        if count > best_votes {
            best_displacement = Some(displacement);
            best_votes = count;
            tied = false;
        } else if count == best_votes {
            tied = true;
        }
    }

    if tied || best_votes == 0 {
        None
    } else {
        best_displacement
    }
}

#[cfg(windows)]
fn tile_matches_displacement(
    previous: &crate::capture::Frame,
    current: &crate::capture::Frame,
    tile_rect: crate::capture::DirtyRect,
    displacement: PixelDisplacement,
) -> bool {
    let Some(source_rect) = translate_dirty_rect(tile_rect, displacement.dx, displacement.dy)
    else {
        return false;
    };
    frame_rects_equal(previous, source_rect, current, tile_rect)
}

#[cfg(windows)]
fn move_rect_conflicts(
    move_rect: crate::capture::MoveRect,
    accepted_move_rects: &[crate::capture::MoveRect],
) -> bool {
    let destination_rect = crate::capture::DirtyRect {
        left: move_rect.left,
        top: move_rect.top,
        right: move_rect.right,
        bottom: move_rect.bottom,
    };
    let Some(source_rect) = translate_dirty_rect(
        destination_rect,
        move_rect.source_x as i32 - move_rect.left as i32,
        move_rect.source_y as i32 - move_rect.top as i32,
    ) else {
        return true;
    };

    accepted_move_rects.iter().any(|accepted_move_rect| {
        let accepted_destination = crate::capture::DirtyRect {
            left: accepted_move_rect.left,
            top: accepted_move_rect.top,
            right: accepted_move_rect.right,
            bottom: accepted_move_rect.bottom,
        };
        let Some(accepted_source) = translate_dirty_rect(
            accepted_destination,
            accepted_move_rect.source_x as i32 - accepted_move_rect.left as i32,
            accepted_move_rect.source_y as i32 - accepted_move_rect.top as i32,
        ) else {
            return true;
        };
        dirty_rects_overlap(destination_rect, accepted_destination)
            || dirty_rects_overlap(destination_rect, accepted_source)
            || dirty_rects_overlap(source_rect, accepted_destination)
            || dirty_rects_overlap(source_rect, accepted_source)
    })
}

#[cfg(windows)]
fn synthesize_dirty_rects_from_frame_diff_tiled(
    previous: Option<&crate::capture::Frame>,
    current: &crate::capture::Frame,
) -> Vec<crate::capture::DirtyRect> {
    let Some(previous) = previous else {
        return Vec::new();
    };
    if current.width == 0 || current.height == 0 {
        return Vec::new();
    }
    if previous.width != current.width
        || previous.height != current.height
        || previous.stride != current.stride
        || previous.data.len() != current.data.len()
        || previous.stride < previous.width.saturating_mul(4)
        || current.stride < current.width.saturating_mul(4)
    {
        return full_frame_dirty_rects(current.width, current.height);
    }

    let analysis = compute_tile_diff_analysis(previous, current, CPU_SYNTH_TILE_SIZE);
    if analysis.changed_tile_count == 0 {
        Vec::new()
    } else {
        dirty_rects_from_tile_mask(
            &analysis.changed_tiles,
            analysis.tiles_x,
            analysis.tiles_y,
            current.width,
            current.height,
        )
    }
}

#[cfg(windows)]
fn synthesize_cpu_frame_metadata_from_diff(
    previous: Option<&crate::capture::Frame>,
    current: &crate::capture::Frame,
) -> Result<crate::capture::FrameMetadata> {
    let Some(previous) = previous else {
        return Ok(crate::capture::FrameMetadata::default());
    };
    if current.width == 0 || current.height == 0 {
        return Ok(crate::capture::FrameMetadata::default());
    }
    if previous.width != current.width
        || previous.height != current.height
        || previous.stride != current.stride
        || previous.data.len() != current.data.len()
        || previous.stride < previous.width.saturating_mul(4)
        || current.stride < current.width.saturating_mul(4)
    {
        return Ok(crate::capture::FrameMetadata {
            dirty_rects: full_frame_dirty_rects(current.width, current.height),
            move_rects: Vec::new(),
            accumulated_frames: 1,
            rects_coalesced: false,
        });
    }

    let analysis = compute_tile_diff_analysis(previous, current, CPU_SYNTH_TILE_SIZE);
    if analysis.changed_tile_count == 0 {
        return Ok(crate::capture::FrameMetadata::default());
    }

    let mut dirty_rects = dirty_rects_from_tile_mask(
        &analysis.changed_tiles,
        analysis.tiles_x,
        analysis.tiles_y,
        current.width,
        current.height,
    );
    let mut move_rects = Vec::new();
    if analysis.changed_tile_count > CPU_MOVE_MAX_CHANGED_TILES_FOR_DETECTION {
        debug!(
            changed_tile_count = analysis.changed_tile_count,
            "cpu move rect synthesis skipped due change budget"
        );
    } else {
        let previous_anchor_signatures = build_previous_anchor_signature_map(previous);
        let mut claimed_tiles = vec![false; analysis.changed_tiles.len()];
        let mut seed_tiles = 0usize;

        for tile_index in 0..analysis.changed_tiles.len() {
            if !analysis.changed_tiles[tile_index] || claimed_tiles[tile_index] {
                continue;
            }
            if seed_tiles >= CPU_MOVE_MAX_SEED_TILES
                || move_rects.len() >= CPU_MOVE_MAX_RECTS_PER_FRAME
            {
                break;
            }
            seed_tiles = seed_tiles.saturating_add(1);

            let Some(tile_rect) = tile_rect_for_index(
                tile_index,
                analysis.tiles_x,
                CPU_SYNTH_TILE_SIZE,
                current.width,
                current.height,
            ) else {
                continue;
            };
            let Some(displacement) = estimate_move_displacement_for_tile(
                previous,
                current,
                tile_rect,
                &previous_anchor_signatures,
            ) else {
                continue;
            };

            let mut queued_tiles = vec![false; analysis.changed_tiles.len()];
            let mut queue = VecDeque::from([tile_index]);
            queued_tiles[tile_index] = true;
            let mut component_tiles = Vec::new();
            while let Some(candidate_index) = queue.pop_front() {
                if claimed_tiles[candidate_index] || !analysis.changed_tiles[candidate_index] {
                    continue;
                }
                let Some(candidate_rect) = tile_rect_for_index(
                    candidate_index,
                    analysis.tiles_x,
                    CPU_SYNTH_TILE_SIZE,
                    current.width,
                    current.height,
                ) else {
                    continue;
                };
                if !tile_matches_displacement(previous, current, candidate_rect, displacement) {
                    continue;
                }

                component_tiles.push(candidate_index);
                let tile_x = candidate_index % analysis.tiles_x;
                let tile_y = candidate_index / analysis.tiles_x;
                let neighbors = [
                    tile_x.checked_sub(1).map(|x| (x, tile_y)),
                    (tile_x + 1 < analysis.tiles_x).then_some((tile_x + 1, tile_y)),
                    tile_y.checked_sub(1).map(|y| (tile_x, y)),
                    (tile_y + 1 < analysis.tiles_y).then_some((tile_x, tile_y + 1)),
                ];
                for neighbor in neighbors.into_iter().flatten() {
                    let neighbor_index = neighbor.1 * analysis.tiles_x + neighbor.0;
                    if queued_tiles[neighbor_index] {
                        continue;
                    }
                    queued_tiles[neighbor_index] = true;
                    queue.push_back(neighbor_index);
                }
            }

            let Some(destination_rect) = component_bounding_rect(
                &component_tiles,
                analysis.tiles_x,
                CPU_SYNTH_TILE_SIZE,
                current.width,
                current.height,
            ) else {
                continue;
            };
            if dirty_rect_area(destination_rect) < CPU_MOVE_MIN_AREA {
                continue;
            }
            let Some(source_rect) =
                translate_dirty_rect(destination_rect, displacement.dx, displacement.dy)
            else {
                continue;
            };
            if !frame_rects_equal(previous, source_rect, current, destination_rect) {
                continue;
            }

            let move_rect = crate::capture::MoveRect {
                source_x: source_rect.left,
                source_y: source_rect.top,
                left: destination_rect.left,
                top: destination_rect.top,
                right: destination_rect.right,
                bottom: destination_rect.bottom,
            };
            if move_rect_conflicts(move_rect, &move_rects)
                || clip_move_rect(move_rect, current.width, current.height).is_none()
            {
                continue;
            }

            for claimed_tile in component_tiles {
                claimed_tiles[claimed_tile] = true;
            }
            move_rects.push(move_rect);
        }

        if !move_rects.is_empty() {
            let mut shadow = previous.clone();
            for move_rect in &move_rects {
                let clipped = clip_move_rect(*move_rect, current.width, current.height)
                    .ok_or_else(|| {
                        anyhow::anyhow!("cpu synthesized move rect exceeded framebuffer")
                    })?;
                apply_move_rect_to_frame_shadow(&mut shadow, clipped).ok_or_else(|| {
                    anyhow::anyhow!("cpu synthesized move rect shadow apply failed")
                })?;
            }
            dirty_rects = synthesize_dirty_rects_from_frame_diff_tiled(Some(&shadow), current);
        }
    }

    Ok(crate::capture::FrameMetadata {
        dirty_rects,
        move_rects,
        accumulated_frames: 1,
        rects_coalesced: false,
    })
}

#[cfg(windows)]
fn pending_capture_index_from_rx(rx: &mpsc::Receiver<usize>) -> Option<usize> {
    let mut last = None;
    while let Ok(idx) = rx.try_recv() {
        last = Some(idx);
    }
    last
}

#[cfg(windows)]
fn pending_stream_bitrate_from_rx(rx: &mpsc::Receiver<u32>) -> Option<u32> {
    let mut last = None;
    while let Ok(kbps) = rx.try_recv() {
        last = Some(kbps);
    }
    last
}

#[cfg(windows)]
fn apply_stream_bitrate_update(tuning: &mut EncodeTuning, kbps: u32) -> bool {
    if kbps == 0 {
        return false;
    }
    if tuning.bitrate_override_kbps == Some(kbps) {
        return false;
    }
    tuning.bitrate_override_kbps = Some(kbps);
    true
}

#[cfg(windows)]
fn h264_bitrate_bps(tuning: EncodeTuning) -> u32 {
    tuning.bitrate_kbps().saturating_mul(1000)
}

#[cfg(windows)]
fn merge_capture_outputs_metadata(obj: &mut serde_json::Map<String, serde_json::Value>) {
    let list = crate::capture::d3d11_duplication::enumerate_dxgi_capture_outputs();
    if list.is_empty() {
        return;
    }
    let entries: Vec<serde_json::Value> = list
        .iter()
        .map(|o| {
            serde_json::json!({
                "index": o.index,
                "name": o.name,
                "width": o.width,
                "height": o.height,
            })
        })
        .collect();
    obj.insert(
        "captureOutputs".to_string(),
        serde_json::Value::Array(entries),
    );
}

#[cfg(windows)]
fn build_stream_metadata(
    tuning: EncodeTuning,
    fps: u32,
    agent_monitor_hz: Option<u32>,
    active_capture_output_index: u32,
    display_stream: Option<DisplayStreamDescriptor>,
    capture_type: &'static str,
) -> Option<Vec<u8>> {
    let mut metadata = serde_json::json!({
        "bitrate_kbps": tuning.bitrate_kbps(),
        "preset": tuning.preset.as_str(),
        "cpu_used": tuning.cpu_used.unwrap_or(2),
        "encoding_fps": fps,
        "agent_monitor_hz": agent_monitor_hz,
        "activeIndex": active_capture_output_index,
        "captureType": capture_type,
    });
    if let Some(obj) = metadata.as_object_mut() {
        if let Some(display_stream) = display_stream {
            if let Ok(v) = serde_json::to_value(&display_stream) {
                obj.insert(DISPLAY_STREAM_META_TYPE.to_string(), v);
            }
        }
        merge_capture_outputs_metadata(obj);
    }
    let json_bytes = serde_json::to_vec(&metadata).ok()?;
    let mut msg = Vec::with_capacity(8 + json_bytes.len());
    msg.extend_from_slice(b"RMMD");
    msg.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    msg.extend_from_slice(&json_bytes);
    Some(msg)
}

#[cfg(windows)]
fn cfg_primary_monitor_hz() -> Option<u32> {
    crate::get_primary_monitor_hz()
}

#[cfg(windows)]
fn aligned_stream_dimensions(width: u32, height: u32) -> (u32, u32) {
    const VP8_ALIGN: u32 = 16;
    let raw_w = (width & !(VP8_ALIGN - 1)).max(VP8_ALIGN);
    let raw_h = (height & !(VP8_ALIGN - 1)).max(VP8_ALIGN);
    let mut enc_width = (raw_w / STREAM_ENCODE_SCALE).max(VP8_ALIGN) & !(VP8_ALIGN - 1);
    let mut enc_height = (raw_h / STREAM_ENCODE_SCALE).max(VP8_ALIGN) & !(VP8_ALIGN - 1);
    enc_width += enc_width & 1;
    enc_height += enc_height & 1;
    (enc_width, enc_height)
}

#[cfg(windows)]
fn primary_display_dimensions() -> Option<(u32, u32)> {
    use std::ptr::null_mut;
    use winapi::um::wingdi::DEVMODEW;
    use winapi::um::winuser::{EnumDisplaySettingsW, ENUM_CURRENT_SETTINGS};

    let mut devmode: DEVMODEW = unsafe { std::mem::zeroed() };
    devmode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
    let ok = unsafe { EnumDisplaySettingsW(null_mut(), ENUM_CURRENT_SETTINGS, &mut devmode) != 0 };
    if !ok || devmode.dmPelsWidth == 0 || devmode.dmPelsHeight == 0 {
        return None;
    }
    Some((devmode.dmPelsWidth, devmode.dmPelsHeight))
}

#[cfg(windows)]
fn build_bootstrap_i420_frame(width: u32, height: u32) -> Vec<u8> {
    let y_len = (width as usize) * (height as usize);
    let uv_len = y_len / 4;
    let mut frame = vec![0u8; y_len + uv_len * 2];
    frame[..y_len].fill(16);
    frame[y_len..].fill(128);
    frame
}

#[cfg(windows)]
struct DesktopContextPoller {
    confirmed: crate::control::DesktopContext,
    pending: Option<(crate::control::DesktopContext, u8)>,
    last_poll: Instant,
    refresh_epoch: u64,
}

#[cfg(windows)]
impl DesktopContextPoller {
    fn new() -> Self {
        Self {
            confirmed: crate::control::input_desktop_context(),
            pending: None,
            last_poll: Instant::now(),
            refresh_epoch: crate::control::desktop_context_refresh_epoch(),
        }
    }

    fn poll_transition(&mut self) -> Option<crate::control::DesktopContext> {
        const POLL_INTERVAL: Duration = Duration::from_millis(150);
        let refresh_epoch = crate::control::desktop_context_refresh_epoch();
        let should_poll_now =
            refresh_epoch != self.refresh_epoch || self.last_poll.elapsed() >= POLL_INTERVAL;
        if !should_poll_now {
            return None;
        }
        self.refresh_epoch = refresh_epoch;
        self.last_poll = Instant::now();

        let now_context = crate::control::input_desktop_context();
        if now_context == self.confirmed {
            self.pending = None;
            return None;
        }

        match &mut self.pending {
            Some((pending_ctx, count)) if *pending_ctx == now_context => {
                *count = count.saturating_add(1);
                if *count >= 2 {
                    self.confirmed = now_context.clone();
                    self.pending = None;
                    return Some(now_context);
                }
            }
            _ => {
                self.pending = Some((now_context, 1));
            }
        }
        None
    }
}

#[cfg(windows)]
fn pack_dirty_rects_into_atlas(
    frame_width: u32,
    frame_height: u32,
    rects: &[crate::capture::DirtyRect],
) -> Option<Vec<PackedAtlasRect>> {
    // Keep atlas coordinates aligned to the framebuffer so temporal H.264
    // prediction stays stable across frames. Repacking dirty rects into
    // different atlas slots frame-to-frame leaks unrelated prior content.
    let mut packed = Vec::with_capacity(rects.len());
    for rect in rects {
        let width = rect.right.checked_sub(rect.left)?;
        let height = rect.bottom.checked_sub(rect.top)?;
        if width == 0
            || height == 0
            || rect.right > frame_width
            || rect.bottom > frame_height
            || width > frame_width
            || height > frame_height
        {
            return None;
        }
        packed.push(PackedAtlasRect {
            source: *rect,
            atlas: DisplayAtlasRect {
                dst_x: rect.left,
                dst_y: rect.top,
                width,
                height,
                atlas_x: rect.left,
                atlas_y: rect.top,
            },
        });
    }
    Some(packed)
}

/// Load encode tuning from env: RMM_ENCODE_PRESET, RMM_ENCODE_BITRATE, RMM_ENCODE_CPUUSED.
pub fn load_encode_tuning_from_env() -> EncodeTuning {
    let preset = env::var("RMM_ENCODE_PRESET")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(Preset::Medium);
    let bitrate_override_kbps = env::var("RMM_ENCODE_BITRATE")
        .ok()
        .and_then(|s| s.parse::<u32>().ok());
    let cpu_used = env::var("RMM_ENCODE_CPUUSED")
        .ok()
        .and_then(|s| s.parse::<i32>().ok());
    EncodeTuning {
        preset,
        bitrate_override_kbps,
        cpu_used,
    }
}

#[cfg(windows)]
#[derive(Clone, Copy)]
enum DirtyRectStreamOutput<'a> {
    Channel(&'a IvfStreamSender),
    Pipe {
        sender: &'a PipeStreamSender,
        pipe_name: &'a str,
    },
}

#[cfg(windows)]
impl<'a> DirtyRectStreamOutput<'a> {
    fn pipe_name(self) -> Option<&'a str> {
        match self {
            Self::Channel(_) => None,
            Self::Pipe { pipe_name, .. } => Some(pipe_name),
        }
    }

    fn write_metadata(self, metadata: &[u8]) -> Result<()> {
        match self {
            Self::Channel(sender) => sender.write_metadata(metadata),
            Self::Pipe { sender, .. } => sender.write_metadata(metadata),
        }
    }

    fn write_display_delta(self, payload: &[u8]) -> Result<()> {
        match self {
            Self::Channel(sender) => sender.write_display_delta(payload),
            Self::Pipe { sender, .. } => sender.write_display_delta(payload),
        }
    }

    fn log_helper(self, event: &str, mut data: serde_json::Value) {
        let Self::Pipe { pipe_name, .. } = self else {
            return;
        };
        if let serde_json::Value::Object(ref mut map) = data {
            map.insert(
                "pipe_name".to_string(),
                serde_json::Value::String(pipe_name.to_string()),
            );
        }
        helper_io_log(event, data);
    }

    fn log_frame_stats(self, stats: DirtyRectFrameLogStats) {
        match self {
            Self::Channel(_) => log_dirty_rect_frame_stats_channel(stats),
            Self::Pipe { pipe_name, .. } => log_dirty_rect_frame_stats_pipe(pipe_name, stats),
        }
    }

    fn log_classifier_summary(self, summary: &DirtyRectClassifierLogSummary) {
        match self {
            Self::Channel(_) => log_dirty_rect_classifier_summary_channel(summary),
            Self::Pipe { pipe_name, .. } => {
                log_dirty_rect_classifier_summary_pipe(pipe_name, summary)
            }
        }
    }

    fn log_gpu_backend_initialized(
        self,
        mode: crate::display_processing::DisplayProcessingMode,
        attempt: u32,
    ) {
        let strict_gpu = !mode.allows_cpu_fallback();
        let pipe_name = self.pipe_name();
        debug!(
            ?pipe_name,
            attempt, strict_gpu, "gpu dirty rect dxgi backend initialized"
        );
        self.log_helper(
            "gpu_dirty_rect_dxgi_backend_initialized",
            serde_json::json!({
                "attempt": attempt,
                "strict_gpu": strict_gpu,
            }),
        );
    }

    fn log_gpu_capture_active(
        self,
        mode: crate::display_processing::DisplayProcessingMode,
        frame_width: u32,
        frame_height: u32,
    ) {
        let strict_gpu = !mode.allows_cpu_fallback();
        let pipe_name = self.pipe_name();
        debug!(
            ?pipe_name,
            frame_width, frame_height, strict_gpu, "gpu dirty rect capture active"
        );
        self.log_helper(
            "gpu_dirty_rect_capture_active",
            serde_json::json!({
                "frame_width": frame_width,
                "frame_height": frame_height,
                "strict_gpu": strict_gpu,
            }),
        );
    }
}

#[cfg(windows)]
fn emit_screenshot_only_bgra(
    output: DirtyRectStreamOutput<'_>,
    tuning: EncodeTuning,
    fps: u32,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    use crate::capture::windows;
    use crate::capture::CaptureBackend;

    crate::control::attach_thread_to_input_desktop();
    let interval = if fps > 0 {
        Duration::from_secs_f64(1.0 / fps as f64)
    } else {
        Duration::from_millis(100)
    };

    let mut backend = windows::GdiBackend::new().context("initialize screenshot-only capture")?;
    let frame = backend
        .capture_frame()
        .context("capture screenshot-only frame")?;
    let expected_len = (frame.width as usize)
        .checked_mul(frame.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("screenshot-only frame dimensions overflow"))?;
    ensure!(
        frame.data.len() == expected_len,
        "screenshot-only frame payload length mismatch: got {}, expected {}",
        frame.data.len(),
        expected_len
    );

    let frame_id = 0u64;
    let descriptor = DisplayStreamDescriptor::screenshot_only(frame.width, frame.height);
    if let Some(msg) = build_stream_metadata(
        tuning,
        fps,
        cfg_primary_monitor_hz(),
        backend.capture_source_index() as u32,
        Some(descriptor),
        "screenshot_only",
    ) {
        output.write_metadata(&msg)?;
    }
    output.write_display_delta(&build_display_frame_begin(
        frame_id,
        frame.width,
        frame.height,
    ))?;
    output.write_display_delta(&build_display_keyframe(
        frame_id,
        frame.width,
        frame.height,
        frame.data.len() as u32,
        &frame.data,
    ))?;
    output.write_display_delta(&build_display_frame_end(frame_id))?;
    output.log_helper(
        "screenshot_only_frame_emitted",
        serde_json::json!({
            "frame_id": frame_id,
            "width": frame.width,
            "height": frame.height,
            "payload_bytes": frame.data.len(),
        }),
    );

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(interval);
    }
    Ok(())
}

#[cfg(windows)]
fn log_skipped_dirty_rect_classifier(
    output: DirtyRectStreamOutput<'_>,
    path: &'static str,
    dirty_rect_count: usize,
    skip_reason: &'static str,
) {
    let summary = DirtyRectClassifierLogSummary::skipped(path, dirty_rect_count, skip_reason);
    output.log_classifier_summary(&summary);
}

#[cfg(windows)]
fn classify_dirty_rects_with_gpu(
    output: DirtyRectStreamOutput<'_>,
    classifier: &mut DirtyRectClassifierContext,
    backend: &crate::capture::d3d11_duplication::GpuDxgiBackend,
    texture_frame: &crate::capture::d3d11_duplication::DxgiTextureFrame,
    dirty_rects: &[crate::capture::DirtyRect],
    path: &'static str,
) {
    if dirty_rects.is_empty() {
        return;
    }
    if !texture_frame.is_identity_rotation() {
        log_skipped_dirty_rect_classifier(output, path, dirty_rects.len(), "rotated_frame");
        return;
    }
    if let Some(reason) = classifier.state.skip_reason() {
        log_skipped_dirty_rect_classifier(output, path, dirty_rects.len(), reason);
        return;
    }
    if classifier.classifier.is_none() {
        match DirtyRectClassifier::new(
            match backend.device() {
                Ok(device) => device,
                Err(err) => {
                    warn!(
                        path,
                        error = %err,
                        "dirty rect classifier device acquisition failed; disabling classifier for session"
                    );
                    output.log_helper(
                        "dirty_rect_classifier_init_failed",
                        serde_json::json!({
                            "path": path,
                            "error": err.to_string(),
                        }),
                    );
                    classifier.state = DirtyRectClassifierState::Disabled {
                        reason: "init_failed",
                    };
                    log_skipped_dirty_rect_classifier(
                        output,
                        path,
                        dirty_rects.len(),
                        "init_failed",
                    );
                    return;
                }
            },
            match backend.device_context() {
                Ok(context) => context,
                Err(err) => {
                    warn!(
                        path,
                        error = %err,
                        "dirty rect classifier context acquisition failed; disabling classifier for session"
                    );
                    output.log_helper(
                        "dirty_rect_classifier_init_failed",
                        serde_json::json!({
                            "path": path,
                            "error": err.to_string(),
                        }),
                    );
                    classifier.state = DirtyRectClassifierState::Disabled {
                        reason: "init_failed",
                    };
                    log_skipped_dirty_rect_classifier(
                        output,
                        path,
                        dirty_rects.len(),
                        "init_failed",
                    );
                    return;
                }
            },
        ) {
            Ok(new_classifier) => {
                classifier.classifier = Some(new_classifier);
                classifier.state = DirtyRectClassifierState::Uninitialized;
            }
            Err(err) => {
                warn!(
                    path,
                    error = %err,
                    "dirty rect classifier initialization failed; disabling classifier for session"
                );
                output.log_helper(
                    "dirty_rect_classifier_init_failed",
                    serde_json::json!({
                        "path": path,
                        "error": err.to_string(),
                    }),
                );
                classifier.state = DirtyRectClassifierState::Disabled {
                    reason: "init_failed",
                };
                log_skipped_dirty_rect_classifier(output, path, dirty_rects.len(), "init_failed");
                return;
            }
        }
    }

    let Some(classifier_impl) = classifier.classifier.as_mut() else {
        log_skipped_dirty_rect_classifier(output, path, dirty_rects.len(), "init_failed");
        return;
    };
    match classifier_impl.classify_frame(
        &texture_frame.texture(),
        texture_frame.width,
        texture_frame.height,
        dirty_rects,
    ) {
        Ok(frame_summary) => {
            let summary = DirtyRectClassifierLogSummary::gpu(path, frame_summary);
            output.log_classifier_summary(&summary);
        }
        Err(err) => {
            warn!(
                path,
                error = %err,
                "dirty rect classifier execution failed; disabling classifier for session"
            );
            output.log_helper(
                "dirty_rect_classifier_run_failed",
                serde_json::json!({
                    "path": path,
                    "error": err.to_string(),
                }),
            );
            classifier.state = DirtyRectClassifierState::Disabled {
                reason: "run_failed",
            };
            classifier.classifier = None;
            log_skipped_dirty_rect_classifier(output, path, dirty_rects.len(), "run_failed");
        }
    }
}

#[cfg(windows)]
fn emit_dirty_rect_stream_h264_gpu(
    output: DirtyRectStreamOutput<'_>,
    tuning: EncodeTuning,
    fps: u32,
    stop: Arc<AtomicBool>,
    mode: crate::display_processing::DisplayProcessingMode,
    capture_output_switch_rx: &mpsc::Receiver<usize>,
    stream_bitrate_rx: &mpsc::Receiver<u32>,
) -> Result<()> {
    use crate::capture::d3d11_duplication::{GpuDxgiBackend, TextureCaptureResult};
    use crate::capture::CaptureBackend;

    crate::control::attach_thread_to_input_desktop();
    let interval = if fps > 0 {
        Duration::from_secs_f64(1.0 / fps as f64)
    } else {
        Duration::from_millis(100)
    };
    let agent_monitor_hz = cfg_primary_monitor_hz();
    let mut backend: Option<GpuDxgiBackend> = None;
    let mut cpu_encoder: Option<mf_h264::H264Encoder> = None;
    let mut active_capture_output_index: usize = 0;
    let mut pending_capture_output_index: Option<usize> = None;
    let mut frame_id = 0u64;
    let mut force_keyframe = true;
    let mut context_poller = DesktopContextPoller::new();
    let mut init_started_at = Instant::now();
    let mut init_attempt: u32 = 0;
    let mut stream_announced = false;
    let mut previous_frame: Option<crate::capture::Frame> = None;
    let mut gpu_capture_logged = false;
    let mut atlas_bgra: Vec<u8> = Vec::new();
    let mut current_frame_dimensions: Option<(u32, u32)> = None;
    let mut classifier = DirtyRectClassifierContext::new();
    const KEYFRAME_INTERVAL: Duration = Duration::from_millis(500);
    const STATIC_REFRESH_INTERVAL: Duration = Duration::from_millis(1000);
    let mut last_keyframe_at = Instant::now() - Duration::from_secs(60);
    let mut current_tuning = tuning;
    let mut gpu_capture_fail_streak: u32 = 0;

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if let Some(kbps) = pending_stream_bitrate_from_rx(stream_bitrate_rx) {
            if apply_stream_bitrate_update(&mut current_tuning, kbps) {
                cpu_encoder = None;
                force_keyframe = true;
                stream_announced = false;
                current_frame_dimensions = None;
                output.log_helper(
                    "h264_dirty_rect_bitrate_updated",
                    serde_json::json!({ "kbps": kbps }),
                );
            }
        }
        if let Some(idx) = pending_capture_index_from_rx(capture_output_switch_rx) {
            pending_capture_output_index = Some(idx);
            if let Some(backend_ref) = backend.as_mut() {
                if let Err(e) = backend_ref.set_capture_source_index(idx) {
                    warn!(
                        error = %e,
                        index = idx,
                        "h264 dirty-rect capture output switch failed"
                    );
                    backend = None;
                    cpu_encoder = None;
                    init_started_at = Instant::now();
                    init_attempt = 0;
                    force_keyframe = true;
                    gpu_capture_logged = false;
                    previous_frame = None;
                    atlas_bgra.clear();
                    current_frame_dimensions = None;
                    classifier.reset();
                } else {
                    active_capture_output_index = backend_ref.capture_source_index();
                    pending_capture_output_index = None;
                    crate::control::set_remote_input_capture_output_index(
                        active_capture_output_index,
                    );
                    if let Some(msg) = build_stream_metadata(
                        current_tuning,
                        fps,
                        agent_monitor_hz,
                        active_capture_output_index as u32,
                        None,
                        "modern_gpu",
                    ) {
                        let _ = output.write_metadata(&msg);
                    }
                    force_keyframe = true;
                    cpu_encoder = None;
                    atlas_bgra.clear();
                    current_frame_dimensions = None;
                }
            }
        }
        if backend.is_none() {
            init_attempt = init_attempt.saturating_add(1);
            match GpuDxgiBackend::new() {
                Ok(new_backend) => {
                    backend = Some(new_backend);
                    if let Some(backend_ref) = backend.as_mut() {
                        let desired_capture_output_index =
                            pending_capture_output_index.unwrap_or(active_capture_output_index);
                        if backend_ref.capture_source_index() != desired_capture_output_index {
                            if let Err(err) =
                                backend_ref.set_capture_source_index(desired_capture_output_index)
                            {
                                warn!(
                                    index = desired_capture_output_index,
                                    error = %err,
                                    "failed to restore active capture output index after backend init"
                                );
                            }
                        }
                        active_capture_output_index = backend_ref.capture_source_index();
                        if active_capture_output_index == desired_capture_output_index {
                            pending_capture_output_index = None;
                        }
                        crate::control::set_remote_input_capture_output_index(
                            active_capture_output_index,
                        );
                        if let Some(msg) = build_stream_metadata(
                            current_tuning,
                            fps,
                            agent_monitor_hz,
                            active_capture_output_index as u32,
                            None,
                            "modern_gpu",
                        ) {
                            let _ = output.write_metadata(&msg);
                        }
                    }
                    output.log_gpu_backend_initialized(mode, init_attempt);
                }
                Err(err) => {
                    if init_attempt == 1
                        || init_attempt.is_multiple_of(MODERN_CAPTURE_RUNTIME_FAILURE_LOG_INTERVAL)
                    {
                        log_dxgi_backend_init_failed(
                            "h264_dirty_rect_dxgi_backend_init_failed",
                            output.pipe_name(),
                            init_attempt,
                            &err,
                        );
                    }
                    if !stream_announced && init_attempt >= MODERN_CAPTURE_STARTUP_FAILURE_LIMIT {
                        return Err(anyhow::anyhow!(
                            "h264 dirty rect dxgi backend init failed: {err}"
                        ));
                    }
                    std::thread::sleep(interval);
                    continue;
                }
            }
        }
        if context_poller.poll_transition().is_some() {
            force_keyframe = true;
        }
        let start = Instant::now();
        let (capture, metadata) = match backend
            .as_mut()
            .expect("backend should exist before h264 dirty rect capture")
            .try_capture_texture_with_metadata()
        {
            Ok(result) => {
                gpu_capture_fail_streak = 0;
                result
            }
            Err(err) => {
                gpu_capture_fail_streak = gpu_capture_fail_streak.saturating_add(1);
                if gpu_capture_fail_streak == 1
                    || gpu_capture_fail_streak
                        .is_multiple_of(MODERN_CAPTURE_RUNTIME_FAILURE_LOG_INTERVAL)
                {
                    warn!(
                        error = %err,
                        capture_fail_streak = gpu_capture_fail_streak,
                        "h264 dirty rect capture failed"
                    );
                }
                if !stream_announced
                    && gpu_capture_fail_streak >= MODERN_CAPTURE_STARTUP_FAILURE_LIMIT
                {
                    return Err(anyhow::anyhow!("h264 dirty rect capture failed: {err}"));
                }
                backend = None;
                cpu_encoder = None;
                init_started_at = Instant::now();
                init_attempt = 0;
                force_keyframe = true;
                gpu_capture_logged = false;
                previous_frame = None;
                atlas_bgra.clear();
                classifier.reset();
                std::thread::sleep(interval);
                continue;
            }
        };
        match capture {
            TextureCaptureResult::Frame(texture_frame) => {
                if cpu_encoder.is_none() {
                    let new_encoder = mf_h264::H264Encoder::new(
                        texture_frame.width,
                        texture_frame.height,
                        fps.max(1),
                        Some(h264_bitrate_bps(current_tuning)),
                    )
                    .map_err(anyhow::Error::msg)?;
                    debug!(
                        pipe_name = ?output.pipe_name(),
                        frame_width = texture_frame.width,
                        frame_height = texture_frame.height,
                        fps = fps.max(1),
                        input_format = new_encoder.input_format().label(),
                        "h264 dirty rect cpu atlas encoder initialized"
                    );
                    output.log_helper(
                        "h264_dirty_rect_encoder_initialized",
                        serde_json::json!({
                            "frame_width": texture_frame.width,
                            "frame_height": texture_frame.height,
                            "fps": fps.max(1),
                            "path": "cpu",
                            "input_format": new_encoder.input_format().label(),
                        }),
                    );
                    cpu_encoder = Some(new_encoder);
                }
                if atlas_bgra.len()
                    != texture_frame.width as usize * texture_frame.height as usize * 4
                {
                    atlas_bgra =
                        vec![0u8; texture_frame.width as usize * texture_frame.height as usize * 4];
                }
                if !gpu_capture_logged {
                    output.log_gpu_capture_active(mode, texture_frame.width, texture_frame.height);
                    gpu_capture_logged = true;
                }
                let pipe_name = output.pipe_name();
                let accumulated_frames = metadata.accumulated_frames;
                let rects_coalesced = metadata.rects_coalesced;
                let mut move_rects = Vec::with_capacity(metadata.move_rects.len());
                let mut invalid_move_rect = false;
                for rect in metadata.move_rects {
                    match clip_move_rect(rect, texture_frame.width, texture_frame.height) {
                        Some(clipped) => move_rects.push(clipped),
                        None => {
                            invalid_move_rect = true;
                            warn!(
                                ?pipe_name,
                                frame_id,
                                src_x = rect.source_x,
                                src_y = rect.source_y,
                                dst_x = rect.left,
                                dst_y = rect.top,
                                dst_right = rect.right,
                                dst_bottom = rect.bottom,
                                frame_width = texture_frame.width,
                                frame_height = texture_frame.height,
                                "h264 dirty rect move rect exceeded framebuffer; promoting frame to keyframe"
                            );
                        }
                    }
                }

                let mut dirty_rects = metadata.dirty_rects;
                let metadata_had_dirty_rects = !dirty_rects.is_empty();
                let mut full_frame_for_keyframe: Option<crate::capture::Frame> = None;
                if dirty_rects.is_empty() {
                    let full_frame = backend
                        .as_mut()
                        .expect("backend should exist for h264 dirty rect full-frame readback")
                        .readback_full_frame(&texture_frame)
                        .map_err(|err| {
                            anyhow::anyhow!("h264 dirty rect full-frame readback failed: {err}")
                        })?;
                    dirty_rects = synthesize_dirty_rects_from_frame_diff(
                        previous_frame.as_ref(),
                        &full_frame,
                    );
                    full_frame_for_keyframe = Some(full_frame);
                }
                let synthetic_rects = !metadata_had_dirty_rects && !dirty_rects.is_empty();
                let descriptor = DisplayStreamDescriptor::modern_capture(
                    texture_frame.width,
                    texture_frame.height,
                );
                if !stream_announced {
                    if let Some(msg) = build_stream_metadata(
                        current_tuning,
                        fps,
                        agent_monitor_hz,
                        active_capture_output_index as u32,
                        Some(descriptor),
                        "modern_gpu",
                    ) {
                        let _ = output.write_metadata(&msg);
                    }
                    stream_announced = true;
                }

                let should_keyframe = force_keyframe
                    || frame_id == 0
                    || last_keyframe_at.elapsed() >= KEYFRAME_INTERVAL
                    || invalid_move_rect;
                if should_keyframe {
                    let encode_started_at = Instant::now();
                    let (encoded, raw_bytes, frame_width, frame_height, full_frame_for_shadow) = {
                        let full_frame = if let Some(frame) = full_frame_for_keyframe.take() {
                            frame
                        } else {
                            backend
                                .as_mut()
                                .expect(
                                    "backend should exist for h264 dirty rect keyframe readback",
                                )
                                .readback_full_frame(&texture_frame)
                                .map_err(|err| {
                                    anyhow::anyhow!(
                                        "h264 dirty rect keyframe readback failed: {err}"
                                    )
                                })?
                        };
                        atlas_bgra.copy_from_slice(&full_frame.data);
                        let i420 = bgra_bytes_to_i420(
                            &atlas_bgra,
                            full_frame.width,
                            full_frame.height,
                            full_frame.width * 4,
                            false,
                        )?;
                        let encoded = cpu_encoder
                            .as_mut()
                            .expect("cpu encoder should exist")
                            .encode_i420(&i420, true)
                            .map_err(anyhow::Error::msg)?
                            .ok_or_else(|| {
                                anyhow::anyhow!("h264 encoder returned no keyframe output")
                            })?;
                        (
                            encoded,
                            full_frame.data.len(),
                            full_frame.width,
                            full_frame.height,
                            Some(full_frame),
                        )
                    };
                    output.write_display_delta(&build_display_frame_begin(
                        frame_id,
                        frame_width,
                        frame_height,
                    ))?;
                    let keyframe_rect = [DisplayAtlasRect {
                        dst_x: 0,
                        dst_y: 0,
                        width: frame_width,
                        height: frame_height,
                        atlas_x: 0,
                        atlas_y: 0,
                    }];
                    output.write_display_delta(&build_display_atlas_h264(
                        frame_id,
                        DISPLAY_ATLAS_H264_FLAG_KEYFRAME,
                        frame_width,
                        frame_height,
                        &keyframe_rect,
                        &encoded.payload,
                    ))?;
                    output.write_display_delta(&build_display_frame_end(frame_id))?;
                    output.log_helper(
                        "h264_dirty_rect_keyframe_emitted",
                        serde_json::json!({
                            "frame_id": frame_id,
                            "payload_len": encoded.payload.len(),
                            "clean_point": encoded.clean_point,
                        }),
                    );
                    output.log_frame_stats(DirtyRectFrameLogStats {
                        frame_id,
                        dirty_rect_count: 1,
                        move_rect_count: 0,
                        raw_bytes,
                        compressed_bytes: encoded.payload.len(),
                        compress_time: encode_started_at.elapsed(),
                        total_time: start.elapsed(),
                        accumulated_frames,
                        rects_coalesced,
                        keyframe: true,
                        synthetic_rects: false,
                    });
                    previous_frame = full_frame_for_shadow;
                    force_keyframe = false;
                    last_keyframe_at = Instant::now();
                    current_frame_dimensions = Some((frame_width, frame_height));
                    frame_id = frame_id.saturating_add(1);
                    continue;
                }

                let clipped_dirty_rects: Vec<_> = dirty_rects
                    .into_iter()
                    .filter_map(|rect| {
                        clip_dirty_rect(rect, texture_frame.width, texture_frame.height)
                    })
                    .collect();
                classify_dirty_rects_with_gpu(
                    output,
                    &mut classifier,
                    backend
                        .as_ref()
                        .expect("backend should exist for h264 dirty rect classifier"),
                    &texture_frame,
                    &clipped_dirty_rects,
                    "modern_capture_gpu_cpu_encode",
                );

                let packed_rects = if clipped_dirty_rects.is_empty() {
                    Some(Vec::new())
                } else {
                    pack_dirty_rects_into_atlas(
                        texture_frame.width,
                        texture_frame.height,
                        &clipped_dirty_rects,
                    )
                };
                let Some(packed_rects) = packed_rects else {
                    force_keyframe = true;
                    continue;
                };

                let mut raw_bytes = 0usize;
                let move_rect_count = move_rects.len();
                let mut dirty_rect_count = 0usize;
                let mut compressed_bytes = 0usize;
                let mut encode_time = Duration::ZERO;
                let atlas_rects: Vec<_> = packed_rects.iter().map(|packed| packed.atlas).collect();
                let mut full_frame_for_previous = None;
                if !packed_rects.is_empty() {
                    let full_frame = backend
                        .as_mut()
                        .expect("backend should exist for h264 dirty rect full-frame readback")
                        .readback_full_frame(&texture_frame)
                        .map_err(|err| {
                            anyhow::anyhow!("h264 dirty rect full-frame readback failed: {err}")
                        })?;
                    if atlas_bgra.len() != full_frame.data.len() {
                        force_keyframe = true;
                        continue;
                    }
                    atlas_bgra.copy_from_slice(&full_frame.data);
                    for packed in &packed_rects {
                        let width = packed.source.right - packed.source.left;
                        let height = packed.source.bottom - packed.source.top;
                        raw_bytes = raw_bytes.saturating_add(width as usize * height as usize * 4);
                        dirty_rect_count = dirty_rect_count.saturating_add(1);
                    }
                    full_frame_for_previous = Some(full_frame);
                }
                let encoded_atlas = if dirty_rect_count > 0 {
                    let i420 = bgra_bytes_to_i420(
                        &atlas_bgra,
                        texture_frame.width,
                        texture_frame.height,
                        texture_frame.width * 4,
                        false,
                    )?;
                    let encode_started_at = Instant::now();
                    let encoded = cpu_encoder
                        .as_mut()
                        .expect("cpu encoder should exist")
                        .encode_i420(&i420, false)
                        .map_err(anyhow::Error::msg)?
                        .ok_or_else(|| anyhow::anyhow!("h264 encoder returned no delta output"))?;
                    encode_time = encode_started_at.elapsed();
                    compressed_bytes = encoded.payload.len();
                    if let Some(full_frame) = full_frame_for_previous.as_ref() {
                        previous_frame = Some(full_frame.clone());
                    }
                    Some(encoded)
                } else {
                    None
                };

                if move_rect_count > 0 || encoded_atlas.is_some() {
                    output.write_display_delta(&build_display_frame_begin(
                        frame_id,
                        texture_frame.width,
                        texture_frame.height,
                    ))?;
                    for rect in &move_rects {
                        let mut payload = Vec::with_capacity(32);
                        payload.extend_from_slice(&frame_id.to_le_bytes());
                        payload.extend_from_slice(&rect.src_x.to_le_bytes());
                        payload.extend_from_slice(&rect.src_y.to_le_bytes());
                        payload.extend_from_slice(&rect.dst_x.to_le_bytes());
                        payload.extend_from_slice(&rect.dst_y.to_le_bytes());
                        payload.extend_from_slice(&rect.width.to_le_bytes());
                        payload.extend_from_slice(&rect.height.to_le_bytes());
                        output.write_display_delta(&build_display_record(
                            DISPLAY_RECORD_MOVE_RECT,
                            &payload,
                        ))?;
                    }
                    if let Some(encoded) = encoded_atlas.as_ref() {
                        output.write_display_delta(&build_display_atlas_h264(
                            frame_id,
                            if encoded.clean_point {
                                DISPLAY_ATLAS_H264_FLAG_KEYFRAME
                            } else {
                                0
                            },
                            texture_frame.width,
                            texture_frame.height,
                            &atlas_rects,
                            &encoded.payload,
                        ))?;
                    }
                    output.write_display_delta(&build_display_frame_end(frame_id))?;
                    if previous_frame.is_none() {
                        if let Some(full_frame) = full_frame_for_keyframe {
                            previous_frame = Some(full_frame);
                        }
                    }
                    output.log_frame_stats(DirtyRectFrameLogStats {
                        frame_id,
                        dirty_rect_count,
                        move_rect_count,
                        raw_bytes,
                        compressed_bytes,
                        compress_time: encode_time,
                        total_time: start.elapsed(),
                        accumulated_frames,
                        rects_coalesced,
                        keyframe: false,
                        synthetic_rects,
                    });
                    frame_id = frame_id.saturating_add(1);
                }
            }
            TextureCaptureResult::Timeout => {
                if !stream_announced && init_started_at.elapsed() >= Duration::from_millis(1500) {
                    let gdi_frame =
                        match crate::capture::windows::GdiBackend::new().and_then(|mut gdi| {
                            let _ = gdi.set_capture_source_index(active_capture_output_index);
                            gdi.capture_frame()
                        }) {
                            Ok(frame) => {
                                output.log_helper(
                                    "h264_dirty_rect_timeout_gdi_bootstrap",
                                    serde_json::json!({
                                        "frame_width": frame.width,
                                        "frame_height": frame.height,
                                        "elapsed_ms": init_started_at.elapsed().as_millis() as u64,
                                    }),
                                );
                                Some(frame)
                            }
                            Err(err) => {
                                output.log_helper(
                                    "h264_dirty_rect_timeout_gdi_bootstrap_failed",
                                    serde_json::json!({
                                        "error": err.to_string(),
                                        "elapsed_ms": init_started_at.elapsed().as_millis() as u64,
                                    }),
                                );
                                None
                            }
                        };
                    let (width, height) = gdi_frame
                        .as_ref()
                        .map(|frame| (frame.width, frame.height))
                        .or_else(primary_display_dimensions)
                        .unwrap_or((1024, 768));
                    let width = width & !1;
                    let height = height & !1;
                    if width == 0 || height == 0 {
                        std::thread::sleep(interval);
                        continue;
                    }
                    if cpu_encoder.is_none() {
                        let new_encoder = mf_h264::H264Encoder::new(
                            width,
                            height,
                            fps.max(1),
                            Some(h264_bitrate_bps(current_tuning)),
                        )
                        .map_err(anyhow::Error::msg)?;
                        output.log_helper(
                            "h264_dirty_rect_encoder_initialized",
                            serde_json::json!({
                                "frame_width": width,
                                "frame_height": height,
                                "fps": fps.max(1),
                                "bootstrap": true,
                                "path": "cpu",
                            }),
                        );
                        cpu_encoder = Some(new_encoder);
                    }
                    if atlas_bgra.len() != width as usize * height as usize * 4 {
                        atlas_bgra = vec![0u8; width as usize * height as usize * 4];
                    }
                    if let Some(frame) = gdi_frame.as_ref() {
                        if frame.width == width
                            && frame.height == height
                            && frame.stride == width * 4
                            && frame.data.len() == atlas_bgra.len()
                        {
                            atlas_bgra.copy_from_slice(&frame.data);
                        } else {
                            let row_bytes = width as usize * 4;
                            for row in 0..height.min(frame.height) as usize {
                                let src = row * frame.stride as usize;
                                let dst = row * row_bytes;
                                let available = frame.data.len().saturating_sub(src).min(row_bytes);
                                if available == row_bytes && dst + row_bytes <= atlas_bgra.len() {
                                    atlas_bgra[dst..dst + row_bytes]
                                        .copy_from_slice(&frame.data[src..src + row_bytes]);
                                }
                            }
                        }
                    }
                    let descriptor = DisplayStreamDescriptor::modern_capture(width, height);
                    if let Some(msg) = build_stream_metadata(
                        current_tuning,
                        fps,
                        agent_monitor_hz,
                        active_capture_output_index as u32,
                        Some(descriptor),
                        "modern_gpu",
                    ) {
                        let _ = output.write_metadata(&msg);
                    }
                    let i420 = bgra_bytes_to_i420(&atlas_bgra, width, height, width * 4, false)?;
                    let encoded = cpu_encoder
                        .as_mut()
                        .expect("cpu encoder should exist")
                        .encode_i420(&i420, true)
                        .map_err(anyhow::Error::msg)?;
                    if let Some(encoded) = encoded {
                        output.write_display_delta(&build_display_frame_begin(
                            frame_id, width, height,
                        ))?;
                        let keyframe_rect = [DisplayAtlasRect {
                            dst_x: 0,
                            dst_y: 0,
                            width,
                            height,
                            atlas_x: 0,
                            atlas_y: 0,
                        }];
                        output.write_display_delta(&build_display_atlas_h264(
                            frame_id,
                            DISPLAY_ATLAS_H264_FLAG_KEYFRAME,
                            width,
                            height,
                            &keyframe_rect,
                            &encoded.payload,
                        ))?;
                        output.write_display_delta(&build_display_frame_end(frame_id))?;
                        stream_announced = true;
                        last_keyframe_at = Instant::now();
                        current_frame_dimensions = Some((width, height));
                        frame_id = frame_id.saturating_add(1);
                    }
                } else if stream_announced
                    && last_keyframe_at.elapsed() >= STATIC_REFRESH_INTERVAL
                    && !atlas_bgra.is_empty()
                {
                    if let (Some((width, height)), Some(encoder)) =
                        (current_frame_dimensions, cpu_encoder.as_mut())
                    {
                        let expected_len = width as usize * height as usize * 4;
                        if atlas_bgra.len() == expected_len {
                            let i420 =
                                bgra_bytes_to_i420(&atlas_bgra, width, height, width * 4, false)?;
                            let encoded = encoder
                                .encode_i420(&i420, true)
                                .map_err(anyhow::Error::msg)?;
                            if let Some(encoded) = encoded {
                                output.write_display_delta(&build_display_frame_begin(
                                    frame_id, width, height,
                                ))?;
                                let keyframe_rect = [DisplayAtlasRect {
                                    dst_x: 0,
                                    dst_y: 0,
                                    width,
                                    height,
                                    atlas_x: 0,
                                    atlas_y: 0,
                                }];
                                output.write_display_delta(&build_display_atlas_h264(
                                    frame_id,
                                    DISPLAY_ATLAS_H264_FLAG_KEYFRAME,
                                    width,
                                    height,
                                    &keyframe_rect,
                                    &encoded.payload,
                                ))?;
                                output.write_display_delta(&build_display_frame_end(frame_id))?;
                                output.log_helper(
                                    "h264_dirty_rect_static_keyframe_refresh",
                                    serde_json::json!({
                                        "frame_id": frame_id,
                                        "payload_len": encoded.payload.len(),
                                        "clean_point": encoded.clean_point,
                                    }),
                                );
                                last_keyframe_at = Instant::now();
                                frame_id = frame_id.saturating_add(1);
                            }
                        }
                    }
                }
            }
            TextureCaptureResult::AccessLost => {
                backend = None;
                cpu_encoder = None;
                init_started_at = Instant::now();
                init_attempt = 0;
                force_keyframe = true;
                gpu_capture_logged = false;
                previous_frame = None;
                atlas_bgra.clear();
                current_frame_dimensions = None;
                classifier.reset();
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn emit_dirty_rect_stream_h264_cpu(
    output: DirtyRectStreamOutput<'_>,
    tuning: EncodeTuning,
    fps: u32,
    stop: Arc<AtomicBool>,
    capture_output_switch_rx: &mpsc::Receiver<usize>,
    stream_bitrate_rx: &mpsc::Receiver<u32>,
) -> Result<()> {
    use crate::capture::windows::GdiBackend;
    use crate::capture::CaptureBackend;

    crate::control::attach_thread_to_input_desktop();
    let interval = if fps > 0 {
        Duration::from_secs_f64(1.0 / fps as f64)
    } else {
        Duration::from_millis(100)
    };
    let agent_monitor_hz = cfg_primary_monitor_hz();
    let mut backend: Option<GdiBackend> = None;
    let mut capture_source_index = 0usize;
    let mut pending_capture_source_index: Option<usize> = None;
    let mut cpu_encoder: Option<mf_h264::H264Encoder> = None;
    let mut frame_dimensions: Option<(u32, u32)> = None;
    let mut frame_id = 0u64;
    let mut force_keyframe = true;
    let mut stream_announced = false;
    let mut previous_frame: Option<crate::capture::Frame> = None;
    let mut atlas_bgra: Vec<u8> = Vec::new();
    let mut init_attempt: u32 = 0;
    let mut capture_fail_streak: u32 = 0;
    let mut current_tuning = tuning;
    let mut cpu_capture_logged = false;
    const KEYFRAME_INTERVAL: Duration = Duration::from_millis(500);
    let mut last_keyframe_at = Instant::now() - Duration::from_secs(60);
    let mut context_poller = DesktopContextPoller::new();

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        if let Some(new_context) = context_poller.poll_transition() {
            debug!(
                context = ?new_context,
                "desktop context changed; reinitializing cpu dirty-rect capture backend"
            );
            crate::control::attach_thread_to_input_desktop();
            backend = None;
            cpu_encoder = None;
            frame_dimensions = None;
            previous_frame = None;
            atlas_bgra.clear();
            force_keyframe = true;
            stream_announced = false;
            cpu_capture_logged = false;
        }

        if let Some(kbps) = pending_stream_bitrate_from_rx(stream_bitrate_rx) {
            if apply_stream_bitrate_update(&mut current_tuning, kbps) {
                cpu_encoder = None;
                force_keyframe = true;
                stream_announced = false;
                output.log_helper(
                    "h264_dirty_rect_bitrate_updated",
                    serde_json::json!({ "kbps": kbps, "path": "cpu_capture" }),
                );
            }
        }

        if let Some(idx) = pending_capture_index_from_rx(capture_output_switch_rx) {
            pending_capture_source_index = Some(idx);
            if let Some(backend_ref) = backend.as_mut() {
                if let Err(err) = backend_ref.set_capture_source_index(idx) {
                    warn!(
                        error = %err,
                        index = idx,
                        "cpu dirty-rect capture output switch failed"
                    );
                    output.log_helper(
                        "h264_dirty_rect_cpu_capture_output_switch_failed",
                        serde_json::json!({
                            "index": idx,
                            "error": err.to_string(),
                        }),
                    );
                } else {
                    capture_source_index = backend_ref.capture_source_index();
                    pending_capture_source_index = None;
                    crate::control::set_remote_input_capture_output_index(capture_source_index);
                    if let Some(msg) = build_stream_metadata(
                        current_tuning,
                        fps,
                        agent_monitor_hz,
                        capture_source_index as u32,
                        None,
                        "modern_cpu",
                    ) {
                        let _ = output.write_metadata(&msg);
                    }
                    cpu_encoder = None;
                    frame_dimensions = None;
                    previous_frame = None;
                    atlas_bgra.clear();
                    force_keyframe = true;
                    stream_announced = false;
                    cpu_capture_logged = false;
                }
            }
        }

        if backend.is_none() {
            init_attempt = init_attempt.saturating_add(1);
            crate::control::attach_thread_to_input_desktop();
            match GdiBackend::new() {
                Ok(mut new_backend) => {
                    let desired_capture_output_index =
                        pending_capture_source_index.unwrap_or(capture_source_index);
                    if desired_capture_output_index != 0 {
                        if let Err(err) =
                            new_backend.set_capture_source_index(desired_capture_output_index)
                        {
                            if init_attempt == 1
                                || init_attempt
                                    .is_multiple_of(MODERN_CAPTURE_RUNTIME_FAILURE_LOG_INTERVAL)
                            {
                                warn!(
                                    attempt = init_attempt,
                                    index = desired_capture_output_index,
                                    error = %err,
                                    "cpu dirty-rect capture backend output selection failed"
                                );
                            }
                            capture_source_index = 0;
                        } else {
                            capture_source_index = desired_capture_output_index;
                            pending_capture_source_index = None;
                        }
                    } else {
                        capture_source_index = 0;
                        pending_capture_source_index = None;
                    }
                    crate::control::set_remote_input_capture_output_index(capture_source_index);
                    if let Some(msg) = build_stream_metadata(
                        current_tuning,
                        fps,
                        agent_monitor_hz,
                        capture_source_index as u32,
                        None,
                        "modern_cpu",
                    ) {
                        let _ = output.write_metadata(&msg);
                    }
                    output.log_helper(
                        "cpu_dirty_rect_capture_backend_initialized",
                        serde_json::json!({
                            "attempt": init_attempt,
                            "capture_output_index": capture_source_index,
                        }),
                    );
                    backend = Some(new_backend);
                }
                Err(err) => {
                    if init_attempt == 1
                        || init_attempt.is_multiple_of(MODERN_CAPTURE_RUNTIME_FAILURE_LOG_INTERVAL)
                    {
                        warn!(
                            attempt = init_attempt,
                            error = %err,
                            "cpu dirty-rect capture backend init failed"
                        );
                    }
                    if frame_id == 0 && init_attempt >= MODERN_CAPTURE_STARTUP_FAILURE_LIMIT {
                        return Err(anyhow::anyhow!(
                            "h264 dirty rect cpu capture backend init failed: {err}"
                        ));
                    }
                    std::thread::sleep(interval);
                    continue;
                }
            }
        }

        let start = Instant::now();
        let Some(backend_ref) = backend.as_mut() else {
            std::thread::sleep(interval);
            continue;
        };

        let frame = match backend_ref.capture_frame() {
            Ok(frame) => {
                capture_fail_streak = 0;
                frame
            }
            Err(err) => {
                capture_fail_streak = capture_fail_streak.saturating_add(1);
                if capture_fail_streak == 1
                    || capture_fail_streak
                        .is_multiple_of(MODERN_CAPTURE_RUNTIME_FAILURE_LOG_INTERVAL)
                {
                    warn!(
                        error = %err,
                        capture_fail_streak = capture_fail_streak,
                        "cpu dirty-rect capture failed"
                    );
                }
                if frame_id == 0 && capture_fail_streak >= MODERN_CAPTURE_STARTUP_FAILURE_LIMIT {
                    return Err(anyhow::anyhow!("h264 dirty rect cpu capture failed: {err}"));
                }
                if capture_fail_streak >= MODERN_CAPTURE_STARTUP_FAILURE_LIMIT {
                    backend = None;
                    cpu_encoder = None;
                    frame_dimensions = None;
                    previous_frame = None;
                    atlas_bgra.clear();
                    force_keyframe = true;
                    capture_fail_streak = 0;
                    cpu_capture_logged = false;
                }
                std::thread::sleep(interval);
                continue;
            }
        };

        if frame.width % 2 != 0 || frame.height % 2 != 0 {
            let err = anyhow::anyhow!(
                "h264 dirty rect cpu capture requires even dimensions, got {}x{}",
                frame.width,
                frame.height
            );
            if frame_id == 0 {
                return Err(err);
            }
            warn!(
                width = frame.width,
                height = frame.height,
                "cpu dirty-rect capture produced odd dimensions"
            );
            backend = None;
            cpu_encoder = None;
            frame_dimensions = None;
            previous_frame = None;
            atlas_bgra.clear();
            force_keyframe = true;
            cpu_capture_logged = false;
            std::thread::sleep(interval);
            continue;
        }

        let dimensions = (frame.width, frame.height);
        if frame_dimensions != Some(dimensions) {
            cpu_encoder = None;
            frame_dimensions = Some(dimensions);
            previous_frame = None;
            atlas_bgra.clear();
            force_keyframe = true;
            stream_announced = false;
        }

        if cpu_encoder.is_none() {
            match mf_h264::H264Encoder::new(
                frame.width,
                frame.height,
                fps.max(1),
                Some(h264_bitrate_bps(current_tuning)),
            ) {
                Ok(new_encoder) => {
                    debug!(
                        pipe_name = ?output.pipe_name(),
                        frame_width = frame.width,
                        frame_height = frame.height,
                        fps = fps.max(1),
                        input_format = new_encoder.input_format().label(),
                        "h264 dirty rect cpu capture encoder initialized"
                    );
                    output.log_helper(
                        "h264_dirty_rect_encoder_initialized",
                        serde_json::json!({
                            "frame_width": frame.width,
                            "frame_height": frame.height,
                            "fps": fps.max(1),
                            "path": "cpu_capture",
                            "input_format": new_encoder.input_format().label(),
                        }),
                    );
                    cpu_encoder = Some(new_encoder);
                }
                Err(err) => {
                    if frame_id == 0 {
                        return Err(anyhow::anyhow!(
                            "h264 dirty rect cpu capture encoder init failed: {err}"
                        ));
                    }
                    warn!(
                        error = %err,
                        frame_width = frame.width,
                        frame_height = frame.height,
                        "h264 dirty rect cpu capture encoder init failed; retrying"
                    );
                    cpu_encoder = None;
                    force_keyframe = true;
                    stream_announced = false;
                    std::thread::sleep(interval);
                    continue;
                }
            }
        }

        if atlas_bgra.len() != frame.width as usize * frame.height as usize * 4 {
            atlas_bgra = vec![0u8; frame.width as usize * frame.height as usize * 4];
        }
        if !cpu_capture_logged {
            debug!(
                pipe_name = ?output.pipe_name(),
                frame_width = frame.width,
                frame_height = frame.height,
                "cpu dirty rect capture active"
            );
            output.log_helper(
                "cpu_dirty_rect_capture_active",
                serde_json::json!({
                    "frame_width": frame.width,
                    "frame_height": frame.height,
                }),
            );
            cpu_capture_logged = true;
        }

        let descriptor = DisplayStreamDescriptor::modern_capture(frame.width, frame.height);
        if !stream_announced {
            if let Some(msg) = build_stream_metadata(
                current_tuning,
                fps,
                agent_monitor_hz,
                capture_source_index as u32,
                Some(descriptor),
                "modern_cpu",
            ) {
                let _ = output.write_metadata(&msg);
            }
            stream_announced = true;
        }

        let should_keyframe = force_keyframe
            || frame_id == 0
            || previous_frame.is_none()
            || last_keyframe_at.elapsed() >= KEYFRAME_INTERVAL;
        if should_keyframe {
            atlas_bgra.copy_from_slice(&frame.data);
            let i420 = match bgra_bytes_to_i420(
                &atlas_bgra,
                frame.width,
                frame.height,
                frame.width * 4,
                false,
            ) {
                Ok(i420) => i420,
                Err(err) => {
                    if frame_id == 0 {
                        return Err(err);
                    }
                    warn!(error = %err, "cpu dirty-rect keyframe conversion failed; retrying");
                    cpu_encoder = None;
                    force_keyframe = true;
                    std::thread::sleep(interval);
                    continue;
                }
            };

            let encoded = cpu_encoder
                .as_mut()
                .expect("cpu encoder should exist")
                .encode_i420(&i420, true)
                .map_err(anyhow::Error::msg);
            let encoded = match encoded {
                Ok(Some(encoded)) => encoded,
                Ok(None) => {
                    let err = anyhow::anyhow!("h264 encoder returned no keyframe output");
                    if frame_id == 0 {
                        return Err(err);
                    }
                    warn!("cpu dirty-rect keyframe encode returned no output; retrying");
                    cpu_encoder = None;
                    force_keyframe = true;
                    std::thread::sleep(interval);
                    continue;
                }
                Err(err) => {
                    if frame_id == 0 {
                        return Err(err);
                    }
                    warn!(error = %err, "cpu dirty-rect keyframe encode failed; retrying");
                    cpu_encoder = None;
                    force_keyframe = true;
                    std::thread::sleep(interval);
                    continue;
                }
            };

            let encode_time = start.elapsed();
            output.write_display_delta(&build_display_frame_begin(
                frame_id,
                frame.width,
                frame.height,
            ))?;
            let keyframe_rect = [DisplayAtlasRect {
                dst_x: 0,
                dst_y: 0,
                width: frame.width,
                height: frame.height,
                atlas_x: 0,
                atlas_y: 0,
            }];
            output.write_display_delta(&build_display_atlas_h264(
                frame_id,
                DISPLAY_ATLAS_H264_FLAG_KEYFRAME,
                frame.width,
                frame.height,
                &keyframe_rect,
                &encoded.payload,
            ))?;
            output.write_display_delta(&build_display_frame_end(frame_id))?;
            output.log_frame_stats(DirtyRectFrameLogStats {
                frame_id,
                dirty_rect_count: 1,
                move_rect_count: 0,
                raw_bytes: frame.data.len(),
                compressed_bytes: encoded.payload.len(),
                compress_time: encode_time,
                total_time: start.elapsed(),
                accumulated_frames: 1,
                rects_coalesced: false,
                keyframe: true,
                synthetic_rects: false,
            });
            previous_frame = Some(frame);
            force_keyframe = false;
            last_keyframe_at = Instant::now();
            frame_id = frame_id.saturating_add(1);
            let elapsed = start.elapsed();
            if elapsed < interval {
                std::thread::sleep(interval - elapsed);
            }
            continue;
        }

        let metadata =
            match synthesize_cpu_frame_metadata_from_diff(previous_frame.as_ref(), &frame) {
                Ok(metadata) => metadata,
                Err(err) => {
                    warn!(
                        error = %err,
                        "cpu move rect synthesis failed; promoting to keyframe"
                    );
                    force_keyframe = true;
                    let elapsed = start.elapsed();
                    if elapsed < interval {
                        std::thread::sleep(interval - elapsed);
                    }
                    continue;
                }
            };
        let mut move_rects = Vec::with_capacity(metadata.move_rects.len());
        let mut invalid_move_rect = false;
        for rect in metadata.move_rects {
            match clip_move_rect(rect, frame.width, frame.height) {
                Some(clipped) => move_rects.push(clipped),
                None => {
                    invalid_move_rect = true;
                    warn!(
                        src_x = rect.source_x,
                        src_y = rect.source_y,
                        dst_x = rect.left,
                        dst_y = rect.top,
                        dst_right = rect.right,
                        dst_bottom = rect.bottom,
                        frame_width = frame.width,
                        frame_height = frame.height,
                        "cpu synthesized move rect exceeded framebuffer; promoting to keyframe"
                    );
                    break;
                }
            }
        }
        if invalid_move_rect {
            force_keyframe = true;
            let elapsed = start.elapsed();
            if elapsed < interval {
                std::thread::sleep(interval - elapsed);
            }
            continue;
        }
        let clipped_dirty_rects: Vec<_> = metadata
            .dirty_rects
            .into_iter()
            .filter_map(|rect| clip_dirty_rect(rect, frame.width, frame.height))
            .collect();
        let move_rect_count = move_rects.len();
        if move_rect_count == 0 && clipped_dirty_rects.is_empty() {
            previous_frame = Some(frame);
            let elapsed = start.elapsed();
            if elapsed < interval {
                std::thread::sleep(interval - elapsed);
            }
            continue;
        }

        if move_rect_count > 0 {
            debug!(
                move_rect_count,
                dirty_rect_count = clipped_dirty_rects.len(),
                "cpu move rect synthesis emitted move rects"
            );
        }
        if !clipped_dirty_rects.is_empty() {
            log_skipped_dirty_rect_classifier(
                output,
                "modern_capture_cpu_capture",
                clipped_dirty_rects.len(),
                "cpu_capture_backend",
            );
        }

        let packed_rects = match if clipped_dirty_rects.is_empty() {
            Some(Vec::new())
        } else {
            pack_dirty_rects_into_atlas(frame.width, frame.height, &clipped_dirty_rects)
        } {
            Some(packed) => packed,
            None => {
                force_keyframe = true;
                let elapsed = start.elapsed();
                if elapsed < interval {
                    std::thread::sleep(interval - elapsed);
                }
                continue;
            }
        };

        let mut raw_bytes = 0usize;
        let dirty_rect_count = packed_rects.len();
        if dirty_rect_count > 0 {
            if atlas_bgra.len() != frame.data.len() {
                force_keyframe = true;
                let elapsed = start.elapsed();
                if elapsed < interval {
                    std::thread::sleep(interval - elapsed);
                }
                continue;
            }
            atlas_bgra.copy_from_slice(&frame.data);
            for packed in &packed_rects {
                let width = packed.source.right - packed.source.left;
                let height = packed.source.bottom - packed.source.top;
                raw_bytes = raw_bytes.saturating_add(width as usize * height as usize * 4);
            }
        }

        let mut compressed_bytes = 0usize;
        let mut encode_time = Duration::ZERO;
        let encoded_atlas = if dirty_rect_count > 0 {
            let i420 = match bgra_bytes_to_i420(
                &atlas_bgra,
                frame.width,
                frame.height,
                frame.width * 4,
                false,
            ) {
                Ok(i420) => i420,
                Err(err) => {
                    warn!(error = %err, "cpu dirty-rect delta conversion failed; promoting to keyframe");
                    force_keyframe = true;
                    let elapsed = start.elapsed();
                    if elapsed < interval {
                        std::thread::sleep(interval - elapsed);
                    }
                    continue;
                }
            };

            let encode_started_at = Instant::now();
            let encoded = cpu_encoder
                .as_mut()
                .expect("cpu encoder should exist")
                .encode_i420(&i420, false)
                .map_err(anyhow::Error::msg);
            let encoded = match encoded {
                Ok(Some(encoded)) => encoded,
                Ok(None) => {
                    warn!("cpu dirty-rect delta encode returned no output; promoting to keyframe");
                    force_keyframe = true;
                    let elapsed = start.elapsed();
                    if elapsed < interval {
                        std::thread::sleep(interval - elapsed);
                    }
                    continue;
                }
                Err(err) => {
                    warn!(error = %err, "cpu dirty-rect delta encode failed; promoting to keyframe");
                    cpu_encoder = None;
                    force_keyframe = true;
                    let elapsed = start.elapsed();
                    if elapsed < interval {
                        std::thread::sleep(interval - elapsed);
                    }
                    continue;
                }
            };
            encode_time = encode_started_at.elapsed();
            compressed_bytes = encoded.payload.len();
            Some(encoded)
        } else {
            None
        };

        if move_rect_count == 0 && encoded_atlas.is_none() {
            previous_frame = Some(frame);
            let elapsed = start.elapsed();
            if elapsed < interval {
                std::thread::sleep(interval - elapsed);
            }
            continue;
        }

        let atlas_rects: Vec<_> = packed_rects.iter().map(|packed| packed.atlas).collect();
        output.write_display_delta(&build_display_frame_begin(
            frame_id,
            frame.width,
            frame.height,
        ))?;
        for rect in &move_rects {
            let mut payload = Vec::with_capacity(32);
            payload.extend_from_slice(&frame_id.to_le_bytes());
            payload.extend_from_slice(&rect.src_x.to_le_bytes());
            payload.extend_from_slice(&rect.src_y.to_le_bytes());
            payload.extend_from_slice(&rect.dst_x.to_le_bytes());
            payload.extend_from_slice(&rect.dst_y.to_le_bytes());
            payload.extend_from_slice(&rect.width.to_le_bytes());
            payload.extend_from_slice(&rect.height.to_le_bytes());
            output
                .write_display_delta(&build_display_record(DISPLAY_RECORD_MOVE_RECT, &payload))?;
        }
        if let Some(encoded) = encoded_atlas.as_ref() {
            output.write_display_delta(&build_display_atlas_h264(
                frame_id,
                if encoded.clean_point {
                    DISPLAY_ATLAS_H264_FLAG_KEYFRAME
                } else {
                    0
                },
                frame.width,
                frame.height,
                &atlas_rects,
                &encoded.payload,
            ))?;
        }
        output.write_display_delta(&build_display_frame_end(frame_id))?;
        output.log_frame_stats(DirtyRectFrameLogStats {
            frame_id,
            dirty_rect_count,
            move_rect_count,
            raw_bytes,
            compressed_bytes,
            compress_time: encode_time,
            total_time: start.elapsed(),
            accumulated_frames: 1,
            rects_coalesced: false,
            keyframe: false,
            synthetic_rects: true,
        });
        previous_frame = Some(frame);
        frame_id = frame_id.saturating_add(1);

        let elapsed = start.elapsed();
        if elapsed < interval {
            std::thread::sleep(interval - elapsed);
        }
    }

    Ok(())
}

#[cfg(windows)]
fn emit_dirty_rect_stream_h264(
    output: DirtyRectStreamOutput<'_>,
    tuning: EncodeTuning,
    fps: u32,
    stop: Arc<AtomicBool>,
    mode: crate::display_processing::DisplayProcessingMode,
    capture_output_switch_rx: &mpsc::Receiver<usize>,
    stream_bitrate_rx: &mpsc::Receiver<u32>,
) -> Result<()> {
    if !mode.prefers_gpu() {
        return emit_dirty_rect_stream_h264_cpu(
            output,
            tuning,
            fps,
            stop.clone(),
            capture_output_switch_rx,
            stream_bitrate_rx,
        );
    }

    match emit_dirty_rect_stream_h264_gpu(
        output,
        tuning,
        fps,
        stop.clone(),
        mode,
        capture_output_switch_rx,
        stream_bitrate_rx,
    ) {
        Ok(()) => Ok(()),
        Err(err) if mode.allows_cpu_fallback() => {
            warn!(
                error = %err,
                "gpu dirty-rect capture startup failed; falling back to cpu capture"
            );
            output.log_helper(
                "gpu_dirty_rect_capture_startup_failed_fallback_cpu",
                serde_json::json!({ "error": err.to_string() }),
            );
            emit_dirty_rect_stream_h264_cpu(
                output,
                tuning,
                fps,
                stop,
                capture_output_switch_rx,
                stream_bitrate_rx,
            )
        }
        Err(err) => Err(err),
    }
}

/// Runs capture → encode → IVF chunks into the channel until the receiver is dropped
/// or the stop flag is set. Call from a dedicated thread.
#[cfg(windows)]
pub fn run_capture_encode_stream_with_stop(
    tx: mpsc::Sender<IvfChunk>,
    tuning: EncodeTuning,
    fps: u32,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    use crate::capture::windows;
    use crate::capture::CaptureBackend;

    let sender = IvfStreamSender::new(tx);
    let processing_mode =
        crate::display_processing::effective_display_processing_mode("agent capture/encode");
    let display_stream_mode = display_stream_mode_for_processing_mode(
        processing_mode,
        if h264_dirty_rect_stream_supported() {
            DisplayStreamMode::ModernCapture
        } else {
            DisplayStreamMode::LegacyCapture
        },
    );
    if display_stream_mode == DisplayStreamMode::ScreenshotOnly {
        return emit_screenshot_only_bgra(
            DirtyRectStreamOutput::Channel(&sender),
            tuning,
            fps,
            stop,
        );
    }
    if display_stream_mode == DisplayStreamMode::ModernCapture {
        let (_noop_capture_switch_tx, noop_capture_switch_rx) = mpsc::channel::<usize>();
        let (_noop_stream_bitrate_tx, noop_stream_bitrate_rx) = mpsc::channel::<u32>();
        match emit_dirty_rect_stream_h264(
            DirtyRectStreamOutput::Channel(&sender),
            tuning,
            fps,
            stop.clone(),
            processing_mode,
            &noop_capture_switch_rx,
            &noop_stream_bitrate_rx,
        ) {
            Ok(()) => return Ok(()),
            Err(err) => {
                warn!(
                    error = %err,
                    "modern capture startup failed; falling back to legacy capture"
                );
            }
        }
    }
    let interval = if fps > 0 {
        Duration::from_secs_f64(1.0 / fps as f64)
    } else {
        Duration::from_millis(100)
    };
    let grayscale = tuning.preset.grayscale_chroma();

    // `RMM_DISPLAY_PROCESSING_MODE=legacy`: full-frame GDI capture + VP8 software encode only
    // (no DXGI Desktop Duplication, no D3D11 capture).
    let legacy_cpu_vp8_capture = processing_mode.is_legacy();

    let active_capture_output_index: usize = 0;
    // Ensure the calling thread is attached to the current input desktop before capture.
    crate::control::attach_thread_to_input_desktop();
    let init_started_at = Instant::now();
    let mut init_attempt: u32 = 0;
    let (backend, enc_width, enc_height, expected_len, mut encoder, first_i420) = loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        init_attempt += 1;
        let mut backend: Box<dyn CaptureBackend> = if legacy_cpu_vp8_capture {
            match windows::GdiBackend::new() {
                Ok(mut backend) => {
                    if let Err(err) = backend.set_capture_source_index(active_capture_output_index)
                    {
                        if init_attempt == 1 || init_attempt.is_multiple_of(20) {
                            warn!(
                                attempt = init_attempt,
                                index = active_capture_output_index,
                                error = %err,
                                "legacy VP8: gdi capture output index refused"
                            );
                        }
                        std::thread::sleep(interval);
                        continue;
                    }
                    debug!(
                        attempt = init_attempt,
                        "legacy VP8 IVF: gdi (cpu) capture backend initialized — no DXGI"
                    );
                    Box::new(backend)
                }
                Err(err) => {
                    if init_attempt == 1 || init_attempt.is_multiple_of(20) {
                        warn!(
                            attempt = init_attempt,
                            error = %err,
                            "legacy VP8: gdi capture backend init failed"
                        );
                    }
                    std::thread::sleep(interval);
                    continue;
                }
            }
        } else {
            match windows::DxgiBackend::new() {
                Ok(backend) => Box::new(backend),
                Err(_e) => {
                    std::thread::sleep(interval);
                    continue;
                }
            }
        };

        match backend.capture_frame() {
            Ok(first_frame) => {
                let (enc_width, enc_height) =
                    aligned_stream_dimensions(first_frame.width, first_frame.height);
                let i420 = match bgra_to_i420_scaled(&first_frame, grayscale, enc_width, enc_height)
                {
                    Ok(buf) => buf,
                    Err(_e) => {
                        std::thread::sleep(interval);
                        continue;
                    }
                };
                let expected_len = (enc_width as usize) * (enc_height as usize) * 3 / 2;
                if i420.len() != expected_len {
                    std::thread::sleep(interval);
                    continue;
                }
                let encoder = match vp8::Vp8Encoder::new(enc_width, enc_height, fps, tuning)
                    .context("vp8 encoder init failed")
                {
                    Ok(enc) => enc,
                    Err(_e) => {
                        std::thread::sleep(interval);
                        continue;
                    }
                };
                break (backend, enc_width, enc_height, expected_len, encoder, i420);
            }
            Err(e) => {
                if legacy_cpu_vp8_capture {
                    if init_attempt == 1 || init_attempt.is_multiple_of(20) {
                        warn!(
                            attempt = init_attempt,
                            error = %e,
                            "legacy VP8: first gdi capture_frame failed"
                        );
                    }
                    std::thread::sleep(interval);
                    continue;
                }
                if e.to_string().contains("timed out")
                    && init_started_at.elapsed() >= Duration::from_millis(1500)
                {
                    let (src_width, src_height) =
                        primary_display_dimensions().unwrap_or((1024, 768));
                    let (enc_width, enc_height) = aligned_stream_dimensions(src_width, src_height);
                    let expected_len = (enc_width as usize) * (enc_height as usize) * 3 / 2;
                    let i420 = build_bootstrap_i420_frame(enc_width, enc_height);
                    if i420.len() == expected_len {
                        let encoder = match vp8::Vp8Encoder::new(enc_width, enc_height, fps, tuning)
                            .context("vp8 encoder init failed")
                        {
                            Ok(enc) => enc,
                            Err(_e) => {
                                std::thread::sleep(interval);
                                continue;
                            }
                        };
                        warn!(
                            src_width,
                            src_height,
                            enc_width,
                            enc_height,
                            elapsed_ms = init_started_at.elapsed().as_millis() as u64,
                            "bootstrapping remote desktop stream with synthetic first frame after DXGI timeout"
                        );
                        break (backend, enc_width, enc_height, expected_len, encoder, i420);
                    }
                }
                std::thread::sleep(interval);
            }
        }
    };

    // Send metadata before IVF header for viewer debug overlay. Format: "RMMD" (4) + len (4 LE) + json.
    let agent_monitor_hz = cfg_primary_monitor_hz();
    let mut metadata = serde_json::json!({
        "bitrate_kbps": tuning.bitrate_kbps(),
        "preset": tuning.preset.as_str(),
        "cpu_used": tuning.cpu_used.unwrap_or(2),
        "encoding_fps": fps,
        "agent_monitor_hz": agent_monitor_hz,
        "activeIndex": active_capture_output_index as u32,
        "captureType": "legacy",
    });
    if let Some(obj) = metadata.as_object_mut() {
        merge_capture_outputs_metadata(obj);
    }
    if let Ok(json_bytes) = serde_json::to_vec(&metadata) {
        let mut msg = Vec::with_capacity(8 + json_bytes.len());
        msg.extend_from_slice(b"RMMD");
        msg.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        msg.extend_from_slice(&json_bytes);
        let _ = sender.write_metadata(&msg);
    }

    if sender.write_header(enc_width, enc_height, fps).is_err() {
        return Ok(());
    }
    if let Ok(payload) = encoder.encode(&first_i420, 0) {
        if !payload.is_empty() && sender.write_frame(&payload, 0).is_err() {
            return Ok(());
        }
    }

    debug!(
        preset = ?tuning.preset,
        fps = fps,
        width = enc_width,
        height = enc_height,
        "capture-encode-stream started"
    );

    let mut pts: i64 = 1;
    let mut context_poller = DesktopContextPoller::new();
    let mut capture_fail_streak: u32 = 0;
    let mut force_reinitialize = false;
    // Wrap backend in Option so we can drop it before reinit. For DXGI, only one
    // IDXGIOutputDuplication per output per process — release before DuplicateOutput again.
    let mut backend_opt: Option<Box<dyn CaptureBackend>> = Some(backend);
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        if let Some(new_context) = context_poller.poll_transition() {
            debug!(
                context = ?new_context,
                legacy_cpu_vp8_capture,
                "desktop context changed; forcing capture backend reinitialize"
            );
            force_reinitialize = true;
        }

        if force_reinitialize {
            backend_opt = None;
            if !legacy_cpu_vp8_capture {
                // Give COM/DXGI time to fully release the old IDXGIOutputDuplication.
                std::thread::sleep(Duration::from_millis(200));
            }
            crate::control::attach_thread_to_input_desktop();

            let reopened: Result<Box<dyn CaptureBackend>> = if legacy_cpu_vp8_capture {
                windows::GdiBackend::new()
                    .map(|mut b| {
                        let _ = b.set_capture_source_index(active_capture_output_index);
                        Box::new(b) as Box<dyn CaptureBackend>
                    })
                    .map_err(|e| anyhow::anyhow!("{}", e))
            } else {
                windows::DxgiBackend::new()
                    .map(|b| Box::new(b) as Box<dyn CaptureBackend>)
                    .map_err(|e| anyhow::anyhow!("{}", e))
            };

            match reopened {
                Ok(mut new_backend) => {
                    if let Ok(recovery_frame) = new_backend.capture_frame() {
                        if let Ok(new_encoder) =
                            vp8::Vp8Encoder::new(enc_width, enc_height, fps, tuning)
                        {
                            encoder = new_encoder;
                        }
                        if let Ok(i420) =
                            bgra_to_i420_scaled(&recovery_frame, grayscale, enc_width, enc_height)
                        {
                            if i420.len() == expected_len {
                                if let Ok(payload) = encoder.encode(&i420, pts) {
                                    if !payload.is_empty()
                                        && sender.write_frame(&payload, pts as u64).is_err()
                                    {
                                        break;
                                    }
                                    pts += 1;
                                }
                            }
                        }
                        backend_opt = Some(new_backend);
                        force_reinitialize = false;
                        capture_fail_streak = 0;
                    } else {
                        std::thread::sleep(interval);
                        continue;
                    }
                }
                Err(_) => {
                    std::thread::sleep(interval);
                    continue;
                }
            }
        }

        let backend = match backend_opt.as_mut() {
            Some(b) => b,
            None => {
                force_reinitialize = true;
                std::thread::sleep(interval);
                continue;
            }
        };

        match backend.try_capture_frame() {
            Ok(crate::capture::CaptureResult::Frame(frame)) => {
                capture_fail_streak = 0;
                let i420 = match bgra_to_i420_scaled(&frame, grayscale, enc_width, enc_height) {
                    Ok(buf) => buf,
                    Err(e) => {
                        warn!(error = %e, "bgra_to_i420 failed");
                        std::thread::sleep(interval);
                        continue;
                    }
                };
                if i420.len() != expected_len {
                    warn!(
                        expected = expected_len,
                        actual = i420.len(),
                        "i420 length mismatch; skipping frame"
                    );
                    std::thread::sleep(interval);
                    continue;
                }
                match encoder.encode(&i420, pts) {
                    Ok(payload) => {
                        if !payload.is_empty() && sender.write_frame(&payload, pts as u64).is_err()
                        {
                            break;
                        }
                    }
                    Err(e) => warn!(error = %e, "encode failed"),
                }
                pts += 1;
            }
            Ok(crate::capture::CaptureResult::Timeout) => {
                // No new frame -- desktop is static. Not an error.
            }
            Ok(crate::capture::CaptureResult::AccessLost) => {
                debug!("DXGI AccessLost; scheduling DXGI reinitialize");
                force_reinitialize = true;
            }
            Err(e) => {
                capture_fail_streak = capture_fail_streak.saturating_add(1);
                if capture_fail_streak <= 3 || capture_fail_streak.is_multiple_of(30) {
                    warn!(
                        error = %e,
                        capture_fail_streak = capture_fail_streak,
                        "capture_frame failed"
                    );
                }
                if capture_fail_streak >= 30 {
                    force_reinitialize = true;
                }
            }
        }
        std::thread::sleep(interval);
    }

    debug!("capture-encode-stream finished (receiver dropped or error)");
    Ok(())
}

#[cfg(windows)]
pub fn run_capture_encode_stream_to_pipe(
    pipe_name: &str,
    auth_token: &str,
    tuning: EncodeTuning,
    fps: u32,
    display_stream_mode: DisplayStreamMode,
    stop: Arc<AtomicBool>,
    capture_output_switch_rx: mpsc::Receiver<usize>,
    stream_bitrate_rx: mpsc::Receiver<u32>,
) -> Result<()> {
    use crate::capture::windows;
    use crate::capture::CaptureBackend;
    use winapi::um::handleapi::CloseHandle;

    helper_io_log(
        "pipeline_entry",
        serde_json::json!({ "pipe_name": pipe_name }),
    );
    let handle = open_named_pipe_writer(pipe_name)?;
    helper_io_log(
        "pipe_writer_opened",
        serde_json::json!({ "pipe_name": pipe_name }),
    );

    let sender = PipeStreamSender::new(handle);
    let mode =
        crate::display_processing::effective_display_processing_mode("agent helper capture/encode");
    let display_stream_mode = display_stream_mode_for_processing_mode(mode, display_stream_mode);
    // Authenticate this helper to the agent before sending any chunks.
    const PIPE_AUTH_TAG: u8 = 3;
    ensure!(!auth_token.trim().is_empty(), "missing pipe auth token");
    ensure!(
        auth_token.len() <= HELPER_PIPE_MAX_AUTH_TOKEN_LEN,
        "pipe auth token too long"
    );
    let mut auth_payload = Vec::with_capacity(6 + auth_token.len());
    auth_payload.extend_from_slice(&HELPER_PIPE_HANDSHAKE_MAGIC);
    auth_payload.extend_from_slice(&HELPER_PIPE_PROTOCOL_VERSION.to_be_bytes());
    auth_payload.extend_from_slice(auth_token.as_bytes());
    sender
        .write_chunk(PIPE_AUTH_TAG, &auth_payload)
        .context("write pipe auth handshake")?;
    if display_stream_mode == DisplayStreamMode::ModernCapture {
        helper_io_log(
            "capture_mode_dirty_rects_h264",
            serde_json::json!({ "pipe_name": pipe_name, "mode": display_stream_mode.as_str() }),
        );
        match emit_dirty_rect_stream_h264(
            DirtyRectStreamOutput::Pipe {
                sender: &sender,
                pipe_name,
            },
            tuning,
            fps,
            stop.clone(),
            mode,
            &capture_output_switch_rx,
            &stream_bitrate_rx,
        ) {
            Ok(()) => {
                unsafe {
                    CloseHandle(handle);
                }
                return Ok(());
            }
            Err(err) => {
                helper_io_log(
                    "modern_capture_startup_failed_fallback_legacy",
                    serde_json::json!({
                        "pipe_name": pipe_name,
                        "error": err.to_string(),
                    }),
                );
            }
        }
    }
    if display_stream_mode == DisplayStreamMode::ScreenshotOnly {
        helper_io_log(
            "capture_mode_screenshot_only",
            serde_json::json!({ "pipe_name": pipe_name, "mode": display_stream_mode.as_str() }),
        );
        let result = emit_screenshot_only_bgra(
            DirtyRectStreamOutput::Pipe {
                sender: &sender,
                pipe_name,
            },
            tuning,
            fps,
            stop,
        );
        unsafe {
            CloseHandle(handle);
        }
        return result;
    }
    let interval = if fps > 0 {
        Duration::from_secs_f64(1.0 / fps as f64)
    } else {
        Duration::from_millis(100)
    };
    let mut current_tuning = tuning;
    let grayscale = current_tuning.preset.grayscale_chroma();
    let mut active_capture_output_index: usize = 0;

    // CRITICAL: Attach this thread to the current input desktop BEFORE
    // attempting desktop capture. Explicit legacy mode uses GDI so it can be
    // used to test around DXGI/D3D11 failures.
    let _ = crate::control::attach_thread_to_input_desktop();

    let init_started_at = Instant::now();
    let mut init_attempt: u32 = 0;
    let (backend, enc_width, enc_height, expected_len, mut encoder, first_i420) = loop {
        if stop.load(Ordering::Relaxed) {
            helper_io_log(
                "stop_before_capture_backend_init",
                serde_json::json!({ "pipe_name": pipe_name, "attempt": init_attempt }),
            );
            unsafe {
                CloseHandle(handle);
            }
            return Ok(());
        }
        init_attempt += 1;
        let mut backend: Box<dyn CaptureBackend> = if mode.is_legacy() {
            match windows::GdiBackend::new() {
                Ok(mut backend) => {
                    if let Err(err) = backend.set_capture_source_index(active_capture_output_index)
                    {
                        helper_io_log(
                            "gdi_backend_capture_output_set_failed",
                            serde_json::json!({
                                "pipe_name": pipe_name,
                                "attempt": init_attempt,
                                "index": active_capture_output_index,
                                "error": err.to_string(),
                            }),
                        );
                        std::thread::sleep(interval);
                        continue;
                    }
                    helper_io_log(
                        "legacy_gdi_backend_initialized",
                        serde_json::json!({ "pipe_name": pipe_name, "attempt": init_attempt }),
                    );
                    Box::new(backend)
                }
                Err(e) => {
                    if init_attempt == 1 || init_attempt.is_multiple_of(20) {
                        helper_io_log(
                            "gdi_backend_init_failed",
                            serde_json::json!({
                                "pipe_name": pipe_name,
                                "attempt": init_attempt,
                                "error": e.to_string(),
                            }),
                        );
                    }
                    if init_attempt.is_multiple_of(30) {
                        let _ = crate::control::attach_thread_to_input_desktop();
                    }
                    std::thread::sleep(interval);
                    continue;
                }
            }
        } else {
            match windows::DxgiBackend::new() {
                Ok(backend) => Box::new(backend),
                Err(e) => {
                    if init_attempt == 1 || init_attempt.is_multiple_of(20) {
                        log_dxgi_backend_init_failed(
                            "dxgi_backend_init_failed",
                            Some(pipe_name),
                            init_attempt,
                            &e,
                        );
                    }
                    // Re-attach to desktop periodically in case it changed
                    if init_attempt.is_multiple_of(30) {
                        let _ = crate::control::attach_thread_to_input_desktop();
                    }
                    std::thread::sleep(interval);
                    continue;
                }
            }
        };
        match backend.capture_frame() {
            Ok(first_frame) => {
                let (enc_width, enc_height) =
                    aligned_stream_dimensions(first_frame.width, first_frame.height);
                let i420 = match bgra_to_i420_scaled(&first_frame, grayscale, enc_width, enc_height)
                {
                    Ok(buf) => buf,
                    Err(_e) => {
                        std::thread::sleep(interval);
                        continue;
                    }
                };
                let expected_len = (enc_width as usize) * (enc_height as usize) * 3 / 2;
                if i420.len() != expected_len {
                    std::thread::sleep(interval);
                    continue;
                }
                let encoder = match vp8::Vp8Encoder::new(enc_width, enc_height, fps, current_tuning)
                    .context("vp8 encoder init failed")
                {
                    Ok(enc) => enc,
                    Err(_e) => {
                        std::thread::sleep(interval);
                        continue;
                    }
                };
                break (backend, enc_width, enc_height, expected_len, encoder, i420);
            }
            Err(e) => {
                if init_attempt == 1 || init_attempt.is_multiple_of(20) {
                    helper_io_log(
                        "first_capture_frame_failed",
                        serde_json::json!({
                            "pipe_name": pipe_name,
                            "attempt": init_attempt,
                            "error": e.to_string(),
                        }),
                    );
                }
                if e.to_string().contains("timed out")
                    && init_started_at.elapsed() >= Duration::from_millis(1500)
                {
                    let gdi_frame = match windows::GdiBackend::new().and_then(|mut gdi| {
                        let _ = gdi.set_capture_source_index(active_capture_output_index);
                        gdi.capture_frame()
                    }) {
                        Ok(frame) => {
                            helper_io_log(
                                "first_frame_gdi_timeout_fallback",
                                serde_json::json!({
                                    "pipe_name": pipe_name,
                                    "attempt": init_attempt,
                                    "elapsed_ms": init_started_at.elapsed().as_millis() as u64,
                                    "frame_width": frame.width,
                                    "frame_height": frame.height,
                                }),
                            );
                            Some(frame)
                        }
                        Err(gdi_err) => {
                            helper_io_log(
                                "first_frame_gdi_timeout_fallback_failed",
                                serde_json::json!({
                                    "pipe_name": pipe_name,
                                    "attempt": init_attempt,
                                    "elapsed_ms": init_started_at.elapsed().as_millis() as u64,
                                    "error": gdi_err.to_string(),
                                }),
                            );
                            None
                        }
                    };
                    let (src_width, src_height) = gdi_frame
                        .as_ref()
                        .map(|frame| (frame.width, frame.height))
                        .or_else(primary_display_dimensions)
                        .unwrap_or((1024, 768));
                    let (enc_width, enc_height) = aligned_stream_dimensions(src_width, src_height);
                    let expected_len = (enc_width as usize) * (enc_height as usize) * 3 / 2;
                    let i420 = if let Some(frame) = gdi_frame.as_ref() {
                        match bgra_to_i420_scaled(frame, grayscale, enc_width, enc_height) {
                            Ok(buf) => buf,
                            Err(convert_err) => {
                                helper_io_log(
                                    "first_frame_gdi_timeout_fallback_convert_failed",
                                    serde_json::json!({
                                        "pipe_name": pipe_name,
                                        "attempt": init_attempt,
                                        "error": convert_err.to_string(),
                                    }),
                                );
                                build_bootstrap_i420_frame(enc_width, enc_height)
                            }
                        }
                    } else {
                        build_bootstrap_i420_frame(enc_width, enc_height)
                    };
                    if i420.len() == expected_len {
                        let encoder =
                            match vp8::Vp8Encoder::new(enc_width, enc_height, fps, current_tuning)
                                .context("vp8 encoder init failed")
                            {
                                Ok(enc) => enc,
                                Err(encoder_err) => {
                                    helper_io_log(
                                        "first_frame_bootstrap_encoder_init_failed",
                                        serde_json::json!({
                                            "pipe_name": pipe_name,
                                            "attempt": init_attempt,
                                            "enc_width": enc_width,
                                            "enc_height": enc_height,
                                            "error": encoder_err.to_string(),
                                        }),
                                    );
                                    std::thread::sleep(interval);
                                    continue;
                                }
                            };
                        helper_io_log(
                            "first_frame_bootstrap_fallback",
                            serde_json::json!({
                                "pipe_name": pipe_name,
                                "attempt": init_attempt,
                                "elapsed_ms": init_started_at.elapsed().as_millis() as u64,
                                "src_width": src_width,
                                "src_height": src_height,
                                "enc_width": enc_width,
                                "enc_height": enc_height,
                            }),
                        );
                        break (backend, enc_width, enc_height, expected_len, encoder, i420);
                    }
                }
                std::thread::sleep(interval);
            }
        }
    };

    let agent_monitor_hz = cfg_primary_monitor_hz();
    let mut metadata = serde_json::json!({
        "bitrate_kbps": current_tuning.bitrate_kbps(),
        "preset": current_tuning.preset.as_str(),
        "cpu_used": current_tuning.cpu_used.unwrap_or(2),
        "encoding_fps": fps,
        "agent_monitor_hz": agent_monitor_hz,
        "activeIndex": active_capture_output_index as u32,
        "captureType": "legacy",
    });
    if let Some(obj) = metadata.as_object_mut() {
        merge_capture_outputs_metadata(obj);
    }
    if let Ok(json_bytes) = serde_json::to_vec(&metadata) {
        let mut msg = Vec::with_capacity(8 + json_bytes.len());
        msg.extend_from_slice(b"RMMD");
        msg.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        msg.extend_from_slice(&json_bytes);
        let _ = sender.write_metadata(&msg);
        helper_io_log(
            "metadata_written",
            serde_json::json!({ "pipe_name": pipe_name, "metadata_len": msg.len() }),
        );
    }

    if sender.write_header(enc_width, enc_height, fps).is_err() {
        helper_io_log(
            "header_write_failed",
            serde_json::json!({ "pipe_name": pipe_name, "enc_width": enc_width, "enc_height": enc_height }),
        );
        unsafe {
            CloseHandle(handle);
        }
        return Ok(());
    }
    helper_io_log(
        "header_written",
        serde_json::json!({ "pipe_name": pipe_name, "enc_width": enc_width, "enc_height": enc_height }),
    );
    if let Ok(payload) = encoder.encode(&first_i420, 0) {
        if !payload.is_empty() {
            if sender.write_frame(&payload, 0).is_err() {
                helper_io_log(
                    "first_frame_write_failed",
                    serde_json::json!({ "pipe_name": pipe_name, "payload_len": payload.len() }),
                );
                unsafe {
                    CloseHandle(handle);
                }
                return Ok(());
            }
            helper_io_log(
                "first_frame_written",
                serde_json::json!({ "pipe_name": pipe_name, "payload_len": payload.len() }),
            );
        }
    }

    debug!(
        preset = ?current_tuning.preset,
        fps = fps,
        width = enc_width,
        height = enc_height,
        "capture-encode-stream started (pipe)"
    );

    let mut pts: i64 = 1;
    let mut context_poller = DesktopContextPoller::new();
    let mut capture_fail_streak: u32 = 0;
    let mut force_reinitialize = false;
    // Wrap backend in Option so we can drop it before backend reinit.
    // DXGI only allows ONE active IDXGIOutputDuplication per output per process;
    // the old handle MUST be released before DuplicateOutput can succeed again.
    let mut backend_opt: Option<Box<dyn CaptureBackend>> = Some(backend);
    loop {
        if stop.load(Ordering::Relaxed) {
            helper_io_log(
                "stop_flag_observed",
                serde_json::json!({ "pipe_name": pipe_name, "pts": pts }),
            );
            break;
        }

        if let Some(kbps) = pending_stream_bitrate_from_rx(&stream_bitrate_rx) {
            let mut requested_tuning = current_tuning;
            if apply_stream_bitrate_update(&mut requested_tuning, kbps) {
                match vp8::Vp8Encoder::new(enc_width, enc_height, fps, requested_tuning) {
                    Ok(new_encoder) => {
                        current_tuning = requested_tuning;
                        encoder = new_encoder;
                        let mut bitrate_metadata = serde_json::json!({
                            "bitrate_kbps": current_tuning.bitrate_kbps(),
                            "preset": current_tuning.preset.as_str(),
                            "cpu_used": current_tuning.cpu_used.unwrap_or(2),
                            "encoding_fps": fps,
                            "agent_monitor_hz": agent_monitor_hz,
                            "activeIndex": active_capture_output_index as u32,
                            "captureType": "legacy",
                        });
                        if let Some(obj) = bitrate_metadata.as_object_mut() {
                            merge_capture_outputs_metadata(obj);
                        }
                        if let Ok(json_bytes) = serde_json::to_vec(&bitrate_metadata) {
                            let mut msg = Vec::with_capacity(8 + json_bytes.len());
                            msg.extend_from_slice(b"RMMD");
                            msg.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
                            msg.extend_from_slice(&json_bytes);
                            let _ = sender.write_metadata(&msg);
                        }
                        helper_io_log(
                            "stream_bitrate_updated",
                            serde_json::json!({
                                "pipe_name": pipe_name,
                                "kbps": kbps,
                            }),
                        );
                    }
                    Err(err) => {
                        helper_io_log(
                            "stream_bitrate_update_failed",
                            serde_json::json!({
                                "pipe_name": pipe_name,
                                "kbps": kbps,
                                "error": err.to_string(),
                            }),
                        );
                    }
                }
            }
        }

        if let Some(idx) = pending_capture_index_from_rx(&capture_output_switch_rx) {
            if let Some(b) = backend_opt.as_mut() {
                match b.set_capture_source_index(idx) {
                    Ok(()) => {
                        active_capture_output_index = idx;
                        capture_fail_streak = 0;
                        if let Some(msg) = build_stream_metadata(
                            current_tuning,
                            fps,
                            agent_monitor_hz,
                            active_capture_output_index as u32,
                            None,
                            "legacy",
                        ) {
                            let _ = sender.write_metadata(&msg);
                        }
                    }
                    Err(err) => {
                        helper_io_log(
                            "capture_output_switch_failed",
                            serde_json::json!({
                                "pipe_name": pipe_name,
                                "index": idx,
                                "error": err.to_string(),
                            }),
                        );
                    }
                }
                crate::control::set_remote_input_capture_output_index(active_capture_output_index);
            }
        }

        if let Some(new_context) = context_poller.poll_transition() {
            debug!(context = ?new_context, "desktop context changed; forcing capture backend reinitialize");
            force_reinitialize = true;
        }

        let start = Instant::now();
        if force_reinitialize {
            // 1. Drop old backend FIRST to release any desktop capture handle.
            helper_io_log(
                if mode.is_legacy() {
                    "gdi_reinit_start"
                } else {
                    "dxgi_reinit_start"
                },
                serde_json::json!({ "pipe_name": pipe_name }),
            );
            backend_opt = None;

            // 2. Give COM/DXGI time to fully release the old IDXGIOutputDuplication.
            //    Without this pause, DuplicateOutput often fails because the OS
            //    still holds the previous duplication internally.
            std::thread::sleep(Duration::from_millis(200));

            // 3. Switch calling thread to the current input desktop so DXGI
            //    can duplicate the correct desktop after a transition.
            let attached = crate::control::attach_thread_to_input_desktop();
            let _ = attached;

            let backend_result: Result<Box<dyn CaptureBackend>> = if mode.is_legacy() {
                windows::GdiBackend::new()
                    .map(|backend| Box::new(backend) as Box<dyn CaptureBackend>)
            } else {
                windows::DxgiBackend::new()
                    .map(|backend| Box::new(backend) as Box<dyn CaptureBackend>)
                    .map_err(|err| anyhow::anyhow!("{}", err))
            };
            match backend_result {
                Ok(mut new_backend) => {
                    if let Err(err) =
                        new_backend.set_capture_source_index(active_capture_output_index)
                    {
                        helper_io_log(
                            "capture_output_restore_failed",
                            serde_json::json!({
                                "pipe_name": pipe_name,
                                "index": active_capture_output_index,
                                "error": err.to_string(),
                            }),
                        );
                    }
                    crate::control::set_remote_input_capture_output_index(
                        active_capture_output_index,
                    );
                    if let Some(msg) = build_stream_metadata(
                        current_tuning,
                        fps,
                        agent_monitor_hz,
                        active_capture_output_index as u32,
                        None,
                        "legacy",
                    ) {
                        let _ = sender.write_metadata(&msg);
                    }
                    let frame_result = new_backend.capture_frame();
                    if let Ok(recovery_frame) = frame_result {
                        if let Ok(new_encoder) =
                            vp8::Vp8Encoder::new(enc_width, enc_height, fps, current_tuning)
                        {
                            encoder = new_encoder;
                        }
                        if let Ok(i420) =
                            bgra_to_i420_scaled(&recovery_frame, grayscale, enc_width, enc_height)
                        {
                            if i420.len() == expected_len {
                                if let Ok(payload) = encoder.encode(&i420, pts) {
                                    if !payload.is_empty()
                                        && sender.write_frame(&payload, pts as u64).is_err()
                                    {
                                        break;
                                    }
                                    pts += 1;
                                }
                            }
                        }
                        backend_opt = Some(new_backend);
                        force_reinitialize = false;
                        capture_fail_streak = 0;
                    } else {
                        // New backend created but first capture failed; drop it and retry.
                        std::thread::sleep(interval);
                        continue;
                    }
                }
                Err(_) => {
                    std::thread::sleep(interval);
                    continue;
                }
            }
        }

        // Get a reference to the backend, or force reinit next iteration.
        let backend = match backend_opt.as_mut() {
            Some(b) => b,
            None => {
                force_reinitialize = true;
                std::thread::sleep(interval);
                continue;
            }
        };

        match backend.try_capture_frame() {
            Ok(crate::capture::CaptureResult::Frame(frame)) => {
                capture_fail_streak = 0;
                let i420 = match bgra_to_i420_scaled(&frame, grayscale, enc_width, enc_height) {
                    Ok(buf) => buf,
                    Err(e) => {
                        warn!(error = %e, "bgra_to_i420 failed");
                        std::thread::sleep(interval);
                        continue;
                    }
                };
                if i420.len() != expected_len {
                    warn!(
                        expected = expected_len,
                        actual = i420.len(),
                        "i420 length mismatch; skipping frame"
                    );
                    std::thread::sleep(interval);
                    continue;
                }
                match encoder.encode(&i420, pts) {
                    Ok(payload) => {
                        if !payload.is_empty() && sender.write_frame(&payload, pts as u64).is_err()
                        {
                            break;
                        }
                    }
                    Err(e) => warn!(error = %e, "encode failed"),
                }
                pts += 1;
            }
            Ok(crate::capture::CaptureResult::Timeout) => {
                // No new frame available -- desktop is static. This is normal.
                // Do NOT count as a failure or reinitialize.
            }
            Ok(crate::capture::CaptureResult::AccessLost) => {
                // Desktop switch / mode change -- must recreate DXGI backend.
                debug!("DXGI AccessLost (pipe); scheduling DXGI reinitialize");
                force_reinitialize = true;
            }
            Err(e) => {
                capture_fail_streak = capture_fail_streak.saturating_add(1);
                if capture_fail_streak <= 3 || capture_fail_streak.is_multiple_of(30) {
                    warn!(
                        error = %e,
                        capture_fail_streak = capture_fail_streak,
                        "capture_frame failed"
                    );
                }
                if capture_fail_streak >= 30 {
                    force_reinitialize = true;
                }
            }
        }
        let elapsed = start.elapsed();
        if elapsed < interval {
            std::thread::sleep(interval - elapsed);
        }
    }

    helper_io_log(
        "pipeline_exit",
        serde_json::json!({ "pipe_name": pipe_name, "pts": pts }),
    );
    unsafe {
        CloseHandle(handle);
    }
    Ok(())
}

/// Runs capture → encode → IVF chunks into the channel until the receiver is dropped.
#[cfg(windows)]
pub fn run_capture_encode_stream(
    tx: mpsc::Sender<IvfChunk>,
    tuning: EncodeTuning,
    fps: u32,
) -> Result<()> {
    run_capture_encode_stream_with_stop(tx, tuning, fps, Arc::new(AtomicBool::new(false)))
}

#[cfg(not(windows))]
pub fn run_capture_encode_stream(
    _tx: mpsc::Sender<IvfChunk>,
    _tuning: EncodeTuning,
    _fps: u32,
) -> Result<()> {
    anyhow::bail!("capture-encode-stream is only supported on Windows");
}

/// Runs capture → BGRA→I420 → VP8 encode → IVF file.
pub fn run_capture_encode_dump(
    output_path: &Path,
    preset: Preset,
    fps: u32,
    duration_secs: Option<u64>,
    max_frames: Option<u64>,
) -> Result<()> {
    run_capture_encode_dump_impl(output_path, preset, fps, duration_secs, max_frames)
}

#[cfg(windows)]
fn run_capture_encode_dump_impl(
    output_path: &Path,
    preset: Preset,
    fps: u32,
    duration_secs: Option<u64>,
    max_frames: Option<u64>,
) -> Result<()> {
    use std::time::Instant;

    use crate::capture::windows;
    use crate::capture::CaptureBackend;

    let mut tuning = load_encode_tuning_from_env();
    tuning.preset = preset;

    let mut backend =
        windows::DxgiBackend::new().map_err(|e| anyhow::anyhow!("DXGI init: {}", e))?;
    let interval = if fps > 0 {
        Duration::from_secs_f64(1.0 / fps as f64)
    } else {
        Duration::from_millis(100)
    };
    let deadline = duration_secs.map(|s| Instant::now() + Duration::from_secs(s));
    let mut frame_count: u64 = 0;
    let grayscale = tuning.preset.grayscale_chroma();

    const VP8_ALIGN: u32 = 16;
    let first_frame = backend.capture_frame()?;
    let width = (first_frame.width & !(VP8_ALIGN - 1)).max(VP8_ALIGN);
    let height = (first_frame.height & !(VP8_ALIGN - 1)).max(VP8_ALIGN);
    let i420 = bgra_to_i420(&first_frame, grayscale, Some((width, height)))?;
    let expected_len = (width as usize) * (height as usize) * 3 / 2;
    ensure!(
        i420.len() == expected_len,
        "i420 length {} does not match expected {} for {}x{}",
        i420.len(),
        expected_len,
        width,
        height
    );
    let mut encoder =
        vp8::Vp8Encoder::new(width, height, fps, tuning).context("vp8 encoder init failed")?;
    let mut ivf = IvfWriter::new(output_path, width, height, fps)?;

    let payload = encoder
        .encode(&i420, 0)
        .context("vp8 encode first frame failed")?;
    if !payload.is_empty() {
        ivf.write_frame(&payload, 0)?;
        frame_count += 1;
    }

    debug!(
        output = %output_path.display(),
        preset = ?tuning.preset,
        fps = fps,
        "capture-encode-dump started (Ctrl+C to stop)"
    );

    let mut pts: i64 = 1;
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
                let i420 = match bgra_to_i420(&frame, grayscale, Some((width, height))) {
                    Ok(buf) => buf,
                    Err(e) => {
                        warn!(error = %e, "bgra_to_i420 failed");
                        std::thread::sleep(interval);
                        continue;
                    }
                };
                if i420.len() != expected_len {
                    warn!(
                        expected = expected_len,
                        actual = i420.len(),
                        "i420 length mismatch; skipping frame"
                    );
                    std::thread::sleep(interval);
                    continue;
                }
                match encoder.encode(&i420, pts) {
                    Ok(payload) => {
                        if !payload.is_empty() {
                            if let Err(e) = ivf.write_frame(&payload, pts as u64) {
                                warn!(error = %e, "IVF write failed");
                            } else {
                                frame_count += 1;
                            }
                        }
                    }
                    Err(e) => warn!(error = %e, "encode failed"),
                }
                pts += 1;
            }
            Err(e) => warn!(error = %e, "capture_frame failed"),
        }

        std::thread::sleep(interval);
    }

    ivf.finish()?;
    debug!(frame_count, "capture-encode-dump finished");
    Ok(())
}

#[cfg(not(windows))]
fn run_capture_encode_dump_impl(
    _output_path: &Path,
    _preset: Preset,
    _fps: u32,
    _duration_secs: Option<u64>,
    _max_frames: Option<u64>,
) -> Result<()> {
    anyhow::bail!("capture-encode-dump is only supported on Windows");
}

#[cfg(all(test, windows))]
mod tests {
    use super::{
        apply_dirty_rect_to_frame_shadow, apply_move_rect_to_frame_shadow, clip_move_rect,
        display_processing_mode_for_profile, display_profiles_for_processing_mode,
        display_stream_mode_for_processing_mode, display_stream_mode_for_profile,
        pack_dirty_rects_into_atlas, parse_display_stream_mode,
        synthesize_cpu_frame_metadata_from_diff, ClippedMoveRect, DisplayStreamMode,
    };
    use crate::capture::{DirtyRect, Frame, MoveRect, PixelFormat};
    use crate::display_processing::DisplayProcessingMode;

    fn solid_frame(width: u32, height: u32, pixel: [u8; 4]) -> Frame {
        let stride = width * 4;
        let mut data = vec![0; stride as usize * height as usize];
        for chunk in data.chunks_exact_mut(4) {
            chunk.copy_from_slice(&pixel);
        }
        Frame {
            width,
            height,
            stride,
            format: PixelFormat::Bgra8,
            data,
        }
    }

    fn write_pixel(frame: &mut Frame, x: u32, y: u32, pixel: [u8; 4]) {
        let offset = y as usize * frame.stride as usize + x as usize * 4;
        frame.data[offset..offset + 4].copy_from_slice(&pixel);
    }

    fn fill_rect_with_pattern(frame: &mut Frame, rect: DirtyRect, seed: u8) {
        for y in rect.top..rect.bottom {
            for x in rect.left..rect.right {
                let local_x = (x - rect.left) as u8;
                let local_y = (y - rect.top) as u8;
                let pixel = [
                    seed.wrapping_add(local_x.wrapping_mul(17)),
                    seed.wrapping_add(local_y.wrapping_mul(29)),
                    seed ^ local_x.wrapping_mul(7) ^ local_y.wrapping_mul(13),
                    0xff,
                ];
                write_pixel(frame, x, y, pixel);
            }
        }
    }

    #[test]
    fn clip_move_rect_accepts_valid_rect() {
        let rect = MoveRect {
            source_x: 10,
            source_y: 20,
            left: 30,
            top: 40,
            right: 70,
            bottom: 90,
        };
        assert_eq!(
            clip_move_rect(rect, 1920, 1080),
            Some(ClippedMoveRect {
                src_x: 10,
                src_y: 20,
                dst_x: 30,
                dst_y: 40,
                width: 40,
                height: 50,
            })
        );
    }

    #[test]
    fn clip_move_rect_rejects_out_of_bounds_source() {
        let rect = MoveRect {
            source_x: 100,
            source_y: 100,
            left: 10,
            top: 10,
            right: 40,
            bottom: 40,
        };
        assert_eq!(clip_move_rect(rect, 120, 120), None);
    }

    #[test]
    fn apply_move_rect_to_frame_shadow_copies_overlapping_region() {
        let mut frame = Frame {
            width: 4,
            height: 3,
            stride: 16,
            format: PixelFormat::Bgra8,
            data: (0u8..48u8).collect(),
        };
        let rect = ClippedMoveRect {
            src_x: 0,
            src_y: 0,
            dst_x: 1,
            dst_y: 1,
            width: 2,
            height: 2,
        };
        apply_move_rect_to_frame_shadow(&mut frame, rect).expect("apply move rect");

        let bytes_per_pixel = 4usize;
        let stride = frame.stride as usize;
        let dst_first = rect.dst_y as usize * stride + rect.dst_x as usize * bytes_per_pixel;
        let dst_second = (rect.dst_y as usize + 1) * stride + rect.dst_x as usize * bytes_per_pixel;
        assert_eq!(
            &frame.data[dst_first..dst_first + 8],
            &[0, 1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(
            &frame.data[dst_second..dst_second + 8],
            &[16, 17, 18, 19, 20, 21, 22, 23]
        );
    }

    #[test]
    fn apply_dirty_rect_to_frame_shadow_writes_rect_payload() {
        let mut frame = Frame {
            width: 4,
            height: 3,
            stride: 16,
            format: PixelFormat::Bgra8,
            data: vec![0; 48],
        };
        let rect = DirtyRect {
            left: 1,
            top: 1,
            right: 3,
            bottom: 2,
        };
        let payload = vec![10, 11, 12, 13, 20, 21, 22, 23];
        apply_dirty_rect_to_frame_shadow(&mut frame, rect, &payload).expect("apply dirty rect");

        let row_start = frame.stride as usize + rect.left as usize * 4;
        assert_eq!(
            &frame.data[row_start..row_start + payload.len()],
            payload.as_slice()
        );
    }

    #[test]
    fn parse_display_stream_mode_defaults_to_legacy_capture() {
        assert_eq!(
            parse_display_stream_mode(None),
            DisplayStreamMode::LegacyCapture
        );
        assert_eq!(
            parse_display_stream_mode(Some("legacy_capture")),
            DisplayStreamMode::LegacyCapture
        );
        assert_eq!(
            parse_display_stream_mode(Some("video_vp8")),
            DisplayStreamMode::LegacyCapture
        );
    }

    #[test]
    fn parse_display_stream_mode_accepts_modern_capture() {
        assert_eq!(
            parse_display_stream_mode(Some("modern_capture")),
            DisplayStreamMode::ModernCapture
        );
    }

    #[test]
    fn explicit_display_processing_modes_override_viewer_stream_preference() {
        assert_eq!(
            display_stream_mode_for_processing_mode(
                DisplayProcessingMode::Legacy,
                DisplayStreamMode::ModernCapture,
            ),
            DisplayStreamMode::LegacyCapture
        );
        assert_eq!(
            display_stream_mode_for_processing_mode(
                DisplayProcessingMode::Gpu,
                DisplayStreamMode::LegacyCapture,
            ),
            DisplayStreamMode::ModernCapture
        );
        assert_eq!(
            display_stream_mode_for_processing_mode(
                DisplayProcessingMode::Auto,
                DisplayStreamMode::LegacyCapture,
            ),
            DisplayStreamMode::LegacyCapture
        );
    }

    #[test]
    fn advertised_profiles_preserve_screenshot_fallback_in_every_processing_mode() {
        let ids = |mode| {
            display_profiles_for_processing_mode(mode)
                .into_iter()
                .map(|profile| profile.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(DisplayProcessingMode::Legacy),
            vec!["legacy", "screenshot_only"]
        );
        assert_eq!(
            ids(DisplayProcessingMode::Gpu),
            vec!["modern_gpu", "screenshot_only"]
        );
        assert_eq!(
            ids(DisplayProcessingMode::Auto),
            vec!["modern_gpu", "legacy", "screenshot_only"]
        );
    }

    #[test]
    fn pack_dirty_rects_into_atlas_preserves_frame_coordinates() {
        let rects = vec![
            DirtyRect {
                left: 40,
                top: 20,
                right: 72,
                bottom: 44,
            },
            DirtyRect {
                left: 8,
                top: 60,
                right: 24,
                bottom: 92,
            },
        ];
        let packed = pack_dirty_rects_into_atlas(128, 128, &rects).expect("packed rects");
        assert_eq!(packed.len(), 2);
        for (packed_rect, source_rect) in packed.iter().zip(rects.iter()) {
            assert_eq!(packed_rect.source, *source_rect);
            assert_eq!(packed_rect.atlas.dst_x, source_rect.left);
            assert_eq!(packed_rect.atlas.dst_y, source_rect.top);
            assert_eq!(packed_rect.atlas.atlas_x, source_rect.left);
            assert_eq!(packed_rect.atlas.atlas_y, source_rect.top);
            assert_eq!(
                packed_rect.atlas.width,
                source_rect.right - source_rect.left
            );
            assert_eq!(
                packed_rect.atlas.height,
                source_rect.bottom - source_rect.top
            );
        }
    }

    #[test]
    fn synthesize_cpu_frame_metadata_detects_tile_move_and_residual_dirty_rect() {
        let background = [4, 8, 12, 0xff];
        let moved_rect = DirtyRect {
            left: 0,
            top: 0,
            right: 32,
            bottom: 32,
        };
        let destination_rect = DirtyRect {
            left: 32,
            top: 0,
            right: 64,
            bottom: 32,
        };

        let mut previous = solid_frame(96, 64, background);
        fill_rect_with_pattern(&mut previous, moved_rect, 19);

        let mut current = solid_frame(96, 64, background);
        fill_rect_with_pattern(&mut current, destination_rect, 19);

        let metadata =
            synthesize_cpu_frame_metadata_from_diff(Some(&previous), &current).expect("metadata");

        assert_eq!(
            metadata.move_rects,
            vec![MoveRect {
                source_x: moved_rect.left,
                source_y: moved_rect.top,
                left: destination_rect.left,
                top: destination_rect.top,
                right: destination_rect.right,
                bottom: destination_rect.bottom,
            }]
        );
        assert!(
            metadata.dirty_rects.contains(&moved_rect),
            "expected source tile to remain dirty after move synthesis: {:?}",
            metadata.dirty_rects
        );
    }

    #[test]
    fn synthesize_cpu_frame_metadata_retains_dirty_rects_for_move_plus_edit() {
        let background = [3, 6, 9, 0xff];
        let source_left = DirtyRect {
            left: 0,
            top: 0,
            right: 32,
            bottom: 32,
        };
        let destination_left = DirtyRect {
            left: 32,
            top: 0,
            right: 64,
            bottom: 32,
        };

        let mut previous = solid_frame(128, 64, background);
        fill_rect_with_pattern(
            &mut previous,
            DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 32,
            },
            41,
        );

        let mut current = solid_frame(128, 64, background);
        fill_rect_with_pattern(
            &mut current,
            DirtyRect {
                left: 32,
                top: 0,
                right: 96,
                bottom: 32,
            },
            41,
        );
        write_pixel(&mut current, 70, 10, [0xde, 0xad, 0xbe, 0xff]);

        let metadata =
            synthesize_cpu_frame_metadata_from_diff(Some(&previous), &current).expect("metadata");

        assert!(
            metadata.move_rects.contains(&MoveRect {
                source_x: source_left.left,
                source_y: source_left.top,
                left: destination_left.left,
                top: destination_left.top,
                right: destination_left.right,
                bottom: destination_left.bottom,
            }),
            "expected at least one translated move rect: {:?}",
            metadata.move_rects
        );
        assert!(
            metadata.dirty_rects.contains(&source_left),
            "expected vacated source coverage after move synthesis: {:?}",
            metadata.dirty_rects
        );
        assert!(
            metadata.dirty_rects.iter().any(|rect| {
                rect.left <= 64 && rect.top == 0 && rect.right >= 96 && rect.bottom >= 32
            }),
            "expected residual dirty coverage for the edited destination tile: {:?}",
            metadata.dirty_rects
        );
    }

    #[test]
    fn synthesize_cpu_frame_metadata_falls_back_on_ambiguous_repeated_content() {
        let background = [2, 4, 6, 0xff];
        let repeated_rect_a = DirtyRect {
            left: 0,
            top: 0,
            right: 32,
            bottom: 32,
        };
        let repeated_rect_b = DirtyRect {
            left: 32,
            top: 0,
            right: 64,
            bottom: 32,
        };
        let destination_rect = DirtyRect {
            left: 64,
            top: 0,
            right: 96,
            bottom: 32,
        };

        let mut previous = solid_frame(128, 64, background);
        fill_rect_with_pattern(&mut previous, repeated_rect_a, 73);
        fill_rect_with_pattern(&mut previous, repeated_rect_b, 73);

        let mut current = solid_frame(128, 64, background);
        fill_rect_with_pattern(&mut current, repeated_rect_b, 73);
        fill_rect_with_pattern(&mut current, destination_rect, 73);

        let metadata =
            synthesize_cpu_frame_metadata_from_diff(Some(&previous), &current).expect("metadata");

        assert!(
            metadata.move_rects.is_empty(),
            "ambiguous repeated content should fall back to dirty rects: {:?}",
            metadata.move_rects
        );
        assert!(
            !metadata.dirty_rects.is_empty(),
            "dirty rect fallback should still describe the frame changes"
        );
    }

    #[test]
    fn selected_profile_maps_to_helper_stream_and_processing_modes() {
        assert_eq!(
            display_stream_mode_for_profile("legacy"),
            DisplayStreamMode::LegacyCapture
        );
        assert_eq!(
            display_stream_mode_for_profile("modern_cpu"),
            DisplayStreamMode::LegacyCapture
        );
        assert_eq!(
            display_stream_mode_for_profile("modern_gpu"),
            DisplayStreamMode::ModernCapture
        );
        assert_eq!(
            display_stream_mode_for_profile("experimental"),
            DisplayStreamMode::ModernCapture
        );
        assert_eq!(
            display_stream_mode_for_profile("screenshot_only"),
            DisplayStreamMode::ScreenshotOnly
        );
        assert_eq!(display_processing_mode_for_profile("legacy"), "legacy");
        assert_eq!(display_processing_mode_for_profile("modern_cpu"), "legacy");
        assert_eq!(
            display_processing_mode_for_profile("modern_gpu"),
            "modern_gpu"
        );
        assert_eq!(
            display_processing_mode_for_profile("experimental"),
            "modern_gpu"
        );
        assert_eq!(
            display_processing_mode_for_profile("screenshot_only"),
            "legacy"
        );
    }
}
