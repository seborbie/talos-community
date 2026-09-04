use std::{
    ffi::{c_char, c_void, CStr},
    io::{Read, Write},
    os::unix::net::UnixStream,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, ensure, Context, Result};
#[cfg(not(test))]
use apple_metal::{
    command_buffer_status, resource_options, CommandQueue, ComputePipelineState, MetalBuffer,
    MetalDevice,
};
use block2::{Block, RcBlock};
use objc2::{
    msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject},
};
use screencapturekit::{
    cm::{CMSampleBuffer, CMSampleBufferExt, CMTime},
    prelude::*,
};
use serde::Serialize;
use serde_json::json;
use talos_protocol::{
    build_display_atlas_h264, build_display_experimental_atlas_commands,
    build_display_experimental_atlas_commands_chunk, build_display_frame_begin,
    build_display_frame_end, build_display_keyframe, DisplayAtlasRect, DisplayStreamDescriptor,
    CONTROL_MOD_ALT, CONTROL_MOD_CTRL, CONTROL_MOD_SHIFT, CONTROL_MOD_WIN,
    CONTROL_PAYLOAD_CAPTURE_OUTPUT_INDEX_LEN, CONTROL_PAYLOAD_KEY_LEN,
    CONTROL_PAYLOAD_MOUSE_BUTTON_LEN, CONTROL_PAYLOAD_MOUSE_DOUBLE_CLICK_LEN,
    CONTROL_PAYLOAD_MOUSE_MOVE_LEN, CONTROL_PAYLOAD_MOUSE_WHEEL_LEN,
    CONTROL_PAYLOAD_STREAM_BITRATE_LEN, CONTROL_TYPE_CAPTURE_OUTPUT_SWITCH, CONTROL_TYPE_CLIPBOARD,
    CONTROL_TYPE_KEY_DOWN, CONTROL_TYPE_KEY_UP, CONTROL_TYPE_MOUSE_BUTTON,
    CONTROL_TYPE_MOUSE_DOUBLE_CLICK, CONTROL_TYPE_MOUSE_MOVE, CONTROL_TYPE_MOUSE_WHEEL,
    CONTROL_TYPE_SECURE_ATTENTION, CONTROL_TYPE_SESSION_LOGOFF, CONTROL_TYPE_SESSION_SWITCH,
    CONTROL_TYPE_STOP_CAPTURE, CONTROL_TYPE_STREAM_BITRATE, CONTROL_TYPE_TYPED_INPUT,
    DISPLAY_ATLAS_H264_FLAG_KEYFRAME, DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL,
    DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE, DISPLAY_STREAM_META_TYPE,
    HELPER_PIPE_HANDSHAKE_MAGIC, HELPER_PIPE_PROTOCOL_VERSION,
};
use tracing::{debug, info, warn};

use crate::macos_h264::MacosH264Encoder;

const FRAME_QUEUE_BOUND: usize = 2;
const SCK_CAPTURE_QUEUE_DEPTH: u32 = FRAME_QUEUE_BOUND as u32;
const ATX2_STREAM_MAGIC: u32 = 0x3258_5441;
const ATX2_STREAM_VERSION: u32 = 4;
const ATX2_HEADER_BYTES: usize = 32;
const ATX2_COMMAND_HEADER_BYTES: usize = 24;
const ATX2_TILE_SIZE: u32 = 32;
const ATX2_COMMAND_RAW_BGRA: u32 = 1;
const ATX2_COMMAND_SOLID_COLOR: u32 = 2;
const MACOS_ATX2_DIRTY_TILE_SIZE: u32 = 32;
const MIN_CAPTURE_FPS: u32 = 1;
const MAX_CAPTURE_FPS: u32 = 120;
const WINDOWS_WHEEL_DELTA: i32 = 120;
const CG_SCROLL_EVENT_UNIT_LINE: u32 = 1;
const CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
const CG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const CG_EVENT_RIGHT_MOUSE_UP: u32 = 4;
const CG_EVENT_MOUSE_MOVED: u32 = 5;
const CG_EVENT_LEFT_MOUSE_DRAGGED: u32 = 6;
const CG_EVENT_RIGHT_MOUSE_DRAGGED: u32 = 7;
const CG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
const CG_EVENT_OTHER_MOUSE_UP: u32 = 26;
const CG_EVENT_OTHER_MOUSE_DRAGGED: u32 = 27;
const CG_MOUSE_EVENT_CLICK_STATE: u32 = 1;
const CG_MOUSE_BUTTON_LEFT: u8 = 0;
const CG_MOUSE_BUTTON_RIGHT: u8 = 1;
const CG_MOUSE_BUTTON_OTHER: u8 = 2;
const SCREENSHOT_POST_FRAME_EXIT_GRACE: Duration = Duration::from_secs(2);
const SHAREABLE_CONTENT_TIMEOUT: Duration = Duration::from_secs(5);
const STREAM_START_TIMEOUT: Duration = Duration::from_secs(10);
const STREAM_STOP_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(not(test))]
const ATX2_TARGET_STREAM_BYTES: usize = 8 * 1024 * 1024;
#[cfg(test)]
const ATX2_TARGET_STREAM_BYTES: usize = 224;
#[cfg(not(test))]
const ATX2_METAL_CLASSIFIER_SHADER: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Atx2TileDescriptor {
    uint x;
    uint y;
    uint width;
    uint height;
};

struct Atx2TileClass {
    uint solid;
    uint bgra;
};

struct Atx2ClassifyParams {
    uint stride;
    uint tile_count;
};

kernel void classify_atx2_tiles(
    device const uchar *pixels [[buffer(0)]],
    device const Atx2TileDescriptor *tiles [[buffer(1)]],
    device Atx2TileClass *classes [[buffer(2)]],
    constant Atx2ClassifyParams &params [[buffer(3)]],
    uint gid [[thread_position_in_grid]]
) {
    if (gid >= params.tile_count) {
        return;
    }

    Atx2TileDescriptor tile = tiles[gid];
    uint first = tile.y * params.stride + tile.x * 4;
    uint b = pixels[first];
    uint g = pixels[first + 1];
    uint r = pixels[first + 2];
    uint a = pixels[first + 3];
    bool solid = true;

    for (uint row = 0; row < tile.height && solid; row++) {
        uint row_offset = (tile.y + row) * params.stride + tile.x * 4;
        for (uint col = 0; col < tile.width; col++) {
            uint offset = row_offset + col * 4;
            if (pixels[offset] != b ||
                pixels[offset + 1] != g ||
                pixels[offset + 2] != r ||
                pixels[offset + 3] != a) {
                solid = false;
                break;
            }
        }
    }

    classes[gid].solid = solid ? 1 : 0;
    classes[gid].bgra = b | (g << 8) | (r << 16) | (a << 24);
}
"#;

#[derive(Clone)]
struct CapturedFrame {
    width: u32,
    height: u32,
    stride: u32,
    data: Vec<u8>,
    #[cfg(not(test))]
    screen_dirty_regions: Option<Vec<DirtyRegion>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ActiveDisplayGeometry {
    capture_width: u32,
    capture_height: u32,
    frame_x: f64,
    frame_y: f64,
    frame_width: f64,
    frame_height: f64,
}

impl ActiveDisplayGeometry {
    fn from_display(display: &SCDisplay) -> Self {
        let frame = display.frame();
        Self {
            capture_width: even_dimension(display.width()),
            capture_height: even_dimension(display.height()),
            frame_x: frame.origin.x,
            frame_y: frame.origin.y,
            frame_width: frame.size.width.max(1.0),
            frame_height: frame.size.height.max(1.0),
        }
    }
}

impl Default for ActiveDisplayGeometry {
    fn default() -> Self {
        Self {
            capture_width: 1,
            capture_height: 1,
            frame_x: 0.0,
            frame_y: 0.0,
            frame_width: 1.0,
            frame_height: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirtyRegion {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct CaptureOutputInfo {
    index: u32,
    display_id: u32,
    name: String,
    width: u32,
    height: u32,
    origin_x: f64,
    origin_y: f64,
    point_width: f64,
    point_height: f64,
    primary: bool,
}

enum ControlCommand {
    SwitchCaptureOutput(u32),
    StreamBitrate(u32),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct MouseButtonState {
    pressed_button: Option<u8>,
}

impl MouseButtonState {
    fn apply_button(&mut self, button: u8, down: bool) {
        if down {
            self.pressed_button = Some(button);
        } else if self.pressed_button == Some(button) {
            self.pressed_button = None;
        }
    }

    fn move_event(self) -> (u32, u32) {
        match self.pressed_button {
            Some(CG_MOUSE_BUTTON_LEFT) => {
                (CG_EVENT_LEFT_MOUSE_DRAGGED, CG_MOUSE_BUTTON_LEFT as u32)
            }
            Some(CG_MOUSE_BUTTON_RIGHT) => {
                (CG_EVENT_RIGHT_MOUSE_DRAGGED, CG_MOUSE_BUTTON_RIGHT as u32)
            }
            Some(button) => (CG_EVENT_OTHER_MOUSE_DRAGGED, button as u32),
            None => (CG_EVENT_MOUSE_MOVED, CG_MOUSE_BUTTON_LEFT as u32),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelperCaptureMode {
    H264,
    Legacy,
    Atx2,
    Screenshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyIvfTransition {
    InitializeStream,
    ContinueStream,
    RejectDimensionChange {
        current_width: u32,
        current_height: u32,
        next_width: u32,
        next_height: u32,
    },
}

struct FrameHandler {
    tx: mpsc::SyncSender<CapturedFrame>,
    queued_frames: Arc<AtomicUsize>,
}

impl SCStreamOutputTrait for FrameHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, _: SCStreamOutputType) {
        if !reserve_frame_slot(&self.queued_frames) {
            debug!("macOS helper dropped capture frame before copy due encoder backpressure");
            return;
        }
        let Some(pixel_buffer) = sample.image_buffer() else {
            release_frame_slot(&self.queued_frames);
            return;
        };
        let Ok(guard) = pixel_buffer.lock_read_only() else {
            release_frame_slot(&self.queued_frames);
            return;
        };
        let width = guard.width();
        let height = guard.height();
        let stride = guard.bytes_per_row();
        if width == 0 || height == 0 || stride < width.saturating_mul(4) {
            release_frame_slot(&self.queued_frames);
            return;
        }
        #[cfg(not(test))]
        let screen_dirty_regions =
            screen_capturekit_dirty_regions(&sample, width as u32, height as u32);
        let mut data = Vec::with_capacity(stride * height);
        for row_index in 0..height {
            let Some(row) = guard.row(row_index) else {
                release_frame_slot(&self.queued_frames);
                return;
            };
            data.extend_from_slice(row);
        }
        if let Err(err) = self.tx.try_send(CapturedFrame {
            width: width as u32,
            height: height as u32,
            stride: stride as u32,
            data,
            #[cfg(not(test))]
            screen_dirty_regions,
        }) {
            release_frame_slot(&self.queued_frames);
            debug!(error = %err, "macOS helper dropped capture frame due encoder backpressure");
        }
    }
}

pub fn run_from_args() -> Result<()> {
    init_logging();
    let options = Options::parse()?;
    ensure_permissions();

    let stop = Arc::new(AtomicBool::new(false));
    let mut stream_socket =
        UnixStream::connect(&options.stream_socket).context("connect stream socket")?;
    let mut control_socket =
        UnixStream::connect(&options.control_socket).context("connect control socket")?;
    write_handshake(&mut stream_socket, &options.auth_token)?;
    write_handshake(&mut control_socket, &options.auth_token)?;

    let stop_for_control = stop.clone();
    let display_for_control = Arc::new(std::sync::Mutex::new(ActiveDisplayGeometry::default()));
    let display_for_capture = display_for_control.clone();
    let (control_tx, control_rx) = mpsc::channel::<ControlCommand>();
    std::thread::spawn(move || {
        let stop_on_exit = stop_for_control.clone();
        if let Err(err) = control_loop(
            control_socket,
            stop_for_control,
            display_for_control,
            control_tx,
        ) {
            stop_on_exit.store(true, Ordering::SeqCst);
            warn!(error = %err, "macOS helper control loop ended");
        }
    });

    let queued_frames = Arc::new(AtomicUsize::new(0));
    let (frame_tx, frame_rx) = mpsc::sync_channel::<CapturedFrame>(FRAME_QUEUE_BOUND);
    let content = match get_shareable_content_compat() {
        Ok(content) => content,
        Err(err) => {
            let message = format!(
                "ScreenCaptureKit content enumeration failed ({err:?}); grant Screen Recording permission to talos_worker_helper"
            );
            let _ = write_capture_error(&mut stream_socket, "screen_recording_denied", &message);
            return Err(anyhow!(message));
        }
    };
    let displays = content.displays();
    if displays.is_empty() {
        let message = "ScreenCaptureKit returned no displays".to_string();
        let _ = write_capture_error(&mut stream_socket, "no_display", &message);
        return Err(anyhow!(message));
    }
    let outputs = capture_outputs_for_displays(&displays);

    capture_encode_loop(
        &mut stream_socket,
        frame_tx,
        frame_rx,
        queued_frames,
        control_rx,
        stop.clone(),
        options.fps,
        displays,
        outputs,
        display_for_capture,
    )
}

fn get_shareable_content_compat() -> Result<SCShareableContent> {
    let Some(class) = AnyClass::get(c"SCShareableContent") else {
        return Err(anyhow!("SCShareableContent class is not available"));
    };
    let (tx, rx) = mpsc::channel::<Result<*const c_void, String>>();
    let completion: RcBlock<dyn Fn(*mut AnyObject, *mut AnyObject)> =
        RcBlock::new(move |content: *mut AnyObject, error: *mut AnyObject| {
            let result = if !content.is_null() {
                unsafe { Retained::retain(content) }
                    .ok_or_else(|| {
                        "ScreenCaptureKit returned an invalid content object".to_string()
                    })
                    .map(|content| Retained::into_raw(content).cast_const().cast::<c_void>())
            } else if !error.is_null() {
                Err(unsafe { objective_c_error_description(error) })
            } else {
                Err("ScreenCaptureKit returned neither content nor error".to_string())
            };
            let _ = tx.send(result);
        });
    let completion_ref: &Block<dyn Fn(*mut AnyObject, *mut AnyObject)> = &completion;

    unsafe {
        let _: () = msg_send![
            class,
            getShareableContentExcludingDesktopWindows: false,
            onScreenWindowsOnly: false,
            completionHandler: completion_ref
        ];
    }

    let ptr = rx
        .recv_timeout(SHAREABLE_CONTENT_TIMEOUT)
        .map_err(|_| anyhow!("timed out waiting for ScreenCaptureKit shareable content"))?
        .map_err(|message| anyhow!(message))?;
    if ptr.is_null() {
        return Err(anyhow!("ScreenCaptureKit returned null shareable content"));
    }

    // The crate's public getter calls a newer Swift async thunk that is absent
    // on the macOS 13 VM. This Objective-C callback returns the same retained
    // object representation expected by SCShareableContent.
    Ok(unsafe { std::mem::transmute::<*const c_void, SCShareableContent>(ptr) })
}

unsafe fn objective_c_error_description(error: *mut AnyObject) -> String {
    let description: *mut AnyObject = unsafe { msg_send![error, localizedDescription] };
    if description.is_null() {
        return "ScreenCaptureKit content enumeration failed".to_string();
    }
    let bytes: *const c_char = unsafe { msg_send![description, UTF8String] };
    if bytes.is_null() {
        return "ScreenCaptureKit content enumeration failed".to_string();
    }
    unsafe { CStr::from_ptr(bytes) }
        .to_string_lossy()
        .trim()
        .to_string()
}

fn start_capture_stream(
    display: &SCDisplay,
    frame_tx: mpsc::SyncSender<CapturedFrame>,
    queued_frames: Arc<AtomicUsize>,
    fps: u32,
    hide_cursor: bool,
    display_geometry: &Arc<std::sync::Mutex<ActiveDisplayGeometry>>,
) -> Result<SCStream> {
    let geometry = ActiveDisplayGeometry::from_display(display);
    {
        let mut guard = display_geometry.lock().unwrap();
        *guard = geometry;
    }
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build();
    let frame_interval = CMTime::new(1, fps as i32);
    let config = SCStreamConfiguration::new()
        .with_width(geometry.capture_width)
        .with_height(geometry.capture_height)
        .with_pixel_format(PixelFormat::BGRA)
        .with_queue_depth(SCK_CAPTURE_QUEUE_DEPTH)
        .with_shows_cursor(!hide_cursor)
        .with_minimum_frame_interval(&frame_interval);
    let mut sc_stream = SCStream::new(&filter, &config);
    sc_stream.add_output_handler(
        FrameHandler {
            tx: frame_tx,
            queued_frames,
        },
        SCStreamOutputType::Screen,
    );
    start_capture_compat(&sc_stream).map_err(|err| {
        anyhow!(
            "ScreenCaptureKit start_capture failed ({err:?}); grant Screen Recording permission"
        )
    })?;
    Ok(sc_stream)
}

fn start_capture_compat(stream: &SCStream) -> Result<()> {
    invoke_stream_capture_operation(
        stream,
        "startCaptureWithCompletionHandler",
        STREAM_START_TIMEOUT,
    )
}

fn stop_capture_compat(stream: &SCStream) -> Result<()> {
    invoke_stream_capture_operation(
        stream,
        "stopCaptureWithCompletionHandler",
        STREAM_STOP_TIMEOUT,
    )
}

fn invoke_stream_capture_operation(
    stream: &SCStream,
    operation: &'static str,
    timeout: Duration,
) -> Result<()> {
    let stream_obj = sc_stream_objc_object(stream)?;
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let completion: RcBlock<dyn Fn(*mut AnyObject)> = RcBlock::new(move |error: *mut AnyObject| {
        let result = if error.is_null() {
            Ok(())
        } else {
            Err(unsafe { objective_c_error_description(error) })
        };
        let _ = tx.send(result);
    });
    let completion_ref: &Block<dyn Fn(*mut AnyObject)> = &completion;

    unsafe {
        match operation {
            "startCaptureWithCompletionHandler" => {
                let _: () = msg_send![
                    stream_obj,
                    startCaptureWithCompletionHandler: completion_ref
                ];
            }
            "stopCaptureWithCompletionHandler" => {
                let _: () = msg_send![
                    stream_obj,
                    stopCaptureWithCompletionHandler: completion_ref
                ];
            }
            _ => return Err(anyhow!("unsupported ScreenCaptureKit stream operation")),
        }
    }

    rx.recv_timeout(timeout)
        .map_err(|_| anyhow!("timed out waiting for ScreenCaptureKit {operation}"))?
        .map_err(|message| anyhow!(message))
}

fn sc_stream_objc_object(stream: &SCStream) -> Result<*mut AnyObject> {
    // screencapturekit's public SCStream wraps a retained Objective-C SCStream pointer
    // as its first field. The crate's start/stop methods call Swift async thunks that
    // are missing on macOS 13, so this local shim reaches the same retained object and
    // uses the older completion-handler selectors instead.
    let stream_ptr = unsafe { *(stream as *const SCStream).cast::<*const c_void>() };
    if stream_ptr.is_null() {
        return Err(anyhow!("ScreenCaptureKit stream pointer is null"));
    }
    Ok(stream_ptr.cast_mut().cast::<AnyObject>())
}

#[derive(Serialize)]
struct MacosPermissionCheck {
    accessibility: bool,
    #[serde(rename = "screenRecording")]
    screen_recording: bool,
}

pub fn run_permission_check_from_args() -> Result<()> {
    let json_output = std::env::args().any(|arg| arg == "--json");
    let json_output_path = permission_check_output_path();
    let check = MacosPermissionCheck {
        accessibility: accessibility_trusted(),
        screen_recording: screen_recording_trusted(),
    };
    if let Some(path) = json_output_path {
        std::fs::write(&path, serde_json::to_vec(&check)?)
            .with_context(|| format!("write macOS permission check output: {path}"))?;
    }
    if json_output {
        println!("{}", serde_json::to_string(&check)?);
    } else {
        println!(
            "Accessibility: {}\nScreen Recording: {}",
            if check.accessibility {
                "granted"
            } else {
                "not granted"
            },
            if check.screen_recording {
                "granted"
            } else {
                "not granted"
            }
        );
    }
    Ok(())
}

fn permission_check_output_path() -> Option<String> {
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        if arg == "--json-output" {
            return args.next();
        }
    }
    None
}

fn capture_encode_loop(
    stream_socket: &mut UnixStream,
    frame_tx: mpsc::SyncSender<CapturedFrame>,
    frame_rx: mpsc::Receiver<CapturedFrame>,
    queued_frames: Arc<AtomicUsize>,
    control_rx: mpsc::Receiver<ControlCommand>,
    stop: Arc<AtomicBool>,
    fps: u32,
    displays: Vec<SCDisplay>,
    outputs: Vec<CaptureOutputInfo>,
    display_geometry: Arc<std::sync::Mutex<ActiveDisplayGeometry>>,
) -> Result<()> {
    let mut tuning = talos_worker::encode::load_encode_tuning_from_env();
    let mut encoder: Option<(u32, u32, talos_worker::encode::Vp8Encoder)> = None;
    let mut h264_encoder: Option<(u32, u32, MacosH264Encoder)> = None;
    let mut pts = 0u64;
    let mut last_liveness = Instant::now();
    let mut previous_frame: Option<CapturedFrame> = None;
    let mut duplicate_frames_skipped = 0u64;
    let mut metadata_sent = false;
    let mut legacy_stream_size: Option<(u32, u32)> = None;
    let capture_mode = selected_capture_mode();
    let atx2_enabled = capture_mode == HelperCaptureMode::Atx2;
    let h264_enabled = capture_mode == HelperCaptureMode::H264;
    let screenshot_enabled = capture_mode == HelperCaptureMode::Screenshot;
    let hide_cursor = capture_hides_cursor();
    let mut atx2_metal_classifier = if atx2_enabled {
        match MacAtx2MetalClassifier::new() {
            Ok(classifier) => Some(classifier),
            Err(err) => {
                debug!(error = %err, "macOS helper ATX2 Metal classifier unavailable; using CPU classifier");
                None
            }
        }
    } else {
        None
    };
    let mut active_index = 0u32;
    let mut sc_stream = match start_capture_stream(
        &displays[0],
        frame_tx.clone(),
        queued_frames.clone(),
        fps,
        hide_cursor,
        &display_geometry,
    ) {
        Ok(stream) => stream,
        Err(err) => {
            let message = err.to_string();
            let _ = write_capture_error(stream_socket, "screen_recording_denied", &message);
            return Err(err);
        }
    };

    while !stop.load(Ordering::Relaxed) {
        while let Ok(command) = control_rx.try_recv() {
            match command {
                ControlCommand::SwitchCaptureOutput(next_index) => {
                    if next_index == active_index {
                        continue;
                    }
                    let Some(display) = displays.get(next_index as usize) else {
                        warn!(
                            requested_index = next_index,
                            output_count = displays.len(),
                            "macOS helper ignored invalid capture output switch"
                        );
                        continue;
                    };
                    let next_width = even_dimension(display.width());
                    let next_height = even_dimension(display.height());
                    if capture_mode == HelperCaptureMode::Legacy {
                        if let LegacyIvfTransition::RejectDimensionChange {
                            current_width,
                            current_height,
                            next_width,
                            next_height,
                        } = legacy_ivf_transition(legacy_stream_size, next_width, next_height)
                        {
                            let message = format!(
                                "legacy IVF stream size change unsupported: current={}x{} next={}x{}",
                                current_width, current_height, next_width, next_height
                            );
                            let _ = stop_capture_compat(&sc_stream);
                            let _ = write_capture_error(
                                stream_socket,
                                "legacy_dimension_change_unsupported",
                                &message,
                            );
                            return Ok(());
                        }
                    }
                    let _ = stop_capture_compat(&sc_stream);
                    while frame_rx.try_recv().is_ok() {
                        release_frame_slot(&queued_frames);
                    }
                    sc_stream = match start_capture_stream(
                        display,
                        frame_tx.clone(),
                        queued_frames.clone(),
                        fps,
                        hide_cursor,
                        &display_geometry,
                    ) {
                        Ok(stream) => stream,
                        Err(err) => {
                            let message = err.to_string();
                            let _ = write_capture_error(
                                stream_socket,
                                "screen_recording_denied",
                                &message,
                            );
                            return Err(err);
                        }
                    };
                    active_index = next_index;
                    previous_frame = None;
                    duplicate_frames_skipped = 0;
                    if atx2_enabled || h264_enabled || screenshot_enabled {
                        encoder = None;
                        h264_encoder = None;
                        metadata_sent = false;
                    } else {
                        if let Some((stream_width, stream_height)) = legacy_stream_size {
                            write_legacy_metadata(
                                stream_socket,
                                stream_width,
                                stream_height,
                                fps,
                                tuning,
                                active_index,
                                &outputs,
                            )?;
                        } else {
                            encoder = None;
                            metadata_sent = false;
                        }
                    }
                    let display_id = display.display_id();
                    let display_width = display.width();
                    let display_height = display.height();
                    info!(
                        active_index,
                        display_id,
                        width = display_width,
                        height = display_height,
                        "macOS helper switched capture output"
                    );
                }
                ControlCommand::StreamBitrate(kbps) => {
                    let mut requested_tuning = tuning;
                    if apply_stream_bitrate_update(&mut requested_tuning, kbps) {
                        if atx2_enabled {
                            tuning = requested_tuning;
                            encoder = None;
                            h264_encoder = None;
                            previous_frame = None;
                            metadata_sent = false;
                            duplicate_frames_skipped = 0;
                            info!(kbps, "macOS helper applied live stream bitrate update");
                        } else if h264_enabled {
                            tuning = requested_tuning;
                            h264_encoder = None;
                            previous_frame = None;
                            metadata_sent = false;
                            duplicate_frames_skipped = 0;
                            info!(
                                kbps,
                                "macOS helper applied live H.264 stream bitrate update"
                            );
                        } else if screenshot_enabled {
                            tuning = requested_tuning;
                            metadata_sent = false;
                            info!(
                                kbps,
                                "macOS helper noted bitrate update during screenshot capture"
                            );
                        } else if let Some((width, height)) =
                            encoder.as_ref().map(|(width, height, _)| (*width, *height))
                        {
                            match talos_worker::encode::Vp8Encoder::new(
                                width,
                                height,
                                fps,
                                requested_tuning,
                            ) {
                                Ok(new_encoder) => {
                                    tuning = requested_tuning;
                                    encoder = Some((width, height, new_encoder));
                                    previous_frame = None;
                                    duplicate_frames_skipped = 0;
                                    if let Some((stream_width, stream_height)) = legacy_stream_size
                                    {
                                        write_legacy_metadata(
                                            stream_socket,
                                            stream_width,
                                            stream_height,
                                            fps,
                                            tuning,
                                            active_index,
                                            &outputs,
                                        )?;
                                    }
                                    info!(
                                        kbps,
                                        "macOS helper applied live legacy stream bitrate update"
                                    );
                                }
                                Err(err) => {
                                    warn!(
                                        kbps,
                                        error = %err,
                                        "macOS helper failed to recreate legacy VP8 encoder for bitrate update"
                                    );
                                }
                            }
                        } else {
                            tuning = requested_tuning;
                            previous_frame = None;
                            duplicate_frames_skipped = 0;
                            info!(
                                kbps,
                                "macOS helper stored legacy stream bitrate update before encoder init"
                            );
                        }
                    }
                }
            }
        }

        match frame_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => {
                release_frame_slot(&queued_frames);
                let frame = latest_frame_from_queue(frame, &frame_rx, Some(&queued_frames));
                let width = even_dimension(frame.width);
                let height = even_dimension(frame.height);
                if screenshot_enabled {
                    if !metadata_sent {
                        debug!(
                            width,
                            height,
                            fps,
                            active_index,
                            output_count = outputs.len(),
                            "macOS helper writing screenshot-only metadata"
                        );
                        write_chunk(
                            stream_socket,
                            0,
                            &build_screenshot_metadata(
                                width,
                                height,
                                fps,
                                tuning,
                                active_index,
                                &outputs,
                            )?,
                        )?;
                    }
                    debug!(
                        frame_id = pts,
                        width,
                        height,
                        stride = frame.stride,
                        source_bytes = frame.data.len(),
                        "macOS helper writing screenshot-only frame"
                    );
                    write_screenshot_frame(stream_socket, pts, &frame, width, height)
                        .context("write macOS screenshot frame")?;
                    debug!(
                        frame_id = pts,
                        width, height, "macOS helper wrote screenshot-only frame"
                    );
                    let _ = stop_capture_compat(&sc_stream);
                    let stop_wait_start = Instant::now();
                    while !stop.load(Ordering::Relaxed)
                        && stop_wait_start.elapsed() < SCREENSHOT_POST_FRAME_EXIT_GRACE
                    {
                        maybe_write_liveness(stream_socket, &mut last_liveness)?;
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    if stop.load(Ordering::Relaxed) {
                        debug!("macOS helper screenshot-only stop received after frame");
                    } else {
                        info!(
                            grace_ms = SCREENSHOT_POST_FRAME_EXIT_GRACE.as_millis(),
                            "macOS helper screenshot-only post-frame grace elapsed; exiting"
                        );
                    }
                    return Ok(());
                }
                if atx2_enabled {
                    if !metadata_sent {
                        write_chunk(
                            stream_socket,
                            0,
                            &build_atx2_metadata(
                                width,
                                height,
                                fps,
                                tuning,
                                active_index,
                                &outputs,
                            )?,
                        )?;
                        let bootstrap_frame_id = pts.saturating_add(1);
                        write_atx2_black_bootstrap(
                            stream_socket,
                            bootstrap_frame_id,
                            width,
                            height,
                        )
                        .context("write macOS ATX2 black bootstrap")?;
                        pts = bootstrap_frame_id.saturating_add(1);
                        metadata_sent = true;
                    }
                    if write_atx2_frame(
                        stream_socket,
                        pts,
                        &frame,
                        previous_frame.as_ref(),
                        width,
                        height,
                        atx2_metal_classifier.as_mut(),
                    )? {
                        pts = pts.saturating_add(1);
                        if duplicate_frames_skipped > 0 {
                            debug!(
                                duplicate_frames_skipped,
                                "macOS helper skipped duplicate desktop frames"
                            );
                            duplicate_frames_skipped = 0;
                        }
                        previous_frame = Some(frame);
                    } else {
                        duplicate_frames_skipped = duplicate_frames_skipped.saturating_add(1);
                        maybe_write_liveness(stream_socket, &mut last_liveness)?;
                    }
                    continue;
                }
                if previous_frame
                    .as_ref()
                    .is_some_and(|previous| same_visible_frame(previous, &frame, width, height))
                {
                    duplicate_frames_skipped = duplicate_frames_skipped.saturating_add(1);
                    maybe_write_liveness(stream_socket, &mut last_liveness)?;
                    continue;
                }
                if h264_enabled {
                    if h264_encoder
                        .as_ref()
                        .is_none_or(|(w, h, _)| *w != width || *h != height)
                    {
                        match MacosH264Encoder::new(width, height, fps, h264_bitrate_bps(tuning)) {
                            Ok(enc) => {
                                h264_encoder = Some((width, height, enc));
                                write_chunk(
                                    stream_socket,
                                    0,
                                    &build_h264_metadata(
                                        width,
                                        height,
                                        fps,
                                        tuning,
                                        active_index,
                                        &outputs,
                                    )?,
                                )?;
                                metadata_sent = true;
                            }
                            Err(err) => {
                                let message = err.to_string();
                                let _ = write_capture_error(
                                    stream_socket,
                                    "h264_encoder_unavailable",
                                    &message,
                                );
                                return Err(err).context("create VideoToolbox H.264 encoder");
                            }
                        }
                    } else if !metadata_sent {
                        write_chunk(
                            stream_socket,
                            0,
                            &build_h264_metadata(
                                width,
                                height,
                                fps,
                                tuning,
                                active_index,
                                &outputs,
                            )?,
                        )?;
                        metadata_sent = true;
                    }
                    let Some((_, _, enc)) = h264_encoder.as_mut() else {
                        continue;
                    };
                    let force_keyframe = previous_frame.is_none();
                    let encoded = match enc.encode_bgra(&frame.data, frame.stride, force_keyframe) {
                        Ok(Some(encoded)) => encoded,
                        Ok(None) => {
                            maybe_write_liveness(stream_socket, &mut last_liveness)?;
                            continue;
                        }
                        Err(err) => {
                            let message = err.to_string();
                            let _ =
                                write_capture_error(stream_socket, "h264_encode_failed", &message);
                            return Err(err).context("encode VideoToolbox H.264 frame");
                        }
                    };
                    write_h264_frame(stream_socket, pts, width, height, &encoded)?;
                    pts = pts.saturating_add(1);
                    if duplicate_frames_skipped > 0 {
                        debug!(
                            duplicate_frames_skipped,
                            "macOS helper skipped duplicate desktop frames"
                        );
                        duplicate_frames_skipped = 0;
                    }
                    previous_frame = Some(frame);
                    continue;
                }
                if encoder
                    .as_ref()
                    .is_none_or(|(w, h, _)| *w != width || *h != height)
                {
                    match legacy_ivf_transition(legacy_stream_size, width, height) {
                        LegacyIvfTransition::InitializeStream => {
                            let enc =
                                talos_worker::encode::Vp8Encoder::new(width, height, fps, tuning)
                                    .context("create VP8 encoder")?;
                            encoder = Some((width, height, enc));
                            legacy_stream_size = Some((width, height));
                            write_legacy_metadata(
                                stream_socket,
                                width,
                                height,
                                fps,
                                tuning,
                                active_index,
                                &outputs,
                            )?;
                            write_chunk(
                                stream_socket,
                                1,
                                &talos_worker::encode::build_header(width, height, fps),
                            )?;
                            metadata_sent = true;
                        }
                        LegacyIvfTransition::ContinueStream => {
                            let enc =
                                talos_worker::encode::Vp8Encoder::new(width, height, fps, tuning)
                                    .context("create VP8 encoder")?;
                            encoder = Some((width, height, enc));
                        }
                        LegacyIvfTransition::RejectDimensionChange {
                            current_width,
                            current_height,
                            next_width,
                            next_height,
                        } => {
                            let message = format!(
                                "legacy IVF stream size change unsupported: current={}x{} next={}x{}",
                                current_width, current_height, next_width, next_height
                            );
                            let _ = stop_capture_compat(&sc_stream);
                            let _ = write_capture_error(
                                stream_socket,
                                "legacy_dimension_change_unsupported",
                                &message,
                            );
                            return Ok(());
                        }
                    }
                }
                let i420 = talos_worker::encode::bgra_bytes_to_i420(
                    &frame.data,
                    width,
                    height,
                    frame.stride,
                    tuning.preset.grayscale_chroma(),
                )?;
                let Some((_, _, enc)) = encoder.as_mut() else {
                    continue;
                };
                let payload = enc.encode(&i420, pts as i64)?;
                let mut frame_bytes = Vec::with_capacity(12 + payload.len());
                frame_bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                frame_bytes.extend_from_slice(&pts.to_le_bytes());
                frame_bytes.extend_from_slice(&payload);
                write_chunk(stream_socket, 2, &frame_bytes)?;
                pts = pts.saturating_add(1);
                if duplicate_frames_skipped > 0 {
                    debug!(
                        duplicate_frames_skipped,
                        "macOS helper skipped duplicate desktop frames"
                    );
                    duplicate_frames_skipped = 0;
                }
                previous_frame = Some(frame);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                maybe_write_liveness(stream_socket, &mut last_liveness)?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = stop_capture_compat(&sc_stream);
    Ok(())
}

fn selected_capture_mode() -> HelperCaptureMode {
    match std::env::args().nth(1).as_deref() {
        Some("capture-macos-screenshot") => HelperCaptureMode::Screenshot,
        Some("capture-macos-atx2") => HelperCaptureMode::Atx2,
        Some("capture-macos-legacy") => HelperCaptureMode::Legacy,
        Some("capture-macos-h264") => HelperCaptureMode::H264,
        _ => HelperCaptureMode::H264,
    }
}

fn capture_hides_cursor() -> bool {
    capture_hides_cursor_from_args(std::env::args())
}

fn capture_hides_cursor_from_args<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .any(|arg| arg.as_ref().eq_ignore_ascii_case("--hide-cursor"))
}

fn apply_stream_bitrate_update(tuning: &mut talos_worker::encode::EncodeTuning, kbps: u32) -> bool {
    if kbps == 0 || tuning.bitrate_override_kbps == Some(kbps) {
        return false;
    }
    tuning.bitrate_override_kbps = Some(kbps);
    true
}

fn reserve_frame_slot(queued_frames: &AtomicUsize) -> bool {
    let mut queued = queued_frames.load(Ordering::Acquire);
    loop {
        if queued >= FRAME_QUEUE_BOUND {
            return false;
        }
        match queued_frames.compare_exchange_weak(
            queued,
            queued + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(current) => queued = current,
        }
    }
}

fn release_frame_slot(queued_frames: &AtomicUsize) {
    let mut queued = queued_frames.load(Ordering::Acquire);
    loop {
        if queued == 0 {
            return;
        }
        match queued_frames.compare_exchange_weak(
            queued,
            queued - 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return,
            Err(current) => queued = current,
        }
    }
}

fn latest_frame_from_queue(
    mut frame: CapturedFrame,
    frame_rx: &mpsc::Receiver<CapturedFrame>,
    queued_frames: Option<&AtomicUsize>,
) -> CapturedFrame {
    while let Ok(newer_frame) = frame_rx.try_recv() {
        if let Some(queued_frames) = queued_frames {
            release_frame_slot(queued_frames);
        }
        frame = newer_frame;
    }
    frame
}

fn same_visible_frame(
    previous: &CapturedFrame,
    current: &CapturedFrame,
    width: u32,
    height: u32,
) -> bool {
    previous.width == current.width
        && previous.height == current.height
        && previous.stride == current.stride
        && previous.data.len() == current.data.len()
        && (previous.data == current.data || visible_pixels_equal(previous, current, width, height))
}

fn maybe_write_liveness(stream_socket: &mut UnixStream, last_liveness: &mut Instant) -> Result<()> {
    if last_liveness.elapsed() >= Duration::from_secs(1) {
        write_chunk(stream_socket, 6, &[])?;
        *last_liveness = Instant::now();
    }
    Ok(())
}

fn legacy_ivf_transition(
    current_stream_size: Option<(u32, u32)>,
    next_width: u32,
    next_height: u32,
) -> LegacyIvfTransition {
    match current_stream_size {
        None => LegacyIvfTransition::InitializeStream,
        Some((current_width, current_height))
            if current_width == next_width && current_height == next_height =>
        {
            LegacyIvfTransition::ContinueStream
        }
        Some((current_width, current_height)) => LegacyIvfTransition::RejectDimensionChange {
            current_width,
            current_height,
            next_width,
            next_height,
        },
    }
}

fn write_legacy_metadata(
    stream_socket: &mut UnixStream,
    width: u32,
    height: u32,
    fps: u32,
    tuning: talos_worker::encode::EncodeTuning,
    active_index: u32,
    outputs: &[CaptureOutputInfo],
) -> Result<()> {
    write_chunk(
        stream_socket,
        0,
        &build_metadata(width, height, fps, tuning, active_index, outputs),
    )
}

fn build_metadata(
    _width: u32,
    _height: u32,
    fps: u32,
    tuning: talos_worker::encode::EncodeTuning,
    active_index: u32,
    outputs: &[CaptureOutputInfo],
) -> Vec<u8> {
    let payload = json!({
        "bitrate_kbps": tuning.bitrate_kbps(),
        "preset": tuning.preset.as_str(),
        "cpu_used": tuning.cpu_used,
        "encoding_fps": fps,
        "captureType": "macos_screencapturekit_legacy",
        "activeIndex": active_index,
        "captureOutputs": outputs
    });
    let json_bytes = serde_json::to_vec(&payload).unwrap_or_default();
    let mut msg = Vec::with_capacity(8 + json_bytes.len());
    msg.extend_from_slice(b"RMMD");
    msg.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    msg.extend_from_slice(&json_bytes);
    msg
}

fn build_h264_metadata(
    width: u32,
    height: u32,
    fps: u32,
    tuning: talos_worker::encode::EncodeTuning,
    active_index: u32,
    outputs: &[CaptureOutputInfo],
) -> Result<Vec<u8>> {
    let descriptor = DisplayStreamDescriptor::modern_capture(width, height);
    let mut payload = json!({
        "bitrate_kbps": tuning.bitrate_kbps(),
        "preset": tuning.preset.as_str(),
        "encoding_fps": fps,
        "agent_monitor_hz": null,
        "captureType": "macos_screencapturekit_h264",
        "activeIndex": active_index,
        "captureOutputs": outputs
    });
    if let Some(obj) = payload.as_object_mut() {
        obj.insert(
            DISPLAY_STREAM_META_TYPE.to_string(),
            serde_json::to_value(descriptor).context("serialize display stream descriptor")?,
        );
    }
    let json_bytes = serde_json::to_vec(&payload).context("serialize macOS H.264 metadata")?;
    let mut msg = Vec::with_capacity(8 + json_bytes.len());
    msg.extend_from_slice(b"RMMD");
    msg.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    msg.extend_from_slice(&json_bytes);
    Ok(msg)
}

fn h264_bitrate_bps(tuning: talos_worker::encode::EncodeTuning) -> u32 {
    tuning.bitrate_kbps().saturating_mul(1000).max(250_000)
}

fn build_atx2_metadata(
    width: u32,
    height: u32,
    fps: u32,
    tuning: talos_worker::encode::EncodeTuning,
    active_index: u32,
    outputs: &[CaptureOutputInfo],
) -> Result<Vec<u8>> {
    let descriptor = DisplayStreamDescriptor::experimental_capture(width, height);
    let mut payload = json!({
        "bitrate_kbps": tuning.bitrate_kbps(),
        "preset": tuning.preset.as_str(),
        "encoding_fps": fps,
        "agent_monitor_hz": null,
        "captureType": "macos_screencapturekit_atx2",
        "experimental": {
            "payload": "atx2",
            "tileCommandFormat": "ATX2",
            "tileSize": ATX2_TILE_SIZE,
            "dirtyRectSource": "ScreenCaptureKit",
            "bootstrap": "solidBlack",
            "classifier": "metalTileClassifierWithCpuFallback",
            "exactCommandKinds": ["rawBgra", "solidColor"],
        },
        "activeIndex": active_index,
        "captureOutputs": outputs
    });
    if let Some(obj) = payload.as_object_mut() {
        obj.insert(
            DISPLAY_STREAM_META_TYPE.to_string(),
            serde_json::to_value(descriptor).context("serialize display stream descriptor")?,
        );
    }
    let json_bytes = serde_json::to_vec(&payload).context("serialize macOS ATX2 metadata")?;
    let mut msg = Vec::with_capacity(8 + json_bytes.len());
    msg.extend_from_slice(b"RMMD");
    msg.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    msg.extend_from_slice(&json_bytes);
    Ok(msg)
}

fn build_screenshot_metadata(
    width: u32,
    height: u32,
    fps: u32,
    tuning: talos_worker::encode::EncodeTuning,
    active_index: u32,
    outputs: &[CaptureOutputInfo],
) -> Result<Vec<u8>> {
    let descriptor = DisplayStreamDescriptor::screenshot_only(width, height);
    let mut payload = json!({
        "bitrate_kbps": tuning.bitrate_kbps(),
        "preset": tuning.preset.as_str(),
        "encoding_fps": fps,
        "agent_monitor_hz": null,
        "captureType": "macos_screencapturekit_screenshot",
        "activeIndex": active_index,
        "captureOutputs": outputs
    });
    if let Some(obj) = payload.as_object_mut() {
        obj.insert(
            DISPLAY_STREAM_META_TYPE.to_string(),
            serde_json::to_value(descriptor).context("serialize screenshot stream descriptor")?,
        );
    }
    let json_bytes = serde_json::to_vec(&payload).context("serialize macOS screenshot metadata")?;
    let mut msg = Vec::with_capacity(8 + json_bytes.len());
    msg.extend_from_slice(b"RMMD");
    msg.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    msg.extend_from_slice(&json_bytes);
    Ok(msg)
}

fn capture_outputs_for_displays(displays: &[SCDisplay]) -> Vec<CaptureOutputInfo> {
    displays
        .iter()
        .enumerate()
        .map(|(index, display)| {
            let width = even_dimension(display.width());
            let height = even_dimension(display.height());
            let frame = display.frame();
            CaptureOutputInfo {
                index: index as u32,
                display_id: display.display_id(),
                name: if index == 0 {
                    "Main Display".to_string()
                } else {
                    format!("Display {} ({}x{})", index + 1, width, height)
                },
                width,
                height,
                origin_x: frame.origin.x,
                origin_y: frame.origin.y,
                point_width: frame.size.width,
                point_height: frame.size.height,
                primary: index == 0,
            }
        })
        .collect()
}

fn write_screenshot_frame(
    stream_socket: &mut UnixStream,
    frame_id: u64,
    frame: &CapturedFrame,
    width: u32,
    height: u32,
) -> Result<()> {
    let bgra = visible_bgra_bytes(frame, width, height)?;
    debug!(
        frame_id,
        width,
        height,
        bgra_bytes = bgra.len(),
        "macOS helper screenshot BGRA prepared"
    );
    write_chunk(
        stream_socket,
        5,
        &build_display_frame_begin(frame_id, width, height),
    )?;
    debug!(frame_id, "macOS helper wrote screenshot frame begin record");
    write_chunk(
        stream_socket,
        5,
        &build_display_keyframe(frame_id, width, height, bgra.len() as u32, &bgra),
    )?;
    debug!(
        frame_id,
        bgra_bytes = bgra.len(),
        "macOS helper wrote screenshot keyframe record"
    );
    write_chunk(stream_socket, 5, &build_display_frame_end(frame_id))?;
    debug!(frame_id, "macOS helper wrote screenshot frame end record");
    Ok(())
}

fn visible_bgra_bytes(frame: &CapturedFrame, width: u32, height: u32) -> Result<Vec<u8>> {
    ensure!(
        width > 0 && height > 0,
        "screenshot frame dimensions are empty"
    );
    let row_bytes = width
        .checked_mul(4)
        .context("screenshot row byte width overflow")? as usize;
    let stride = frame.stride as usize;
    ensure!(
        stride >= row_bytes,
        "screenshot frame stride {} is smaller than visible row bytes {}",
        frame.stride,
        row_bytes
    );
    let height_usize = height as usize;
    let required_len = stride
        .checked_mul(height_usize)
        .context("screenshot frame byte length overflow")?;
    ensure!(
        frame.data.len() >= required_len,
        "screenshot frame data length {} is smaller than required {}",
        frame.data.len(),
        required_len
    );
    if stride == row_bytes && frame.data.len() == row_bytes.saturating_mul(height_usize) {
        return Ok(frame.data.clone());
    }
    let mut bgra = Vec::with_capacity(row_bytes.saturating_mul(height_usize));
    for row in 0..height_usize {
        let start = row.saturating_mul(stride);
        bgra.extend_from_slice(&frame.data[start..start + row_bytes]);
    }
    Ok(bgra)
}

fn write_atx2_frame(
    stream_socket: &mut UnixStream,
    frame_id: u64,
    frame: &CapturedFrame,
    previous_frame: Option<&CapturedFrame>,
    width: u32,
    height: u32,
    metal_classifier: Option<&mut MacAtx2MetalClassifier>,
) -> Result<bool> {
    let records = build_atx2_frame_atlas_records(
        frame_id,
        frame,
        previous_frame,
        width,
        height,
        metal_classifier,
    )?;
    if records.is_empty() {
        return Ok(false);
    }
    let frame_begin = build_display_frame_begin(frame_id, width, height);
    write_chunk(stream_socket, 5, &frame_begin)?;
    for record in records {
        write_chunk(stream_socket, 5, &record)?;
    }
    let frame_end = build_display_frame_end(frame_id);
    write_chunk(stream_socket, 5, &frame_end)?;
    Ok(true)
}

fn write_h264_frame(
    stream_socket: &mut UnixStream,
    frame_id: u64,
    width: u32,
    height: u32,
    encoded: &crate::macos_h264::EncodedH264Frame,
) -> Result<()> {
    let rect = DisplayAtlasRect {
        dst_x: 0,
        dst_y: 0,
        width,
        height,
        atlas_x: 0,
        atlas_y: 0,
    };
    let flags = if encoded.keyframe {
        DISPLAY_ATLAS_H264_FLAG_KEYFRAME
    } else {
        0
    };
    write_chunk(
        stream_socket,
        5,
        &build_display_frame_begin(frame_id, width, height),
    )?;
    write_chunk(
        stream_socket,
        5,
        &build_display_atlas_h264(frame_id, flags, width, height, &[rect], &encoded.payload),
    )?;
    write_chunk(stream_socket, 5, &build_display_frame_end(frame_id))?;
    Ok(())
}

fn write_atx2_black_bootstrap(
    stream_socket: &mut UnixStream,
    frame_id: u64,
    width: u32,
    height: u32,
) -> Result<()> {
    let tile_commands = build_solid_atx2_stream(width, height, [0, 0, 0, 0xff])
        .context("build macOS ATX2 black bootstrap stream")?;
    let rect = DisplayAtlasRect {
        dst_x: 0,
        dst_y: 0,
        width,
        height,
        atlas_x: 0,
        atlas_y: 0,
    };
    let frame_begin = build_display_frame_begin(frame_id, width, height);
    write_chunk(stream_socket, 5, &frame_begin)?;
    let atlas_record =
        build_display_experimental_atlas_commands(frame_id, width, height, &[rect], &tile_commands);
    write_chunk(stream_socket, 5, &atlas_record)?;
    let frame_end = build_display_frame_end(frame_id);
    write_chunk(stream_socket, 5, &frame_end)?;
    Ok(())
}

fn build_atx2_frame_atlas_records(
    frame_id: u64,
    frame: &CapturedFrame,
    previous_frame: Option<&CapturedFrame>,
    width: u32,
    height: u32,
    mut metal_classifier: Option<&mut MacAtx2MetalClassifier>,
) -> Result<Vec<Vec<u8>>> {
    ensure!(width > 0 && height > 0, "ATX2 frame dimensions are empty");
    ensure!(
        width <= u16::MAX as u32 && height <= u16::MAX as u32,
        "ATX2 frame dimensions exceed wire field range"
    );
    let row_bytes = width as usize * 4;
    let max_payload_bytes =
        ATX2_TARGET_STREAM_BYTES.saturating_sub(ATX2_HEADER_BYTES + ATX2_COMMAND_HEADER_BYTES + 64);
    let rows_per_chunk = (max_payload_bytes / row_bytes).max(1) as u32;
    let dirty_regions = dirty_atx2_regions(previous_frame, frame, width, height, rows_per_chunk);
    let chunk_count = dirty_regions.len() as u32;
    let mut records = Vec::with_capacity(dirty_regions.len());
    for (chunk_index, region) in dirty_regions.into_iter().enumerate() {
        let tile_commands =
            build_exact_atx2_stream_region(frame, region, metal_classifier.as_deref_mut())?;
        let rect = DisplayAtlasRect {
            dst_x: region.x,
            dst_y: region.y,
            width: region.width,
            height: region.height,
            atlas_x: 0,
            atlas_y: 0,
        };
        if chunk_count == 1 {
            records.push(build_display_experimental_atlas_commands(
                frame_id,
                region.width,
                region.height,
                &[rect],
                &tile_commands,
            ));
        } else {
            let flags = DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE
                | if chunk_index as u32 + 1 == chunk_count {
                    DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL
                } else {
                    0
                };
            records.push(build_display_experimental_atlas_commands_chunk(
                frame_id,
                flags,
                chunk_index as u32,
                chunk_count,
                region.width,
                region.height,
                &[rect],
                &tile_commands,
            ));
        }
    }
    Ok(records)
}

fn dirty_atx2_regions(
    previous_frame: Option<&CapturedFrame>,
    frame: &CapturedFrame,
    width: u32,
    height: u32,
    max_rows_per_band: u32,
) -> Vec<DirtyRegion> {
    let Some(previous) = previous_frame else {
        return full_row_bands(height, max_rows_per_band)
            .into_iter()
            .map(|(y, band_height)| DirtyRegion {
                x: 0,
                y,
                width,
                height: band_height,
            })
            .collect();
    };
    if previous.width != frame.width
        || previous.height != frame.height
        || previous.stride != frame.stride
        || previous.data.len() != frame.data.len()
    {
        return full_row_bands(height, max_rows_per_band)
            .into_iter()
            .map(|(y, band_height)| DirtyRegion {
                x: 0,
                y,
                width,
                height: band_height,
            })
            .collect();
    }
    if let Some(capture_dirty_regions) = captured_screen_dirty_regions(frame) {
        let regions =
            split_capture_dirty_regions(capture_dirty_regions, width, height, max_rows_per_band);
        if !regions.is_empty() {
            return regions;
        }
    }
    if previous.data == frame.data
        || (frame.stride != width.saturating_mul(4)
            && visible_pixels_equal(previous, frame, width, height))
    {
        return Vec::new();
    }
    dirty_tile_run_regions(
        previous,
        frame,
        width,
        height,
        max_rows_per_band.max(MACOS_ATX2_DIRTY_TILE_SIZE),
    )
}

#[cfg(not(test))]
fn captured_screen_dirty_regions(frame: &CapturedFrame) -> Option<&[DirtyRegion]> {
    frame.screen_dirty_regions.as_deref()
}

#[cfg(test)]
fn captured_screen_dirty_regions(_frame: &CapturedFrame) -> Option<&[DirtyRegion]> {
    None
}

#[cfg(not(test))]
fn screen_capturekit_dirty_regions(
    sample: &CMSampleBuffer,
    width: u32,
    height: u32,
) -> Option<Vec<DirtyRegion>> {
    let rects = sample.dirty_rects()?;
    let regions = normalize_capture_dirty_rects(
        rects.iter().map(|rect| {
            (
                rect.origin.x,
                rect.origin.y,
                rect.size.width,
                rect.size.height,
            )
        }),
        width,
        height,
    );
    if regions.is_empty() {
        None
    } else {
        Some(regions)
    }
}

fn normalize_capture_dirty_rects<I>(rects: I, width: u32, height: u32) -> Vec<DirtyRegion>
where
    I: IntoIterator<Item = (f64, f64, f64, f64)>,
{
    let mut regions: Vec<_> = rects
        .into_iter()
        .filter_map(|(x, y, rect_width, rect_height)| {
            normalize_capture_dirty_rect(x, y, rect_width, rect_height, width, height)
        })
        .collect();
    regions.sort_by_key(|region| (region.y, region.x, region.height, region.width));
    let mut merged = Vec::new();
    for region in regions {
        push_dirty_region(&mut merged, region, u32::MAX);
    }
    merged
}

fn normalize_capture_dirty_rect(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    frame_width: u32,
    frame_height: u32,
) -> Option<DirtyRegion> {
    if !x.is_finite()
        || !y.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || frame_width == 0
        || frame_height == 0
    {
        return None;
    }
    let left = x.floor().max(0.0).min(frame_width as f64) as u32;
    let top = y.floor().max(0.0).min(frame_height as f64) as u32;
    let right = (x + width).ceil().max(0.0).min(frame_width as f64) as u32;
    let bottom = (y + height).ceil().max(0.0).min(frame_height as f64) as u32;
    if right <= left || bottom <= top {
        return None;
    }
    Some(DirtyRegion {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

fn split_capture_dirty_regions(
    regions: &[DirtyRegion],
    width: u32,
    height: u32,
    max_rows_per_band: u32,
) -> Vec<DirtyRegion> {
    let mut split = Vec::new();
    let max_rows = max_rows_per_band.max(1);
    for region in regions {
        let Some(x_end) = region.x.checked_add(region.width) else {
            continue;
        };
        let Some(y_end) = region.y.checked_add(region.height) else {
            continue;
        };
        if region.width == 0
            || region.height == 0
            || region.x >= width
            || region.y >= height
            || x_end == 0
            || y_end == 0
        {
            continue;
        }
        let clipped = DirtyRegion {
            x: region.x.min(width),
            y: region.y.min(height),
            width: x_end.min(width).saturating_sub(region.x.min(width)),
            height: y_end.min(height).saturating_sub(region.y.min(height)),
        };
        if clipped.width == 0 || clipped.height == 0 {
            continue;
        }
        let mut y = clipped.y;
        while y < clipped.y + clipped.height {
            let band_height = max_rows.min(clipped.y + clipped.height - y);
            push_dirty_region(
                &mut split,
                DirtyRegion {
                    x: clipped.x,
                    y,
                    width: clipped.width,
                    height: band_height,
                },
                max_rows,
            );
            y += band_height;
        }
    }
    split
}

fn visible_pixels_equal(
    previous: &CapturedFrame,
    frame: &CapturedFrame,
    width: u32,
    height: u32,
) -> bool {
    if previous.width != frame.width
        || previous.height != frame.height
        || previous.stride != frame.stride
        || previous.data.len() != frame.data.len()
    {
        return false;
    }
    let stride = frame.stride as usize;
    let row_bytes = width as usize * 4;
    for row in 0..height as usize {
        let offset = row.saturating_mul(stride);
        let end = offset.saturating_add(row_bytes);
        if end > frame.data.len()
            || end > previous.data.len()
            || frame.data[offset..end] != previous.data[offset..end]
        {
            return false;
        }
    }
    true
}

fn dirty_tile_run_regions(
    previous: &CapturedFrame,
    frame: &CapturedFrame,
    width: u32,
    height: u32,
    max_region_rows: u32,
) -> Vec<DirtyRegion> {
    let tiles_x = width.div_ceil(MACOS_ATX2_DIRTY_TILE_SIZE);
    let tiles_y = height.div_ceil(MACOS_ATX2_DIRTY_TILE_SIZE);
    let mut regions = Vec::new();
    for tile_y in 0..tiles_y {
        let y = tile_y * MACOS_ATX2_DIRTY_TILE_SIZE;
        let tile_height = MACOS_ATX2_DIRTY_TILE_SIZE.min(height - y);
        let mut dirty_tiles = vec![false; tiles_x as usize];
        mark_changed_tiles_in_band(previous, frame, width, y, tile_height, &mut dirty_tiles);
        let mut run_start: Option<u32> = None;
        for (tile_x, changed) in dirty_tiles.into_iter().enumerate() {
            let tile_x = tile_x as u32;
            let x = tile_x * MACOS_ATX2_DIRTY_TILE_SIZE;
            if changed {
                if run_start.is_none() {
                    run_start = Some(x);
                }
            } else if let Some(start_x) = run_start.take() {
                push_dirty_region(
                    &mut regions,
                    DirtyRegion {
                        x: start_x,
                        y,
                        width: x - start_x,
                        height: tile_height,
                    },
                    max_region_rows,
                );
            }
        }
        if let Some(start_x) = run_start {
            push_dirty_region(
                &mut regions,
                DirtyRegion {
                    x: start_x,
                    y,
                    width: width - start_x,
                    height: tile_height,
                },
                max_region_rows,
            );
        }
    }
    regions
}

fn mark_changed_tiles_in_band(
    previous: &CapturedFrame,
    frame: &CapturedFrame,
    width: u32,
    band_y: u32,
    band_height: u32,
    dirty_tiles: &mut [bool],
) {
    let stride = frame.stride as usize;
    let row_bytes = width as usize * 4;
    for row in band_y..band_y + band_height {
        let offset = row as usize * stride;
        let end = offset.saturating_add(row_bytes);
        if end > frame.data.len() || end > previous.data.len() {
            dirty_tiles.fill(true);
            return;
        }
        if let Some((start_byte, end_byte)) =
            changed_visible_row_bounds(&previous.data[offset..end], &frame.data[offset..end])
        {
            let start_pixel = (start_byte / 4) as u32;
            let end_pixel = end_byte.div_ceil(4) as u32;
            let start_tile = start_pixel / MACOS_ATX2_DIRTY_TILE_SIZE;
            let end_tile = end_pixel.saturating_sub(1).min(width.saturating_sub(1))
                / MACOS_ATX2_DIRTY_TILE_SIZE;
            for tile in start_tile..=end_tile {
                if let Some(slot) = dirty_tiles.get_mut(tile as usize) {
                    *slot = true;
                }
            }
        }
    }
}

fn changed_visible_row_bounds(previous: &[u8], current: &[u8]) -> Option<(usize, usize)> {
    if previous.len() != current.len() {
        return Some((0, current.len().max(previous.len())));
    }
    if previous == current {
        return None;
    }

    let start = first_changed_byte(previous, current);
    let end = last_changed_byte_end(previous, current, start);
    Some((start, end))
}

fn first_changed_byte(previous: &[u8], current: &[u8]) -> usize {
    let mut offset = 0usize;
    while offset + 8 <= current.len() {
        if previous[offset..offset + 8] != current[offset..offset + 8] {
            break;
        }
        offset += 8;
    }
    while offset < current.len() && previous[offset] == current[offset] {
        offset += 1;
    }
    offset
}

fn last_changed_byte_end(previous: &[u8], current: &[u8], start: usize) -> usize {
    let mut end = current.len();
    while end >= start + 8 {
        let block_start = end - 8;
        if previous[block_start..end] != current[block_start..end] {
            break;
        }
        end = block_start;
    }
    while end > start && previous[end - 1] == current[end - 1] {
        end -= 1;
    }
    end
}

fn push_dirty_region(regions: &mut Vec<DirtyRegion>, region: DirtyRegion, max_region_rows: u32) {
    if let Some(previous) = regions.last_mut() {
        let contiguous_y = previous.y.saturating_add(previous.height) == region.y;
        let can_merge = previous.x == region.x
            && previous.width == region.width
            && contiguous_y
            && previous.height.saturating_add(region.height) <= max_region_rows;
        if can_merge {
            previous.height = previous.height.saturating_add(region.height);
            return;
        }
    }
    regions.push(region);
}

fn full_row_bands(height: u32, max_rows_per_band: u32) -> Vec<(u32, u32)> {
    let mut bands = Vec::new();
    let mut y = 0u32;
    while y < height {
        let band_height = max_rows_per_band.min(height - y);
        bands.push((y, band_height));
        y += band_height;
    }
    bands
}

#[derive(Clone, Copy)]
struct Atx2TileClass {
    solid: bool,
    bgra: u32,
}

#[cfg(test)]
struct MacAtx2MetalClassifier;

#[cfg(test)]
impl MacAtx2MetalClassifier {
    fn new() -> Result<Self> {
        Err(anyhow!("Metal classifier disabled in unit tests"))
    }

    fn classify_tiles(
        &mut self,
        _frame: &CapturedFrame,
        _tiles: &[DirtyRegion],
    ) -> Result<Vec<Atx2TileClass>> {
        Err(anyhow!("Metal classifier disabled in unit tests"))
    }
}

#[cfg(not(test))]
struct MacAtx2MetalClassifier {
    device: MetalDevice,
    queue: CommandQueue,
    pipeline: ComputePipelineState,
}

#[cfg(not(test))]
#[repr(C)]
#[derive(Clone, Copy)]
struct MetalAtx2TileDescriptor {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[cfg(not(test))]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MetalAtx2TileClass {
    solid: u32,
    bgra: u32,
}

#[cfg(not(test))]
#[repr(C)]
#[derive(Clone, Copy)]
struct MetalAtx2ClassifyParams {
    stride: u32,
    tile_count: u32,
}

#[cfg(not(test))]
impl MacAtx2MetalClassifier {
    fn new() -> Result<Self> {
        let device = MetalDevice::system_default().ok_or_else(|| anyhow!("Metal unavailable"))?;
        let queue = device
            .new_command_queue()
            .ok_or_else(|| anyhow!("Metal command queue unavailable"))?;
        let library = device
            .new_library_with_source(ATX2_METAL_CLASSIFIER_SHADER)
            .map_err(|err| anyhow!("compile ATX2 Metal classifier: {err}"))?;
        let function = library
            .new_function("classify_atx2_tiles")
            .ok_or_else(|| anyhow!("ATX2 Metal classifier kernel unavailable"))?;
        let pipeline = device
            .new_compute_pipeline_state(&function)
            .map_err(|err| anyhow!("create ATX2 Metal classifier pipeline: {err}"))?;
        Ok(Self {
            device,
            queue,
            pipeline,
        })
    }

    fn classify_tiles(
        &mut self,
        frame: &CapturedFrame,
        tiles: &[DirtyRegion],
    ) -> Result<Vec<Atx2TileClass>> {
        if tiles.is_empty() {
            return Ok(Vec::new());
        }
        ensure!(!frame.data.is_empty(), "ATX2 Metal source frame is empty");
        let descriptors: Vec<_> = tiles
            .iter()
            .map(|tile| MetalAtx2TileDescriptor {
                x: tile.x,
                y: tile.y,
                width: tile.width,
                height: tile.height,
            })
            .collect();
        let params = MetalAtx2ClassifyParams {
            stride: frame.stride,
            tile_count: descriptors.len() as u32,
        };

        let source_buffer = self.new_shared_buffer(frame.data.len(), "ATX2 Metal source")?;
        let descriptor_buffer = self.new_shared_buffer(
            descriptors.len() * std::mem::size_of::<MetalAtx2TileDescriptor>(),
            "ATX2 Metal descriptors",
        )?;
        let class_buffer = self.new_shared_buffer(
            descriptors.len() * std::mem::size_of::<MetalAtx2TileClass>(),
            "ATX2 Metal classes",
        )?;
        let params_buffer = self.new_shared_buffer(
            std::mem::size_of::<MetalAtx2ClassifyParams>(),
            "ATX2 Metal params",
        )?;

        write_metal_buffer(&source_buffer, &frame.data, "ATX2 Metal source")?;
        write_metal_buffer(
            &descriptor_buffer,
            slice_as_bytes(&descriptors),
            "ATX2 Metal descriptors",
        )?;
        write_metal_buffer(&params_buffer, value_as_bytes(&params), "ATX2 Metal params")?;

        let command_buffer = self
            .queue
            .new_command_buffer()
            .ok_or_else(|| anyhow!("Metal command buffer unavailable"))?;
        let encoder = command_buffer
            .new_compute_command_encoder()
            .ok_or_else(|| anyhow!("Metal compute encoder unavailable"))?;
        encoder.set_compute_pipeline_state(&self.pipeline);
        encoder.set_buffer(&source_buffer, 0, 0);
        encoder.set_buffer(&descriptor_buffer, 0, 1);
        encoder.set_buffer(&class_buffer, 0, 2);
        encoder.set_buffer(&params_buffer, 0, 3);
        encoder.dispatch_threads((descriptors.len(), 1, 1), (64, 1, 1));
        encoder.end_encoding();
        command_buffer.commit();
        command_buffer.wait_until_completed();
        let status = command_buffer.status();
        ensure!(
            status == command_buffer_status::COMPLETED,
            "ATX2 Metal classifier command failed with status {status}: {}",
            command_buffer
                .error()
                .unwrap_or_else(|| "no Metal error message".to_string())
        );

        let metal_classes = read_metal_classes(&class_buffer, descriptors.len())?;
        Ok(metal_classes
            .into_iter()
            .map(|class| Atx2TileClass {
                solid: class.solid != 0,
                bgra: class.bgra,
            })
            .collect())
    }

    fn new_shared_buffer(&self, len: usize, label: &str) -> Result<MetalBuffer> {
        self.device
            .new_buffer(
                len.max(4),
                resource_options::STORAGE_MODE_SHARED | resource_options::CPU_CACHE_MODE_DEFAULT,
            )
            .ok_or_else(|| anyhow!("allocate {label} buffer"))
    }
}

#[cfg(not(test))]
fn write_metal_buffer(buffer: &MetalBuffer, bytes: &[u8], label: &str) -> Result<()> {
    ensure!(
        buffer.write_bytes(bytes) == bytes.len(),
        "failed to write {label} buffer"
    );
    Ok(())
}

#[cfg(not(test))]
fn read_metal_classes(buffer: &MetalBuffer, count: usize) -> Result<Vec<MetalAtx2TileClass>> {
    let byte_len = count
        .checked_mul(std::mem::size_of::<MetalAtx2TileClass>())
        .context("ATX2 Metal class read length overflow")?;
    let Some(contents) = buffer.contents() else {
        return Err(anyhow!("ATX2 Metal class buffer is not CPU visible"));
    };
    let mut classes = vec![MetalAtx2TileClass::default(); count];
    unsafe {
        std::ptr::copy_nonoverlapping(
            contents.cast::<u8>(),
            classes.as_mut_ptr().cast::<u8>(),
            byte_len,
        );
    }
    Ok(classes)
}

#[cfg(not(test))]
fn slice_as_bytes<T>(values: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values))
    }
}

#[cfg(not(test))]
fn value_as_bytes<T>(value: &T) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts((value as *const T).cast::<u8>(), std::mem::size_of::<T>())
    }
}

struct Atx2Command {
    kind: u32,
    atlas_x: u32,
    atlas_y: u32,
    desktop_x: u32,
    desktop_y: u32,
    width: u32,
    height: u32,
    changed_count: u32,
    payload: Vec<u8>,
}

fn build_solid_atx2_stream(width: u32, height: u32, bgra: [u8; 4]) -> Result<Vec<u8>> {
    ensure!(width > 0 && height > 0, "ATX2 frame dimensions are empty");
    ensure!(
        width <= u16::MAX as u32 && height <= u16::MAX as u32,
        "ATX2 frame dimensions exceed wire field range"
    );
    build_atx2_command_stream(
        width,
        height,
        &[Atx2Command {
            kind: ATX2_COMMAND_SOLID_COLOR,
            atlas_x: 0,
            atlas_y: 0,
            desktop_x: 0,
            desktop_y: 0,
            width,
            height,
            changed_count: width.saturating_mul(height),
            payload: bgra.to_vec(),
        }],
    )
}

fn build_exact_atx2_stream_region(
    frame: &CapturedFrame,
    region: DirtyRegion,
    metal_classifier: Option<&mut MacAtx2MetalClassifier>,
) -> Result<Vec<u8>> {
    let DirtyRegion {
        x,
        y,
        width,
        height,
    } = region;
    ensure!(width > 0 && height > 0, "ATX2 frame dimensions are empty");
    ensure!(
        width <= u16::MAX as u32 && height <= u16::MAX as u32,
        "ATX2 frame dimensions exceed wire field range"
    );
    ensure!(
        x.checked_add(width).is_some_and(|end| end <= frame.width)
            && y.checked_add(height).is_some_and(|end| end <= frame.height),
        "ATX2 frame region exceeds source frame"
    );
    ensure!(
        x <= u16::MAX as u32 && y <= u16::MAX as u32,
        "ATX2 frame region exceeds wire field range"
    );

    let tiles = atx2_tiles_for_region(region);
    let metal_classes = metal_classifier
        .and_then(|classifier| classifier.classify_tiles(frame, &tiles).ok())
        .filter(|classes| classes.len() == tiles.len());
    let mut commands = Vec::with_capacity(tiles.len());
    for (index, tile_region) in tiles.iter().copied().enumerate() {
        let tile_x = tile_region.x - x;
        let tile_y = tile_region.y - y;
        let tile_width = tile_region.width;
        let tile_height = tile_region.height;
        if let Some(classes) = metal_classes.as_ref() {
            let class = classes[index];
            if class.solid {
                commands.push(Atx2Command {
                    kind: ATX2_COMMAND_SOLID_COLOR,
                    atlas_x: tile_x,
                    atlas_y: tile_y,
                    desktop_x: x + tile_x,
                    desktop_y: y + tile_y,
                    width: tile_width,
                    height: tile_height,
                    changed_count: tile_width.saturating_mul(tile_height),
                    payload: class.bgra.to_le_bytes().to_vec(),
                });
            } else {
                commands.push(Atx2Command {
                    kind: ATX2_COMMAND_RAW_BGRA,
                    atlas_x: tile_x,
                    atlas_y: tile_y,
                    desktop_x: x + tile_x,
                    desktop_y: y + tile_y,
                    width: tile_width,
                    height: tile_height,
                    changed_count: tile_width.saturating_mul(tile_height),
                    payload: raw_region_payload(frame, tile_region)?,
                });
            }
        } else if let Some(bgra) = solid_color_for_region(frame, tile_region)? {
            commands.push(Atx2Command {
                kind: ATX2_COMMAND_SOLID_COLOR,
                atlas_x: tile_x,
                atlas_y: tile_y,
                desktop_x: tile_region.x,
                desktop_y: tile_region.y,
                width: tile_width,
                height: tile_height,
                changed_count: tile_width.saturating_mul(tile_height),
                payload: bgra.to_vec(),
            });
        } else {
            commands.push(Atx2Command {
                kind: ATX2_COMMAND_RAW_BGRA,
                atlas_x: tile_x,
                atlas_y: tile_y,
                desktop_x: tile_region.x,
                desktop_y: tile_region.y,
                width: tile_width,
                height: tile_height,
                changed_count: tile_width.saturating_mul(tile_height),
                payload: raw_region_payload(frame, tile_region)?,
            });
        }
    }
    build_atx2_command_stream(width, height, &commands)
}

fn atx2_tiles_for_region(region: DirtyRegion) -> Vec<DirtyRegion> {
    let mut tiles = Vec::new();
    let mut tile_y = 0u32;
    while tile_y < region.height {
        let tile_height = ATX2_TILE_SIZE.min(region.height - tile_y);
        let mut tile_x = 0u32;
        while tile_x < region.width {
            let tile_width = ATX2_TILE_SIZE.min(region.width - tile_x);
            tiles.push(DirtyRegion {
                x: region.x + tile_x,
                y: region.y + tile_y,
                width: tile_width,
                height: tile_height,
            });
            tile_x += tile_width;
        }
        tile_y += tile_height;
    }
    tiles
}

fn build_atx2_command_stream(
    atlas_width: u32,
    atlas_height: u32,
    commands: &[Atx2Command],
) -> Result<Vec<u8>> {
    ensure!(!commands.is_empty(), "ATX2 command stream is empty");
    ensure!(
        atlas_width <= u16::MAX as u32 && atlas_height <= u16::MAX as u32,
        "ATX2 atlas dimensions exceed wire field range"
    );
    let payload_bytes = commands
        .iter()
        .try_fold(0usize, |total, command| {
            total.checked_add(command.payload.len())
        })
        .context("ATX2 payload length overflow")?;
    let command_bytes = ATX2_COMMAND_HEADER_BYTES
        .checked_mul(commands.len())
        .and_then(|value| value.checked_add(payload_bytes))
        .context("ATX2 command length overflow")?;
    let byte_len = ATX2_HEADER_BYTES
        .checked_add(command_bytes)
        .context("ATX2 stream length overflow")?;
    ensure!(byte_len <= u32::MAX as usize, "ATX2 stream too large");
    ensure!(
        commands.len() <= u32::MAX as usize,
        "too many ATX2 commands"
    );

    let mut bytes = vec![0u8; byte_len];
    write_atx2_stream_header(
        &mut bytes,
        atlas_width,
        atlas_height,
        commands.len() as u32,
        byte_len as u32,
        commands.len() as u32,
    );
    let mut offset = ATX2_HEADER_BYTES;
    for command in commands {
        ensure!(
            command.atlas_x <= u16::MAX as u32
                && command.atlas_y <= u16::MAX as u32
                && command.desktop_x <= u16::MAX as u32
                && command.desktop_y <= u16::MAX as u32
                && command.width <= u16::MAX as u32
                && command.height <= u16::MAX as u32,
            "ATX2 command exceeds wire field range"
        );
        write_atx2_command_header(
            &mut bytes,
            offset,
            command.kind,
            pack_xy(command.atlas_x, command.atlas_y),
            pack_xy(command.desktop_x, command.desktop_y),
            pack_xy(command.width, command.height),
            command.payload.len() as u32,
            command.changed_count,
        );
        offset += ATX2_COMMAND_HEADER_BYTES;
        bytes[offset..offset + command.payload.len()].copy_from_slice(&command.payload);
        offset += command.payload.len();
    }
    Ok(bytes)
}

fn solid_color_for_region(frame: &CapturedFrame, region: DirtyRegion) -> Result<Option<[u8; 4]>> {
    let first = source_pixel_offset(frame, region.x, region.y)
        .context("ATX2 solid-color first pixel offset overflow")?;
    ensure!(first + 4 <= frame.data.len(), "ATX2 frame row is truncated");
    let color = [
        frame.data[first],
        frame.data[first + 1],
        frame.data[first + 2],
        frame.data[first + 3],
    ];
    for row in 0..region.height {
        let src = source_pixel_offset(frame, region.x, region.y + row)
            .context("ATX2 solid-color row offset overflow")?;
        let row_bytes = region.width as usize * 4;
        let src_end = src + row_bytes;
        ensure!(src_end <= frame.data.len(), "ATX2 frame row is truncated");
        for pixel in frame.data[src..src_end].chunks_exact(4) {
            if pixel != color.as_slice() {
                return Ok(None);
            }
        }
    }
    Ok(Some(color))
}

fn raw_region_payload(frame: &CapturedFrame, region: DirtyRegion) -> Result<Vec<u8>> {
    let row_bytes = region.width as usize * 4;
    let payload_len = row_bytes
        .checked_mul(region.height as usize)
        .context("ATX2 raw payload length overflow")?;
    let mut payload = vec![0u8; payload_len];
    for row in 0..region.height as usize {
        let src_row = region.y as usize + row;
        let src = source_pixel_offset(frame, region.x, src_row as u32)
            .context("ATX2 source row offset overflow")?;
        let dst = row * row_bytes;
        let src_end = src + row_bytes;
        ensure!(src_end <= frame.data.len(), "ATX2 frame row is truncated");
        payload[dst..dst + row_bytes].copy_from_slice(&frame.data[src..src_end]);
    }
    Ok(payload)
}

fn source_pixel_offset(frame: &CapturedFrame, x: u32, y: u32) -> Option<usize> {
    (y as usize)
        .checked_mul(frame.stride as usize)
        .and_then(|value| value.checked_add(x as usize * 4))
}

#[cfg(test)]
fn build_raw_atx2_stream_region(frame: &CapturedFrame, region: DirtyRegion) -> Result<Vec<u8>> {
    let DirtyRegion {
        x,
        y,
        width,
        height,
    } = region;
    ensure!(width > 0 && height > 0, "ATX2 frame dimensions are empty");
    ensure!(
        width <= u16::MAX as u32 && height <= u16::MAX as u32,
        "ATX2 frame dimensions exceed wire field range"
    );
    ensure!(
        x.checked_add(width).is_some_and(|end| end <= frame.width)
            && y.checked_add(height).is_some_and(|end| end <= frame.height),
        "ATX2 frame region exceeds source frame"
    );
    let row_bytes = width as usize * 4;
    let payload_len = row_bytes
        .checked_mul(height as usize)
        .context("ATX2 raw payload length overflow")?;
    let byte_len = ATX2_HEADER_BYTES
        .checked_add(ATX2_COMMAND_HEADER_BYTES)
        .and_then(|value| value.checked_add(payload_len))
        .context("ATX2 stream length overflow")?;
    ensure!(byte_len <= u32::MAX as usize, "ATX2 stream too large");

    let mut bytes = vec![0u8; byte_len];
    write_atx2_stream_header(&mut bytes, width, height, 1, byte_len as u32, 1);
    write_atx2_command_header(
        &mut bytes,
        ATX2_HEADER_BYTES,
        ATX2_COMMAND_RAW_BGRA,
        pack_xy(0, 0),
        pack_xy(x, y),
        pack_xy(width, height),
        payload_len as u32,
        width.saturating_mul(height),
    );

    let payload_offset = ATX2_HEADER_BYTES + ATX2_COMMAND_HEADER_BYTES;
    for row in 0..height as usize {
        let src_row = y as usize + row;
        let src = src_row
            .checked_mul(frame.stride as usize)
            .and_then(|value| value.checked_add(x as usize * 4))
            .context("ATX2 source row offset overflow")?;
        let dst = payload_offset + row * row_bytes;
        let src_end = src + row_bytes;
        ensure!(src_end <= frame.data.len(), "ATX2 frame row is truncated");
        bytes[dst..dst + row_bytes].copy_from_slice(&frame.data[src..src_end]);
    }
    Ok(bytes)
}

fn write_atx2_stream_header(
    bytes: &mut [u8],
    atlas_width: u32,
    atlas_height: u32,
    command_count: u32,
    byte_len: u32,
    descriptor_count: u32,
) {
    write_u32_le(bytes, 0, ATX2_STREAM_MAGIC);
    write_u32_le(bytes, 4, ATX2_STREAM_VERSION);
    write_u32_le(bytes, 8, pack_xy(atlas_width, atlas_height));
    write_u32_le(
        bytes,
        12,
        pack_xy(ATX2_TILE_SIZE, atlas_width.div_ceil(ATX2_TILE_SIZE).max(1)),
    );
    write_u32_le(bytes, 16, command_count);
    write_u32_le(bytes, 20, byte_len);
    write_u32_le(bytes, 24, descriptor_count);
    write_u32_le(bytes, 28, 0);
}

fn write_atx2_command_header(
    bytes: &mut [u8],
    offset: usize,
    kind: u32,
    atlas_xy: u32,
    desktop_xy: u32,
    wh: u32,
    payload_len: u32,
    changed_count: u32,
) {
    write_u32_le(bytes, offset, kind);
    write_u32_le(bytes, offset + 4, atlas_xy);
    write_u32_le(bytes, offset + 8, desktop_xy);
    write_u32_le(bytes, offset + 12, wh);
    write_u32_le(bytes, offset + 16, payload_len);
    write_u32_le(bytes, offset + 20, changed_count);
}

fn write_u32_le(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn pack_xy(x: u32, y: u32) -> u32 {
    (x & 0xffff) | ((y & 0xffff) << 16)
}

fn write_chunk(stream: &mut UnixStream, tag: u8, payload: &[u8]) -> Result<()> {
    stream.write_all(&[tag])?;
    stream.write_all(&(payload.len() as u32).to_le_bytes())?;
    stream.write_all(payload)?;
    Ok(())
}

fn write_capture_error(stream: &mut UnixStream, reason: &str, message: &str) -> Result<()> {
    let payload = serde_json::to_vec(&json!({
        "reason": reason,
        "message": message,
    }))
    .context("serialize macOS capture error")?;
    write_chunk(stream, 7, &payload)
}

fn write_handshake(stream: &mut UnixStream, auth_token: &str) -> Result<()> {
    ensure!(auth_token.len() <= u16::MAX as usize, "auth token too long");
    stream.write_all(&HELPER_PIPE_HANDSHAKE_MAGIC)?;
    stream.write_all(&HELPER_PIPE_PROTOCOL_VERSION.to_be_bytes())?;
    stream.write_all(&(auth_token.len() as u16).to_be_bytes())?;
    stream.write_all(auth_token.as_bytes())?;
    Ok(())
}

fn control_loop(
    mut socket: UnixStream,
    stop: Arc<AtomicBool>,
    display_geometry: Arc<std::sync::Mutex<ActiveDisplayGeometry>>,
    control_tx: mpsc::Sender<ControlCommand>,
) -> Result<()> {
    let mut mouse_buttons = MouseButtonState::default();
    while !stop.load(Ordering::Relaxed) {
        let mut len_buf = [0u8; 2];
        socket.read_exact(&mut len_buf)?;
        let len = u16::from_be_bytes(len_buf) as usize;
        let mut type_buf = [0u8; 1];
        socket.read_exact(&mut type_buf)?;
        let mut payload = vec![0u8; len];
        if len > 0 {
            socket.read_exact(&mut payload)?;
        }
        if type_buf[0] == CONTROL_TYPE_STOP_CAPTURE {
            stop.store(true, Ordering::SeqCst);
            break;
        }
        if type_buf[0] == CONTROL_TYPE_CAPTURE_OUTPUT_SWITCH {
            if payload.len() != CONTROL_PAYLOAD_CAPTURE_OUTPUT_INDEX_LEN {
                warn!(
                    payload_len = payload.len(),
                    "macOS helper ignored bad capture output switch payload"
                );
                continue;
            }
            let index = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let _ = control_tx.send(ControlCommand::SwitchCaptureOutput(index));
            continue;
        }
        if type_buf[0] == CONTROL_TYPE_STREAM_BITRATE {
            if payload.len() != CONTROL_PAYLOAD_STREAM_BITRATE_LEN {
                warn!(
                    payload_len = payload.len(),
                    "macOS helper ignored bad stream bitrate payload"
                );
                continue;
            }
            let kbps = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let _ = control_tx.send(ControlCommand::StreamBitrate(kbps));
            continue;
        }
        if matches!(
            type_buf[0],
            CONTROL_TYPE_SESSION_SWITCH
                | CONTROL_TYPE_SESSION_LOGOFF
                | CONTROL_TYPE_SECURE_ATTENTION
        ) {
            info!(
                message_type = type_buf[0],
                "macOS helper ignored Windows-only control"
            );
            continue;
        }
        let geometry = *display_geometry.lock().unwrap();
        if let Err(err) = inject_input(type_buf[0], &payload, geometry, &mut mouse_buttons) {
            warn!(message_type = type_buf[0], error = %err, "macOS input injection failed");
        }
    }
    Ok(())
}

fn inject_input(
    message_type: u8,
    payload: &[u8],
    geometry: ActiveDisplayGeometry,
    mouse_buttons: &mut MouseButtonState,
) -> Result<()> {
    ensure_accessibility_trusted()?;
    match message_type {
        CONTROL_TYPE_MOUSE_MOVE => {
            ensure!(
                payload.len() == CONTROL_PAYLOAD_MOUSE_MOVE_LEN,
                "bad mouse move payload"
            );
            let (x, y) = parse_xy(payload);
            let (event_type, button) = mouse_buttons.move_event();
            post_mouse_event(event_type, normalized_point(x, y, geometry), button);
        }
        CONTROL_TYPE_MOUSE_BUTTON => {
            ensure!(
                payload.len() == CONTROL_PAYLOAD_MOUSE_BUTTON_LEN,
                "bad mouse button payload"
            );
            let button = payload[0];
            let down = payload[1] != 0;
            let (x, y) = parse_xy(&payload[2..]);
            let Some(event_type) = mouse_button_event_type(button, down) else {
                return Ok(());
            };
            post_mouse_event_with_click_state(
                event_type,
                normalized_point(x, y, geometry),
                button as u32,
                1,
            );
            mouse_buttons.apply_button(button, down);
        }
        CONTROL_TYPE_MOUSE_DOUBLE_CLICK => {
            ensure!(
                payload.len() == CONTROL_PAYLOAD_MOUSE_DOUBLE_CLICK_LEN,
                "bad mouse double-click payload"
            );
            let button = payload[0];
            let (x, y) = parse_xy(&payload[1..]);
            post_mouse_double_click(button, normalized_point(x, y, geometry));
            mouse_buttons.apply_button(button, false);
        }
        CONTROL_TYPE_MOUSE_WHEEL => {
            ensure!(
                payload.len() == CONTROL_PAYLOAD_MOUSE_WHEEL_LEN,
                "bad mouse wheel payload"
            );
            let delta = i16::from_be_bytes([payload[0], payload[1]]) as i32;
            let (x, y) = parse_xy(&payload[2..]);
            post_mouse_event(
                CG_EVENT_MOUSE_MOVED,
                normalized_point(x, y, geometry),
                CG_MOUSE_BUTTON_LEFT as u32,
            );
            post_scroll_event(delta);
        }
        CONTROL_TYPE_KEY_DOWN | CONTROL_TYPE_KEY_UP => {
            ensure!(payload.len() == CONTROL_PAYLOAD_KEY_LEN, "bad key payload");
            let vkey = u16::from_be_bytes([payload[0], payload[1]]);
            let modifiers = payload[4];
            let keycode = windows_vkey_to_macos_keycode(vkey);
            let is_down = message_type == CONTROL_TYPE_KEY_DOWN;
            if windows_vkey_is_modifier(vkey) {
                if let Some(keycode) = keycode {
                    post_key_event(keycode, is_down);
                }
            } else if is_down {
                if let Some(keycode) = keycode {
                    post_key_event_with_flags(
                        keycode,
                        true,
                        key_event_flags_for_vkey(vkey, modifiers),
                    );
                }
            } else {
                if let Some(keycode) = keycode {
                    post_key_event_with_flags(
                        keycode,
                        false,
                        key_event_flags_for_vkey(vkey, modifiers),
                    );
                }
            }
        }
        CONTROL_TYPE_TYPED_INPUT => {
            let text = parse_text_payload(payload)?;
            for unit in text.encode_utf16() {
                post_unicode_unit(unit);
            }
        }
        CONTROL_TYPE_CLIPBOARD => {
            let text = parse_text_payload(payload)?;
            set_clipboard_text(text)?;
            post_key_combo_command_v();
        }
        other => debug!(
            message_type = other,
            "macOS helper ignored unknown control message"
        ),
    }
    Ok(())
}

fn mouse_button_event_type(button: u8, down: bool) -> Option<u32> {
    match (button, down) {
        (CG_MOUSE_BUTTON_LEFT, true) => Some(CG_EVENT_LEFT_MOUSE_DOWN),
        (CG_MOUSE_BUTTON_LEFT, false) => Some(CG_EVENT_LEFT_MOUSE_UP),
        (CG_MOUSE_BUTTON_RIGHT, true) => Some(CG_EVENT_RIGHT_MOUSE_DOWN),
        (CG_MOUSE_BUTTON_RIGHT, false) => Some(CG_EVENT_RIGHT_MOUSE_UP),
        (CG_MOUSE_BUTTON_OTHER, true) => Some(CG_EVENT_OTHER_MOUSE_DOWN),
        (CG_MOUSE_BUTTON_OTHER, false) => Some(CG_EVENT_OTHER_MOUSE_UP),
        _ => None,
    }
}

fn parse_xy(payload: &[u8]) -> (u32, u32) {
    (
        u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]),
        u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]),
    )
}

fn parse_text_payload(payload: &[u8]) -> Result<&str> {
    ensure!(payload.len() >= 2, "bad text payload");
    let text_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    ensure!(payload.len() == 2 + text_len, "bad text payload length");
    std::str::from_utf8(&payload[2..]).context("text payload utf8")
}

fn normalized_point(x: u32, y: u32, geometry: ActiveDisplayGeometry) -> CGPoint {
    let max = 65_535.0;
    let local_x = (x.min(65_535) as f64 / max) * (geometry.frame_width - 1.0).max(1.0);
    let local_y = (y.min(65_535) as f64 / max) * (geometry.frame_height - 1.0).max(1.0);
    let px = geometry.frame_x + local_x;
    let py = geometry.frame_y + local_y;
    CGPoint { x: px, y: py }
}

fn even_dimension(value: u32) -> u32 {
    value & !1
}

fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(talos_protocol::rmm_tracing_filter_directive())
        .with_target(true)
        .try_init();
}

fn ensure_permissions() {
    if !accessibility_trusted() {
        warn!("Accessibility permission is not trusted; mouse/keyboard control will fail until talos_worker_helper is allowed in Privacy & Security > Accessibility");
    }
}

fn ensure_accessibility_trusted() -> Result<()> {
    if accessibility_trusted() {
        Ok(())
    } else {
        Err(anyhow!("Accessibility permission denied; allow talos_worker_helper in Privacy & Security > Accessibility"))
    }
}

fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) }
}

fn screen_recording_trusted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

#[derive(Default)]
struct Options {
    stream_socket: String,
    control_socket: String,
    auth_token: String,
    fps: u32,
}

impl Options {
    fn parse() -> Result<Self> {
        Self::parse_from(std::env::args().skip(2))
    }

    fn parse_from<I>(args: I) -> Result<Self>
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        let mut options = Options {
            fps: 30,
            ..Default::default()
        };
        let mut args = args.into_iter().map(Into::into);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--stream-socket" => options.stream_socket = args.next().unwrap_or_default(),
                "--control-socket" => options.control_socket = args.next().unwrap_or_default(),
                "--auth-token" => options.auth_token = args.next().unwrap_or_default(),
                "--fps" => {
                    options.fps = args
                        .next()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(30)
                }
                "--session-id" => {
                    let _ = args.next();
                }
                _ => {}
            }
        }
        options.fps = clamp_capture_fps(options.fps);
        ensure!(!options.stream_socket.is_empty(), "missing --stream-socket");
        ensure!(
            !options.control_socket.is_empty(),
            "missing --control-socket"
        );
        ensure!(!options.auth_token.is_empty(), "missing --auth-token");
        Ok(options)
    }
}

fn clamp_capture_fps(fps: u32) -> u32 {
    fps.clamp(MIN_CAPTURE_FPS, MAX_CAPTURE_FPS)
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGEventCreateMouseEvent(
        source: *const c_void,
        mouse_type: u32,
        mouse_cursor_position: CGPoint,
        mouse_button: u32,
    ) -> *mut c_void;
    fn CGEventCreateKeyboardEvent(
        source: *const c_void,
        virtual_key: u16,
        key_down: bool,
    ) -> *mut c_void;
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
    fn CGEventSetIntegerValueField(event: *mut c_void, field: u32, value: i64);
    fn CGEventKeyboardSetUnicodeString(
        event: *mut c_void,
        string_length: usize,
        unicode_string: *const u16,
    );
    fn CGEventCreateScrollWheelEvent(
        source: *const c_void,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
    ) -> *mut c_void;
    fn CGEventPost(tap: u32, event: *mut c_void);
    fn CFRelease(cf: *mut c_void);
}

fn post_mouse_event(event_type: u32, point: CGPoint, button: u32) {
    unsafe {
        let event = CGEventCreateMouseEvent(std::ptr::null(), event_type, point, button);
        if !event.is_null() {
            CGEventPost(0, event);
            CFRelease(event);
        }
    }
}

fn post_mouse_event_with_click_state(
    event_type: u32,
    point: CGPoint,
    button: u32,
    click_state: i64,
) {
    unsafe {
        let event = CGEventCreateMouseEvent(std::ptr::null(), event_type, point, button);
        if !event.is_null() {
            CGEventSetIntegerValueField(event, CG_MOUSE_EVENT_CLICK_STATE, click_state);
            CGEventPost(0, event);
            CFRelease(event);
        }
    }
}

fn post_mouse_double_click(button: u8, point: CGPoint) {
    let Some((down_event, up_event)) = mouse_double_click_event_types(button) else {
        return;
    };
    let button = button as u32;
    post_mouse_event_with_click_state(down_event, point, button, 1);
    std::thread::sleep(Duration::from_millis(20));
    post_mouse_event_with_click_state(up_event, point, button, 1);
    std::thread::sleep(Duration::from_millis(35));
    post_mouse_event_with_click_state(down_event, point, button, 2);
    std::thread::sleep(Duration::from_millis(20));
    post_mouse_event_with_click_state(up_event, point, button, 2);
}

fn mouse_double_click_event_types(button: u8) -> Option<(u32, u32)> {
    Some((
        mouse_button_event_type(button, true)?,
        mouse_button_event_type(button, false)?,
    ))
}

fn post_scroll_event(delta: i32) {
    let lines = windows_wheel_delta_to_macos_lines(delta);
    if lines == 0 {
        return;
    }
    unsafe {
        let event =
            CGEventCreateScrollWheelEvent(std::ptr::null(), CG_SCROLL_EVENT_UNIT_LINE, 1, lines);
        if !event.is_null() {
            CGEventPost(0, event);
            CFRelease(event);
        }
    }
}

fn windows_wheel_delta_to_macos_lines(delta: i32) -> i32 {
    if delta == 0 {
        return 0;
    }
    let lines = delta / WINDOWS_WHEEL_DELTA;
    let protocol_lines = if lines == 0 { delta.signum() } else { lines };
    -protocol_lines
}

fn post_key_event(keycode: u16, down: bool) {
    post_key_event_with_flags(keycode, down, 0);
}

fn post_key_event_with_flags(keycode: u16, down: bool, flags: u64) {
    unsafe {
        let event = CGEventCreateKeyboardEvent(std::ptr::null(), keycode, down);
        if !event.is_null() {
            if flags != 0 {
                CGEventSetFlags(event, flags);
            }
            CGEventPost(0, event);
            CFRelease(event);
        }
    }
}

fn key_event_flags_for_vkey(vkey: u16, modifiers: u8) -> u64 {
    if windows_vkey_is_modifier(vkey) {
        0
    } else {
        cg_event_flags_for_modifiers(modifiers)
    }
}

fn post_unicode_unit(unit: u16) {
    unsafe {
        let event = CGEventCreateKeyboardEvent(std::ptr::null(), 0, true);
        if !event.is_null() {
            CGEventKeyboardSetUnicodeString(event, 1, &unit);
            CGEventPost(0, event);
            CFRelease(event);
        }
        let event = CGEventCreateKeyboardEvent(std::ptr::null(), 0, false);
        if !event.is_null() {
            CGEventKeyboardSetUnicodeString(event, 1, &unit);
            CGEventPost(0, event);
            CFRelease(event);
        }
    }
}

fn set_clipboard_text(text: &str) -> Result<()> {
    let mut child = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .context("spawn pbcopy")?;
    {
        let stdin = child.stdin.as_mut().context("open pbcopy stdin")?;
        stdin
            .write_all(text.as_bytes())
            .context("write clipboard text to pbcopy")?;
    }
    let status = child.wait().context("wait for pbcopy")?;
    ensure!(status.success(), "pbcopy exited with {status}");
    Ok(())
}

fn post_key_combo_command_v() {
    post_key_event(55, true);
    post_key_event_with_flags(9, true, CG_EVENT_FLAG_MASK_COMMAND);
    post_key_event_with_flags(9, false, CG_EVENT_FLAG_MASK_COMMAND);
    post_key_event(55, false);
}

const CG_EVENT_FLAG_MASK_SHIFT: u64 = 1 << 17;
const CG_EVENT_FLAG_MASK_CONTROL: u64 = 1 << 18;
const CG_EVENT_FLAG_MASK_ALTERNATE: u64 = 1 << 19;
const CG_EVENT_FLAG_MASK_COMMAND: u64 = 1 << 20;

fn cg_event_flags_for_modifiers(modifiers: u8) -> u64 {
    let mut flags = 0u64;
    if modifiers & CONTROL_MOD_SHIFT != 0 {
        flags |= CG_EVENT_FLAG_MASK_SHIFT;
    }
    if modifiers & CONTROL_MOD_CTRL != 0 {
        flags |= CG_EVENT_FLAG_MASK_CONTROL;
    }
    if modifiers & CONTROL_MOD_ALT != 0 {
        flags |= CG_EVENT_FLAG_MASK_ALTERNATE;
    }
    if modifiers & CONTROL_MOD_WIN != 0 {
        flags |= CG_EVENT_FLAG_MASK_COMMAND;
    }
    flags
}

fn windows_vkey_to_macos_keycode(vkey: u16) -> Option<u16> {
    Some(match vkey {
        0x08 => 51,
        0x09 => 48,
        0x0D => 36,
        0x14 => 57,
        0x10 => 56,
        0x11 => 59,
        0x12 => 58,
        0x1B => 53,
        0x20 => 49,
        0x21 => 116,
        0x22 => 121,
        0x23 => 119,
        0x24 => 115,
        0x25 => 123,
        0x26 => 126,
        0x27 => 124,
        0x28 => 125,
        0x2D => 114,
        0x2E => 117,
        0x30 => 29,
        0x31 => 18,
        0x32 => 19,
        0x33 => 20,
        0x34 => 21,
        0x35 => 23,
        0x36 => 22,
        0x37 => 26,
        0x38 => 28,
        0x39 => 25,
        0x41 => 0,
        0x42 => 11,
        0x43 => 8,
        0x44 => 2,
        0x45 => 14,
        0x46 => 3,
        0x47 => 5,
        0x48 => 4,
        0x49 => 34,
        0x4A => 38,
        0x4B => 40,
        0x4C => 37,
        0x4D => 46,
        0x4E => 45,
        0x4F => 31,
        0x50 => 35,
        0x51 => 12,
        0x52 => 15,
        0x53 => 1,
        0x54 => 17,
        0x55 => 32,
        0x56 => 9,
        0x57 => 13,
        0x58 => 7,
        0x59 => 16,
        0x5A => 6,
        0x5B => 55,
        0x5C => 54,
        0x60 => 82,
        0x61 => 83,
        0x62 => 84,
        0x63 => 85,
        0x64 => 86,
        0x65 => 87,
        0x66 => 88,
        0x67 => 89,
        0x68 => 91,
        0x69 => 92,
        0x6A => 67,
        0x6B => 69,
        0x6C => 71,
        0x6D => 78,
        0x6E => 65,
        0x6F => 75,
        0x70 => 122,
        0x71 => 120,
        0x72 => 99,
        0x73 => 118,
        0x74 => 96,
        0x75 => 97,
        0x76 => 98,
        0x77 => 100,
        0x78 => 101,
        0x79 => 109,
        0x7A => 103,
        0x7B => 111,
        0x7C => 105,
        0x7D => 107,
        0x7E => 113,
        0x7F => 106,
        0x80 => 64,
        0x81 => 79,
        0x82 => 80,
        0xA0 => 56,
        0xA1 => 60,
        0xA2 => 59,
        0xA3 => 62,
        0xA4 => 58,
        0xA5 => 61,
        0xBA => 41,
        0xBB => 24,
        0xBC => 43,
        0xBD => 27,
        0xBE => 47,
        0xBF => 44,
        0xC0 => 50,
        0xDB => 33,
        0xDC => 42,
        0xDD => 30,
        0xDE => 39,
        _ => return None,
    })
}

fn windows_vkey_is_modifier(vkey: u16) -> bool {
    matches!(
        vkey,
        0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0 | 0xA1 | 0xA2 | 0xA3 | 0xA4 | 0xA5
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use talos_protocol::{decode_display_record, DisplayRecord};

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    fn metadata_json(bytes: &[u8]) -> serde_json::Value {
        assert_eq!(&bytes[..4], b"RMMD");
        let len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        serde_json::from_slice(&bytes[8..8 + len]).expect("metadata json")
    }

    #[test]
    fn cursor_capture_flag_is_detected_from_helper_args() {
        assert!(capture_hides_cursor_from_args([
            "talos_worker_helper",
            "capture-macos-legacy",
            "--hide-cursor",
        ]));
        assert!(capture_hides_cursor_from_args([
            "talos_worker_helper",
            "capture-macos-legacy",
            "--HIDE-CURSOR",
        ]));
        assert!(!capture_hides_cursor_from_args([
            "talos_worker_helper",
            "capture-macos-legacy",
        ]));
    }

    fn sample_outputs() -> Vec<CaptureOutputInfo> {
        vec![
            CaptureOutputInfo {
                index: 0,
                display_id: 11,
                name: "Main Display".to_string(),
                width: 1920,
                height: 1080,
                origin_x: 0.0,
                origin_y: 0.0,
                point_width: 1920.0,
                point_height: 1080.0,
                primary: true,
            },
            CaptureOutputInfo {
                index: 1,
                display_id: 22,
                name: "Display 2 (1280x720)".to_string(),
                width: 1280,
                height: 720,
                origin_x: 1920.0,
                origin_y: 0.0,
                point_width: 1280.0,
                point_height: 720.0,
                primary: false,
            },
        ]
    }

    #[test]
    fn metadata_reports_active_macos_capture_output_list() {
        let tuning = talos_worker::encode::load_encode_tuning_from_env();
        let outputs = sample_outputs();
        let metadata = build_metadata(1280, 720, 30, tuning, 1, &outputs);
        let json = metadata_json(&metadata);

        assert_eq!(json["activeIndex"], 1);
        assert_eq!(json["captureOutputs"].as_array().unwrap().len(), 2);
        assert_eq!(json["captureOutputs"][0]["displayId"], 11);
        assert_eq!(json["captureOutputs"][1]["name"], "Display 2 (1280x720)");
        assert_eq!(json["captureOutputs"][1]["originX"], 1920.0);
        assert_eq!(json["captureOutputs"][1]["pointWidth"], 1280.0);
        assert_eq!(json["captureOutputs"][1]["primary"], false);
    }

    #[test]
    fn atx2_metadata_reports_active_macos_capture_output_list() {
        let tuning = talos_worker::encode::load_encode_tuning_from_env();
        let outputs = sample_outputs();
        let metadata =
            build_atx2_metadata(1280, 720, 30, tuning, 1, &outputs).expect("atx2 metadata");
        let json = metadata_json(&metadata);

        assert_eq!(json["activeIndex"], 1);
        assert_eq!(json["captureOutputs"].as_array().unwrap().len(), 2);
        assert_eq!(json["captureOutputs"][1]["displayId"], 22);
        assert_eq!(json["captureOutputs"][1]["originX"], 1920.0);
        assert_eq!(json["experimental"]["tileCommandFormat"], "ATX2");
        assert_eq!(json["experimental"]["dirtyRectSource"], "ScreenCaptureKit");
        assert_eq!(json["experimental"]["tileSize"], ATX2_TILE_SIZE);
        assert!(json.get(DISPLAY_STREAM_META_TYPE).is_some());
    }

    #[test]
    fn h264_metadata_reports_modern_capture_descriptor() {
        let tuning = talos_worker::encode::load_encode_tuning_from_env();
        let outputs = sample_outputs();
        let metadata =
            build_h264_metadata(1280, 720, 30, tuning, 1, &outputs).expect("h264 metadata");
        let json = metadata_json(&metadata);

        assert_eq!(json["captureType"], "macos_screencapturekit_h264");
        assert_eq!(json["activeIndex"], 1);
        assert_eq!(json["captureOutputs"].as_array().unwrap().len(), 2);
        assert_eq!(json[DISPLAY_STREAM_META_TYPE]["mode"], "modern_capture");
        assert_eq!(json[DISPLAY_STREAM_META_TYPE]["pixelFormat"], "h264");
        assert_eq!(json[DISPLAY_STREAM_META_TYPE]["compression"], "annex_b");
    }

    #[test]
    fn screenshot_metadata_reports_screenshot_only_descriptor() {
        let tuning = talos_worker::encode::load_encode_tuning_from_env();
        let outputs = sample_outputs();
        let metadata = build_screenshot_metadata(1280, 720, 30, tuning, 1, &outputs)
            .expect("screenshot metadata");
        let json = metadata_json(&metadata);

        assert_eq!(json["captureType"], "macos_screencapturekit_screenshot");
        assert_eq!(json["activeIndex"], 1);
        assert_eq!(json["captureOutputs"].as_array().unwrap().len(), 2);
        assert_eq!(json[DISPLAY_STREAM_META_TYPE]["mode"], "screenshot_only");
        assert_eq!(json[DISPLAY_STREAM_META_TYPE]["pixelFormat"], "bgra8");
        assert_eq!(json[DISPLAY_STREAM_META_TYPE]["compression"], "none");
    }

    #[test]
    fn visible_bgra_bytes_removes_stride_padding() {
        let frame = CapturedFrame {
            width: 2,
            height: 2,
            stride: 12,
            data: vec![
                1, 2, 3, 4, 5, 6, 7, 8, 99, 99, 99, 99, 9, 10, 11, 12, 13, 14, 15, 16, 88, 88, 88,
                88,
            ],
        };

        let bgra = visible_bgra_bytes(&frame, 2, 2).expect("visible bgra bytes");

        assert_eq!(
            bgra,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
    }

    #[test]
    fn normalized_mouse_points_use_active_display_point_frame() {
        let geometry = ActiveDisplayGeometry {
            capture_width: 3456,
            capture_height: 2234,
            frame_x: 1920.0,
            frame_y: -100.0,
            frame_width: 1728.0,
            frame_height: 1117.0,
        };

        let top_left = normalized_point(0, 0, geometry);
        assert_eq!(top_left.x, 1920.0);
        assert_eq!(top_left.y, -100.0);

        let bottom_right = normalized_point(65_535, 65_535, geometry);
        assert_eq!(bottom_right.x, 3647.0);
        assert_eq!(bottom_right.y, 1016.0);
    }

    #[test]
    fn macos_mouse_moves_become_drag_events_while_button_pressed() {
        let mut state = MouseButtonState::default();

        assert_eq!(
            state.move_event(),
            (CG_EVENT_MOUSE_MOVED, CG_MOUSE_BUTTON_LEFT as u32)
        );

        state.apply_button(CG_MOUSE_BUTTON_LEFT, true);
        assert_eq!(
            state.move_event(),
            (CG_EVENT_LEFT_MOUSE_DRAGGED, CG_MOUSE_BUTTON_LEFT as u32)
        );

        state.apply_button(CG_MOUSE_BUTTON_RIGHT, false);
        assert_eq!(
            state.move_event(),
            (CG_EVENT_LEFT_MOUSE_DRAGGED, CG_MOUSE_BUTTON_LEFT as u32)
        );

        state.apply_button(CG_MOUSE_BUTTON_LEFT, false);
        assert_eq!(
            state.move_event(),
            (CG_EVENT_MOUSE_MOVED, CG_MOUSE_BUTTON_LEFT as u32)
        );
    }

    #[test]
    fn macos_mouse_drag_events_track_right_and_middle_buttons() {
        let mut state = MouseButtonState::default();

        state.apply_button(CG_MOUSE_BUTTON_RIGHT, true);
        assert_eq!(
            state.move_event(),
            (CG_EVENT_RIGHT_MOUSE_DRAGGED, CG_MOUSE_BUTTON_RIGHT as u32)
        );

        state.apply_button(CG_MOUSE_BUTTON_RIGHT, false);
        state.apply_button(CG_MOUSE_BUTTON_OTHER, true);
        assert_eq!(
            state.move_event(),
            (CG_EVENT_OTHER_MOUSE_DRAGGED, CG_MOUSE_BUTTON_OTHER as u32)
        );
    }

    #[test]
    fn macos_mouse_button_events_use_coregraphics_types() {
        assert_eq!(
            mouse_button_event_type(CG_MOUSE_BUTTON_LEFT, true),
            Some(CG_EVENT_LEFT_MOUSE_DOWN)
        );
        assert_eq!(
            mouse_button_event_type(CG_MOUSE_BUTTON_LEFT, false),
            Some(CG_EVENT_LEFT_MOUSE_UP)
        );
        assert_eq!(
            mouse_button_event_type(CG_MOUSE_BUTTON_RIGHT, true),
            Some(CG_EVENT_RIGHT_MOUSE_DOWN)
        );
        assert_eq!(
            mouse_button_event_type(CG_MOUSE_BUTTON_RIGHT, false),
            Some(CG_EVENT_RIGHT_MOUSE_UP)
        );
        assert_eq!(
            mouse_button_event_type(CG_MOUSE_BUTTON_OTHER, true),
            Some(CG_EVENT_OTHER_MOUSE_DOWN)
        );
        assert_eq!(
            mouse_button_event_type(CG_MOUSE_BUTTON_OTHER, false),
            Some(CG_EVENT_OTHER_MOUSE_UP)
        );
        assert_eq!(mouse_button_event_type(3, true), None);
    }

    #[test]
    fn macos_mouse_double_click_events_reuse_button_down_up_types() {
        assert_eq!(
            mouse_double_click_event_types(CG_MOUSE_BUTTON_LEFT),
            Some((CG_EVENT_LEFT_MOUSE_DOWN, CG_EVENT_LEFT_MOUSE_UP))
        );
        assert_eq!(
            mouse_double_click_event_types(CG_MOUSE_BUTTON_RIGHT),
            Some((CG_EVENT_RIGHT_MOUSE_DOWN, CG_EVENT_RIGHT_MOUSE_UP))
        );
        assert_eq!(
            mouse_double_click_event_types(CG_MOUSE_BUTTON_OTHER),
            Some((CG_EVENT_OTHER_MOUSE_DOWN, CG_EVENT_OTHER_MOUSE_UP))
        );
        assert_eq!(mouse_double_click_event_types(3), None);
    }

    #[test]
    fn stream_bitrate_update_overrides_encode_tuning() {
        let mut tuning = talos_worker::encode::load_encode_tuning_from_env();
        tuning.bitrate_override_kbps = None;

        assert!(apply_stream_bitrate_update(&mut tuning, 8_000));
        assert_eq!(tuning.bitrate_override_kbps, Some(8_000));
        assert_eq!(tuning.bitrate_kbps(), 8_000);
        assert!(!apply_stream_bitrate_update(&mut tuning, 8_000));
        assert!(!apply_stream_bitrate_update(&mut tuning, 0));
        assert_eq!(tuning.bitrate_override_kbps, Some(8_000));
    }

    #[test]
    fn legacy_ivf_transition_initializes_before_header() {
        assert_eq!(
            legacy_ivf_transition(None, 1280, 720),
            LegacyIvfTransition::InitializeStream
        );
    }

    #[test]
    fn legacy_ivf_transition_continues_same_dimensions_without_header_reset() {
        assert_eq!(
            legacy_ivf_transition(Some((1280, 720)), 1280, 720),
            LegacyIvfTransition::ContinueStream
        );
    }

    #[test]
    fn legacy_ivf_transition_rejects_dimension_change_after_header() {
        assert_eq!(
            legacy_ivf_transition(Some((1280, 720)), 1920, 1080),
            LegacyIvfTransition::RejectDimensionChange {
                current_width: 1280,
                current_height: 720,
                next_width: 1920,
                next_height: 1080,
            }
        );
    }

    #[test]
    fn legacy_metadata_update_uses_rmmd_not_dkif() {
        let tuning = talos_worker::encode::load_encode_tuning_from_env();
        let metadata = build_metadata(1280, 720, 30, tuning, 0, &sample_outputs());

        assert_eq!(&metadata[..4], b"RMMD");
        assert_ne!(&metadata[..4], b"DKIF");
    }

    #[test]
    fn helper_options_clamp_capture_fps() {
        let low = Options::parse_from([
            "--stream-socket",
            "/tmp/stream.sock",
            "--control-socket",
            "/tmp/control.sock",
            "--auth-token",
            "token",
            "--fps",
            "0",
        ])
        .expect("low fps options");
        assert_eq!(low.fps, MIN_CAPTURE_FPS);

        let high = Options::parse_from([
            "--stream-socket",
            "/tmp/stream.sock",
            "--control-socket",
            "/tmp/control.sock",
            "--auth-token",
            "token",
            "--fps",
            "1000",
        ])
        .expect("high fps options");
        assert_eq!(high.fps, MAX_CAPTURE_FPS);
    }

    #[test]
    fn control_loop_ignores_bad_helper_control_payloads() {
        fn write_control_frame(socket: &mut UnixStream, message_type: u8, payload: &[u8]) {
            socket
                .write_all(&(payload.len() as u16).to_be_bytes())
                .expect("write frame length");
            socket.write_all(&[message_type]).expect("write frame type");
            socket.write_all(payload).expect("write frame payload");
        }

        let (server, mut client) = UnixStream::pair().expect("create socket pair");
        let stop = Arc::new(AtomicBool::new(false));
        let display_geometry = Arc::new(std::sync::Mutex::new(ActiveDisplayGeometry::default()));
        let (control_tx, control_rx) = mpsc::channel();
        let stop_for_loop = stop.clone();
        let handle = std::thread::spawn(move || {
            control_loop(server, stop_for_loop, display_geometry, control_tx)
        });

        write_control_frame(&mut client, CONTROL_TYPE_CAPTURE_OUTPUT_SWITCH, &[1, 2]);
        write_control_frame(&mut client, CONTROL_TYPE_STREAM_BITRATE, &[1, 2, 3]);
        write_control_frame(
            &mut client,
            CONTROL_TYPE_STREAM_BITRATE,
            &8_000u32.to_be_bytes(),
        );
        write_control_frame(&mut client, CONTROL_TYPE_STOP_CAPTURE, &[]);

        let command = control_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("valid control frame should be delivered");
        match command {
            ControlCommand::StreamBitrate(kbps) => assert_eq!(kbps, 8_000),
            ControlCommand::SwitchCaptureOutput(index) => {
                panic!("unexpected capture output switch command: {index}")
            }
        }
        assert!(control_rx.recv_timeout(Duration::from_millis(100)).is_err());
        assert!(handle.join().expect("control thread join").is_ok());
        assert!(stop.load(Ordering::SeqCst));
    }

    #[test]
    fn latest_frame_from_queue_keeps_first_when_no_newer_frame() {
        let (_tx, rx) = mpsc::channel();
        let frame = CapturedFrame {
            width: 2,
            height: 2,
            stride: 8,
            data: vec![1; 16],
        };

        let latest = latest_frame_from_queue(frame, &rx, None);

        assert_eq!(latest.data[0], 1);
    }

    #[test]
    fn latest_frame_from_queue_drops_stale_frames_for_newest() {
        let (tx, rx) = mpsc::channel();
        let first = CapturedFrame {
            width: 2,
            height: 2,
            stride: 8,
            data: vec![1; 16],
        };
        tx.send(CapturedFrame {
            width: 2,
            height: 2,
            stride: 8,
            data: vec![2; 16],
        })
        .unwrap();
        tx.send(CapturedFrame {
            width: 2,
            height: 2,
            stride: 8,
            data: vec![3; 16],
        })
        .unwrap();

        let latest = latest_frame_from_queue(first, &rx, None);

        assert_eq!(latest.data[0], 3);
    }

    #[test]
    fn frame_queue_slots_cap_pending_frames() {
        let queued = AtomicUsize::new(0);

        assert!(reserve_frame_slot(&queued));
        assert!(reserve_frame_slot(&queued));
        assert!(!reserve_frame_slot(&queued));
        assert_eq!(queued.load(Ordering::SeqCst), FRAME_QUEUE_BOUND);

        release_frame_slot(&queued);
        assert_eq!(queued.load(Ordering::SeqCst), FRAME_QUEUE_BOUND - 1);
        assert!(reserve_frame_slot(&queued));
        assert_eq!(queued.load(Ordering::SeqCst), FRAME_QUEUE_BOUND);
    }

    #[test]
    fn screencapturekit_queue_depth_tracks_copy_queue_bound() {
        assert_eq!(SCK_CAPTURE_QUEUE_DEPTH as usize, FRAME_QUEUE_BOUND);
    }

    #[test]
    fn latest_frame_from_queue_releases_drained_frame_slots() {
        let (tx, rx) = mpsc::channel();
        let queued = AtomicUsize::new(2);
        let first = CapturedFrame {
            width: 2,
            height: 2,
            stride: 8,
            data: vec![1; 16],
        };
        tx.send(CapturedFrame {
            width: 2,
            height: 2,
            stride: 8,
            data: vec![2; 16],
        })
        .unwrap();
        tx.send(CapturedFrame {
            width: 2,
            height: 2,
            stride: 8,
            data: vec![3; 16],
        })
        .unwrap();

        let latest = latest_frame_from_queue(first, &rx, Some(&queued));

        assert_eq!(latest.data[0], 3);
        assert_eq!(queued.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn legacy_duplicate_detection_ignores_stride_padding() {
        let previous = CapturedFrame {
            width: 2,
            height: 2,
            stride: 12,
            data: vec![
                1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9, 10, 11, 12, 13, 14, 15, 16, 17, 8, 8, 8, 8,
            ],
        };
        let current = CapturedFrame {
            width: 2,
            height: 2,
            stride: 12,
            data: vec![
                1, 2, 3, 4, 5, 6, 7, 8, 7, 7, 7, 7, 10, 11, 12, 13, 14, 15, 16, 17, 6, 6, 6, 6,
            ],
        };

        assert!(same_visible_frame(&previous, &current, 2, 2));
    }

    #[test]
    fn text_control_payload_uses_protocol_length_prefix() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&(7u16).to_be_bytes());
        payload.extend_from_slice("hello!!".as_bytes());

        assert_eq!(parse_text_payload(&payload).unwrap(), "hello!!");
    }

    #[test]
    fn text_control_payload_rejects_bad_lengths() {
        let mut truncated = Vec::new();
        truncated.extend_from_slice(&(5u16).to_be_bytes());
        truncated.extend_from_slice(b"hey");

        assert!(parse_text_payload(&[]).is_err());
        assert!(parse_text_payload(&truncated).is_err());
    }

    #[test]
    fn maps_common_windows_vkeys_to_macos_keycodes() {
        assert_eq!(windows_vkey_to_macos_keycode(0x41), Some(0));
        assert_eq!(windows_vkey_to_macos_keycode(0x30), Some(29));
        assert_eq!(windows_vkey_to_macos_keycode(0x10), Some(56));
        assert_eq!(windows_vkey_to_macos_keycode(0xA1), Some(60));
        assert_eq!(windows_vkey_to_macos_keycode(0x11), Some(59));
        assert_eq!(windows_vkey_to_macos_keycode(0xA3), Some(62));
        assert_eq!(windows_vkey_to_macos_keycode(0x12), Some(58));
        assert_eq!(windows_vkey_to_macos_keycode(0xA5), Some(61));
        assert_eq!(windows_vkey_to_macos_keycode(0x5B), Some(55));
        assert_eq!(windows_vkey_to_macos_keycode(0x5C), Some(54));
        assert_eq!(windows_vkey_to_macos_keycode(0x2E), Some(117));
        assert_eq!(windows_vkey_to_macos_keycode(0x24), Some(115));
        assert_eq!(windows_vkey_to_macos_keycode(0x23), Some(119));
        assert_eq!(windows_vkey_to_macos_keycode(0x21), Some(116));
        assert_eq!(windows_vkey_to_macos_keycode(0x22), Some(121));
        assert_eq!(windows_vkey_to_macos_keycode(0xBA), Some(41));
        assert_eq!(windows_vkey_to_macos_keycode(0xDE), Some(39));
        assert_eq!(windows_vkey_to_macos_keycode(0x70), Some(122));
        assert_eq!(windows_vkey_to_macos_keycode(0x7B), Some(111));
        assert_eq!(windows_vkey_to_macos_keycode(0x60), Some(82));
        assert_eq!(windows_vkey_to_macos_keycode(0x6F), Some(75));
        assert_eq!(windows_vkey_to_macos_keycode(0xFF), None);
        assert!(windows_vkey_is_modifier(0x10));
        assert!(windows_vkey_is_modifier(0xA5));
        assert!(!windows_vkey_is_modifier(0x41));
    }

    #[test]
    fn maps_protocol_modifiers_to_coregraphics_flags() {
        assert_eq!(cg_event_flags_for_modifiers(0), 0);
        assert_eq!(
            cg_event_flags_for_modifiers(CONTROL_MOD_CTRL | CONTROL_MOD_SHIFT),
            CG_EVENT_FLAG_MASK_CONTROL | CG_EVENT_FLAG_MASK_SHIFT
        );
        assert_eq!(
            cg_event_flags_for_modifiers(CONTROL_MOD_ALT | CONTROL_MOD_WIN),
            CG_EVENT_FLAG_MASK_ALTERNATE | CG_EVENT_FLAG_MASK_COMMAND
        );
    }

    #[test]
    fn ordinary_key_events_use_coregraphics_modifier_flags() {
        assert_eq!(
            key_event_flags_for_vkey(0x41, CONTROL_MOD_CTRL | CONTROL_MOD_SHIFT),
            CG_EVENT_FLAG_MASK_CONTROL | CG_EVENT_FLAG_MASK_SHIFT
        );
        assert_eq!(key_event_flags_for_vkey(0x11, CONTROL_MOD_CTRL), 0);
    }

    #[test]
    fn windows_wheel_delta_maps_to_macos_line_ticks() {
        assert_eq!(windows_wheel_delta_to_macos_lines(0), 0);
        assert_eq!(windows_wheel_delta_to_macos_lines(120), -1);
        assert_eq!(windows_wheel_delta_to_macos_lines(-120), 1);
        assert_eq!(windows_wheel_delta_to_macos_lines(240), -2);
        assert_eq!(windows_wheel_delta_to_macos_lines(-60), 1);
    }

    #[test]
    fn raw_atx2_stream_uses_expected_wire_layout() {
        let frame = CapturedFrame {
            width: 2,
            height: 2,
            stride: 12,
            data: vec![
                1, 2, 3, 4, 5, 6, 7, 8, 99, 99, 99, 99, 9, 10, 11, 12, 13, 14, 15, 16, 88, 88, 88,
                88,
            ],
        };

        let stream = build_raw_atx2_stream_region(
            &frame,
            DirtyRegion {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
        )
        .expect("raw ATX2 stream");

        assert_eq!(read_u32(&stream, 0), ATX2_STREAM_MAGIC);
        assert_eq!(read_u32(&stream, 4), ATX2_STREAM_VERSION);
        assert_eq!(read_u32(&stream, 8), pack_xy(2, 2));
        assert_eq!(read_u32(&stream, 16), 1);
        assert_eq!(read_u32(&stream, 20) as usize, stream.len());
        assert_eq!(read_u32(&stream, ATX2_HEADER_BYTES), ATX2_COMMAND_RAW_BGRA);
        assert_eq!(read_u32(&stream, ATX2_HEADER_BYTES + 12), pack_xy(2, 2));
        assert_eq!(read_u32(&stream, ATX2_HEADER_BYTES + 16), 16);
        assert_eq!(read_u32(&stream, ATX2_HEADER_BYTES + 20), 4);
        assert_eq!(
            &stream[ATX2_HEADER_BYTES + ATX2_COMMAND_HEADER_BYTES..],
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
    }

    #[test]
    fn exact_atx2_stream_uses_solid_tile_commands() {
        let width = 64;
        let height = 32;
        let stride = width * 4;
        let frame = CapturedFrame {
            width,
            height,
            stride,
            data: [7, 8, 9, 255].repeat((width * height) as usize),
        };

        let stream = build_exact_atx2_stream_region(
            &frame,
            DirtyRegion {
                x: 0,
                y: 0,
                width,
                height,
            },
            None,
        )
        .expect("exact ATX2 stream");

        assert_eq!(read_u32(&stream, 16), 2);
        assert_eq!(read_u32(&stream, 24), 2);
        assert_eq!(
            read_u32(&stream, ATX2_HEADER_BYTES),
            ATX2_COMMAND_SOLID_COLOR
        );
        assert_eq!(read_u32(&stream, ATX2_HEADER_BYTES + 4), pack_xy(0, 0));
        assert_eq!(read_u32(&stream, ATX2_HEADER_BYTES + 16), 4);
        let second_offset = ATX2_HEADER_BYTES + ATX2_COMMAND_HEADER_BYTES + 4;
        assert_eq!(read_u32(&stream, second_offset), ATX2_COMMAND_SOLID_COLOR);
        assert_eq!(read_u32(&stream, second_offset + 4), pack_xy(32, 0));
        assert_eq!(
            &stream[ATX2_HEADER_BYTES + ATX2_COMMAND_HEADER_BYTES
                ..ATX2_HEADER_BYTES + ATX2_COMMAND_HEADER_BYTES + 4],
            &[7, 8, 9, 255]
        );
    }

    #[test]
    fn capture_dirty_rects_are_clipped_and_split_for_atx2_chunks() {
        let regions =
            normalize_capture_dirty_rects([(-1.2, 1.2, 4.1, 3.3), (15.0, 15.0, 5.0, 5.0)], 10, 10);

        assert_eq!(
            regions,
            vec![DirtyRegion {
                x: 0,
                y: 1,
                width: 3,
                height: 4,
            }]
        );
        assert_eq!(
            split_capture_dirty_regions(&regions, 10, 10, 2),
            vec![
                DirtyRegion {
                    x: 0,
                    y: 1,
                    width: 3,
                    height: 2,
                },
                DirtyRegion {
                    x: 0,
                    y: 3,
                    width: 3,
                    height: 2,
                },
            ]
        );
    }

    #[test]
    fn atx2_frame_records_split_large_frames_into_bands() {
        let width = 10;
        let height = 3;
        let stride = width * 4;
        let data: Vec<u8> = (0u32..height)
            .flat_map(|row| {
                (0u32..stride).map(move |byte| {
                    row.checked_mul(stride)
                        .and_then(|base| base.checked_add(byte))
                        .unwrap() as u8
                })
            })
            .collect();
        let frame = CapturedFrame {
            width,
            height,
            stride,
            data,
        };

        let records = build_atx2_frame_atlas_records(42, &frame, None, width, height, None)
            .expect("ATX2 records");

        assert_eq!(records.len(), 2);
        let first = decode_display_record(&records[0]).expect("decode first ATX2 chunk");
        assert_eq!(
            first,
            DisplayRecord::ExperimentalAtlasCommandsChunk {
                frame_id: 42,
                flags: DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE,
                chunk_index: 0,
                chunk_count: 2,
                atlas_width: 10,
                atlas_height: 2,
                rects: vec![DisplayAtlasRect {
                    dst_x: 0,
                    dst_y: 0,
                    width: 10,
                    height: 2,
                    atlas_x: 0,
                    atlas_y: 0,
                }],
                tile_commands: build_raw_atx2_stream_region(
                    &frame,
                    DirtyRegion {
                        x: 0,
                        y: 0,
                        width,
                        height: 2,
                    },
                )
                .unwrap(),
            }
        );
        let second = decode_display_record(&records[1]).expect("decode final ATX2 chunk");
        assert_eq!(
            second,
            DisplayRecord::ExperimentalAtlasCommandsChunk {
                frame_id: 42,
                flags: DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE
                    | DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL,
                chunk_index: 1,
                chunk_count: 2,
                atlas_width: 10,
                atlas_height: 1,
                rects: vec![DisplayAtlasRect {
                    dst_x: 0,
                    dst_y: 2,
                    width: 10,
                    height: 1,
                    atlas_x: 0,
                    atlas_y: 0,
                }],
                tile_commands: build_raw_atx2_stream_region(
                    &frame,
                    DirtyRegion {
                        x: 0,
                        y: 2,
                        width,
                        height: 1,
                    },
                )
                .unwrap(),
            }
        );
    }

    #[test]
    fn atx2_frame_records_send_only_dirty_tile_runs() {
        let width = 96;
        let height = 64;
        let stride = width * 4;
        let previous = CapturedFrame {
            width,
            height,
            stride,
            data: vec![0; (height * stride) as usize],
        };
        let mut frame = previous.clone();
        frame.data[(2 * stride + 4) as usize] = 7;
        frame.data[(40 * stride + 70 * 4 + 3) as usize] = 9;

        let records =
            build_atx2_frame_atlas_records(43, &frame, Some(&previous), width, height, None)
                .expect("ATX2 dirty records");

        assert_eq!(records.len(), 2);
        let first = decode_display_record(&records[0]).expect("decode first dirty ATX2 chunk");
        assert_eq!(
            first,
            DisplayRecord::ExperimentalAtlasCommandsChunk {
                frame_id: 43,
                flags: DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE,
                chunk_index: 0,
                chunk_count: 2,
                atlas_width: 32,
                atlas_height: 32,
                rects: vec![DisplayAtlasRect {
                    dst_x: 0,
                    dst_y: 0,
                    width: 32,
                    height: 32,
                    atlas_x: 0,
                    atlas_y: 0,
                }],
                tile_commands: build_raw_atx2_stream_region(
                    &frame,
                    DirtyRegion {
                        x: 0,
                        y: 0,
                        width: 32,
                        height: 32,
                    },
                )
                .unwrap(),
            }
        );
        let second = decode_display_record(&records[1]).expect("decode final dirty ATX2 chunk");
        assert_eq!(
            second,
            DisplayRecord::ExperimentalAtlasCommandsChunk {
                frame_id: 43,
                flags: DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE
                    | DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL,
                chunk_index: 1,
                chunk_count: 2,
                atlas_width: 32,
                atlas_height: 32,
                rects: vec![DisplayAtlasRect {
                    dst_x: 64,
                    dst_y: 32,
                    width: 32,
                    height: 32,
                    atlas_x: 0,
                    atlas_y: 0,
                }],
                tile_commands: build_raw_atx2_stream_region(
                    &frame,
                    DirtyRegion {
                        x: 64,
                        y: 32,
                        width: 32,
                        height: 32,
                    },
                )
                .unwrap(),
            }
        );
    }

    #[test]
    fn dirty_tile_runs_merge_vertical_neighbors_with_size_cap() {
        let width = 96;
        let height = 96;
        let stride = width * 4;
        let previous = CapturedFrame {
            width,
            height,
            stride,
            data: vec![0; (height * stride) as usize],
        };
        let mut frame = previous.clone();
        for y in [2u32, 40, 72] {
            frame.data[(y * stride + 4) as usize] = 7;
        }

        let merged = dirty_tile_run_regions(&previous, &frame, width, height, 96);
        assert_eq!(
            merged,
            vec![DirtyRegion {
                x: 0,
                y: 0,
                width: 32,
                height: 96,
            }]
        );

        let capped = dirty_tile_run_regions(&previous, &frame, width, height, 64);
        assert_eq!(
            capped,
            vec![
                DirtyRegion {
                    x: 0,
                    y: 0,
                    width: 32,
                    height: 64,
                },
                DirtyRegion {
                    x: 0,
                    y: 64,
                    width: 32,
                    height: 32,
                },
            ]
        );
    }

    #[test]
    fn dirty_tile_runs_keep_sparse_same_band_changes_separate() {
        let width = 160;
        let height = 32;
        let stride = width * 4;
        let previous = CapturedFrame {
            width,
            height,
            stride,
            data: vec![0; (height * stride) as usize],
        };
        let mut frame = previous.clone();
        frame.data[(2 * stride + 4) as usize] = 7;
        frame.data[(20 * stride + 130 * 4 + 1) as usize] = 9;

        let regions = dirty_tile_run_regions(&previous, &frame, width, height, 32);

        assert_eq!(
            regions,
            vec![
                DirtyRegion {
                    x: 0,
                    y: 0,
                    width: 32,
                    height: 32,
                },
                DirtyRegion {
                    x: 128,
                    y: 0,
                    width: 32,
                    height: 32,
                },
            ]
        );
    }

    #[test]
    fn visible_row_bounds_trim_unchanged_prefix_and_suffix() {
        let previous = [1, 2, 3, 4, 5, 6, 7, 8];
        let current = [1, 2, 9, 4, 5, 6, 10, 8];

        assert_eq!(
            changed_visible_row_bounds(&previous, &current),
            Some((2, 7))
        );
        assert_eq!(changed_visible_row_bounds(&previous, &previous), None);
    }

    #[test]
    fn visible_row_bounds_handle_wide_unchanged_edges() {
        let previous = vec![0u8; 128];
        let mut current = previous.clone();
        current[64] = 1;
        current[65] = 2;

        assert_eq!(
            changed_visible_row_bounds(&previous, &current),
            Some((64, 66))
        );
    }

    #[test]
    fn atx2_frame_records_empty_for_unchanged_delta() {
        let width = 64;
        let height = 64;
        let stride = width * 4;
        let frame = CapturedFrame {
            width,
            height,
            stride,
            data: vec![11; (height * stride) as usize],
        };

        let records = build_atx2_frame_atlas_records(44, &frame, Some(&frame), width, height, None)
            .expect("ATX2 unchanged delta");

        assert!(records.is_empty());
    }

    #[test]
    fn visible_pixel_equality_ignores_stride_padding() {
        let width = 2;
        let height = 2;
        let stride = 12;
        let previous = CapturedFrame {
            width,
            height,
            stride,
            data: vec![
                1, 2, 3, 4, 5, 6, 7, 8, 90, 90, 90, 90, 9, 10, 11, 12, 13, 14, 15, 16, 91, 91, 91,
                91,
            ],
        };
        let frame = CapturedFrame {
            width,
            height,
            stride,
            data: vec![
                1, 2, 3, 4, 5, 6, 7, 8, 40, 40, 40, 40, 9, 10, 11, 12, 13, 14, 15, 16, 41, 41, 41,
                41,
            ],
        };

        assert!(visible_pixels_equal(&previous, &frame, width, height));
        assert!(dirty_atx2_regions(Some(&previous), &frame, width, height, 8).is_empty());
    }
}
