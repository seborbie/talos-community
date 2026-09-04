use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    mem::{size_of, ManuallyDrop},
    ptr,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use apple_metal::{
    pixel_format, resource_options, storage_mode, texture_usage, CommandQueue,
    ComputePipelineState, MetalBuffer, MetalDevice, MetalTexture, TextureDescriptor,
};
use objc2::{
    define_class, msg_send,
    rc::{Allocated, Retained},
    runtime::AnyObject,
    AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSEvent, NSEventModifierFlags, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow,
};
use objc2_core_graphics::{CGMutablePath, CGPath};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::{kCAFillRuleEvenOdd, CAMetalLayer, CAShapeLayer, CATransaction};
use tauri::Window;
use tracing::debug;

use crate::{
    send_native_control_with_control_state, ControlEvent, ControlState, DecodedFrame,
    ViewportOcclusionRect, CONTROL_MOD_ALT, CONTROL_MOD_CTRL, CONTROL_MOD_SHIFT, CONTROL_MOD_WIN,
};

const METAL_SHADER: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct ViewportUniforms {
    uint drawable_width;
    uint drawable_height;
    uint source_width;
    uint source_height;
    uint dst_x;
    uint dst_y;
    uint dst_width;
    uint dst_height;
};

kernel void talos_blit_letterbox(
    texture2d<float, access::read> source [[texture(0)]],
    texture2d<float, access::write> target [[texture(1)]],
    constant ViewportUniforms& u [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= u.drawable_width || gid.y >= u.drawable_height) {
        return;
    }

    float4 color = float4(0.0, 0.0, 0.0, 1.0);
    if (u.dst_width > 0 && u.dst_height > 0 &&
        gid.x >= u.dst_x && gid.x < u.dst_x + u.dst_width &&
        gid.y >= u.dst_y && gid.y < u.dst_y + u.dst_height) {
        uint local_x = gid.x - u.dst_x;
        uint local_y = gid.y - u.dst_y;
        uint sx = min((local_x * u.source_width) / u.dst_width, u.source_width - 1);
        uint sy = min((local_y * u.source_height) / u.dst_height, u.source_height - 1);
        color = source.read(uint2(sx, sy));
    }

    target.write(color, gid);
}
"#;

static DISABLE_NATIVE_VIEWPORT: OnceLock<bool> = OnceLock::new();

pub(crate) fn native_viewport_enabled() -> bool {
    !*DISABLE_NATIVE_VIEWPORT.get_or_init(|| {
        std::env::var_os("TALOS_VIEWER_DISABLE_MACOS_NATIVE_VIEWPORT")
            .is_some_and(|value| value == "1" || value == "true" || value == "TRUE")
    })
}

#[derive(Clone, Default)]
pub(crate) struct ViewportState {
    pub(crate) inner: Arc<Mutex<ViewportInner>>,
}

#[derive(Default)]
pub(crate) struct ViewportInner {
    view: Option<Retained<TalosRemoteDesktopView>>,
    layer: Option<Retained<CAMetalLayer>>,
    renderer: Option<MacMetalViewport>,
    last_frame: Option<DecodedFrame>,
    last_rect: Option<(i32, i32, u32, u32)>,
    last_scale: f64,
    hidden: bool,
    native_failed: bool,
    fallback_warned: bool,
}

unsafe impl Send for ViewportInner {}
unsafe impl Sync for ViewportInner {}

impl ViewportInner {
    pub(crate) fn present_decoded_frame(&mut self, frame: DecodedFrame) -> Result<bool, String> {
        self.last_frame = Some(frame.clone());
        if !native_viewport_enabled() || self.native_failed {
            return Ok(false);
        }
        if self.hidden || self.layer.is_none() || self.renderer.is_none() {
            return Ok(true);
        }

        let layer = self
            .layer
            .as_ref()
            .ok_or_else(|| "macOS native viewport layer is missing".to_string())?;
        let renderer = self
            .renderer
            .as_mut()
            .ok_or_else(|| "macOS native viewport renderer is missing".to_string())?;

        match renderer.present(layer, &frame) {
            Ok(()) => Ok(true),
            Err(err) => {
                self.native_failed = true;
                Err(err)
            }
        }
    }

    pub(crate) fn update_rect(
        &mut self,
        window: &Window,
        control_state: ControlState,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        occlusions: &[ViewportOcclusionRect],
        scale: f64,
    ) -> Result<(), String> {
        self.last_scale = scale;
        if width == 0 || height == 0 || !native_viewport_enabled() || self.native_failed {
            self.hidden = true;
            self.last_rect = None;
            if let Some(view) = self.view.as_ref() {
                view.set_cutouts(Vec::new());
                view.as_super().setHidden(true);
            }
            if let Some(layer) = self.layer.as_ref() {
                clear_layer_cutout_mask(layer);
            }
            return Ok(());
        }

        let ns_window = window.ns_window().map_err(|err| err.to_string())?;
        let ns_window = unsafe { &*(ns_window.cast::<NSWindow>()) };
        let content_view = ns_window
            .contentView()
            .ok_or_else(|| "macOS window has no content view".to_string())?;

        let host_view = find_webview_host_view(&content_view, self.view.as_deref());
        let parent_view = host_view.as_deref().unwrap_or(&content_view);

        if let Err(err) = self.ensure_view(ns_window, parent_view, control_state) {
            self.native_failed = true;
            return Err(err);
        }
        let frame = native_frame_from_dom_rect(
            parent_view.bounds(),
            parent_view.isFlipped(),
            x,
            y,
            width,
            height,
        );
        if frame.size.width <= 0.0 || frame.size.height <= 0.0 {
            self.hidden = true;
            self.last_rect = None;
            if let Some(view) = self.view.as_ref() {
                view.set_cutouts(Vec::new());
                view.as_super().setHidden(true);
            }
            if let Some(layer) = self.layer.as_ref() {
                clear_layer_cutout_mask(layer);
            }
            return Ok(());
        }

        let Some(view) = self.view.as_ref() else {
            return Err("macOS native viewport view is missing".to_string());
        };
        let cutouts = viewport_cutouts_from_window_occlusions(
            x,
            y,
            frame.size.width,
            frame.size.height,
            occlusions,
        );

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        view.as_super().setFrame(frame);
        view.set_cutouts(cutouts.clone());
        if let Some(layer) = self.layer.as_ref() {
            apply_layer_cutout_mask(layer, frame.size, &cutouts);
        }
        view.as_super().setHidden(false);
        CATransaction::commit();

        self.hidden = false;
        self.last_rect = Some((x, y, width, height));
        self.update_drawable_size(
            frame.size.width.round() as u32,
            frame.size.height.round() as u32,
            scale,
        );

        if let Some(frame) = self.last_frame.clone() {
            let _ = self.present_decoded_frame(frame);
        }

        Ok(())
    }

    pub(crate) fn take_fallback_warning(&mut self) -> bool {
        if self.fallback_warned {
            false
        } else {
            self.fallback_warned = true;
            true
        }
    }

    fn ensure_view(
        &mut self,
        ns_window: &NSWindow,
        parent_view: &NSView,
        control_state: ControlState,
    ) -> Result<(), String> {
        if self.renderer.is_none() {
            self.renderer = Some(MacMetalViewport::new()?);
        }

        if self.view.is_some() && self.layer.is_some() {
            if let Some(view) = self.view.as_ref() {
                let needs_attach = unsafe { view.as_super().superview() }
                    .map(|superview| !ptr::eq(&*superview, parent_view))
                    .unwrap_or(true);
                if needs_attach {
                    view.as_super().removeFromSuperview();
                    parent_view.addSubview(view.as_super());
                }
            }
            ns_window.setAcceptsMouseMovedEvents(true);
            return Ok(());
        }

        let renderer = self
            .renderer
            .as_ref()
            .ok_or_else(|| "macOS native viewport renderer is missing".to_string())?;
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            "macOS native viewport must be created on the main thread".to_string()
        })?;
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0));
        let view = unsafe {
            TalosRemoteDesktopView::init_with_frame(
                TalosRemoteDesktopView::alloc(mtm),
                frame,
                control_state,
            )
        };
        let layer = CAMetalLayer::new();
        configure_metal_layer(&layer, renderer.device_ptr(), self.last_scale);

        view.as_super().setWantsLayer(true);
        view.as_super().setLayer(Some(layer.as_super()));
        view.as_super().setHidden(true);
        parent_view.addSubview(view.as_super());
        ns_window.setInitialFirstResponder(Some(view.as_super()));
        ns_window.setAcceptsMouseMovedEvents(true);

        self.layer = Some(layer);
        self.view = Some(view);
        debug!("macOS native remote desktop viewport created");
        Ok(())
    }

    fn update_drawable_size(&mut self, width: u32, height: u32, scale: f64) {
        let drawable_width = ((width as f64) * scale).round().max(1.0) as u32;
        let drawable_height = ((height as f64) * scale).round().max(1.0) as u32;
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_drawable_size(drawable_width, drawable_height);
        }
        if let Some(layer) = self.layer.as_ref() {
            unsafe {
                let size = NSSize::new(drawable_width as f64, drawable_height as f64);
                let _: () = msg_send![&**layer, setDrawableSize: size];
                let _: () = msg_send![&**layer, setContentsScale: scale];
            }
        }
    }
}

fn find_webview_host_view(
    content_view: &NSView,
    native_view: Option<&TalosRemoteDesktopView>,
) -> Option<Retained<NSView>> {
    let native_ptr = native_view.map(|view| view.as_super() as *const NSView);
    let subviews = content_view.subviews();
    let mut best: Option<(f64, Retained<NSView>)> = None;

    for index in 0..subviews.count() {
        let subview = subviews.objectAtIndex(index);
        let subview_ptr = (&*subview) as *const NSView;
        if native_ptr.is_some_and(|ptr| ptr == subview_ptr) || subview.isHidden() {
            continue;
        }

        let frame = subview.frame();
        let area = (frame.size.width.max(0.0) * frame.size.height.max(0.0)).max(0.0);
        if area <= 0.0 {
            continue;
        }
        if best.as_ref().is_none_or(|(best_area, _)| area > *best_area) {
            best = Some((area, subview));
        }
    }

    best.map(|(_, view)| view)
}

fn native_frame_from_dom_rect(
    parent_bounds: NSRect,
    parent_flipped: bool,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> NSRect {
    let parent_width = parent_bounds.size.width.max(0.0);
    let parent_height = parent_bounds.size.height.max(0.0);
    let frame_x = (x.max(0) as f64).clamp(0.0, parent_width);
    let requested_top = (y.max(0) as f64).clamp(0.0, parent_height);
    let frame_width = (width as f64).min(parent_width - frame_x).max(0.0);
    let frame_height = (height as f64).min(parent_height - requested_top).max(0.0);
    // The DOM rect is relative to the WebView viewport, while the AppKit parent
    // can include extra native chrome space. Preserve the requested viewport
    // height and pin it to the native parent's bottom edge when that mismatch
    // leaves a trailing gap.
    let frame_top = (parent_height - frame_height).max(requested_top);
    let frame_y = if parent_flipped {
        frame_top
    } else {
        (parent_height - frame_top - frame_height).max(0.0)
    };

    NSRect::new(
        NSPoint::new(frame_x, frame_y),
        NSSize::new(frame_width, frame_height),
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewportCutout {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl ViewportCutout {
    fn contains(self, point: NSPoint) -> bool {
        point.x >= self.x
            && point.x < self.x + self.width
            && point.y >= self.y
            && point.y < self.y + self.height
    }
}

fn viewport_cutouts_from_window_occlusions(
    viewport_x: i32,
    viewport_y: i32,
    viewport_width: f64,
    viewport_height: f64,
    occlusions: &[ViewportOcclusionRect],
) -> Vec<ViewportCutout> {
    let viewport_width = viewport_width.max(0.0);
    let viewport_height = viewport_height.max(0.0);
    if viewport_width <= 0.0 || viewport_height <= 0.0 {
        return Vec::new();
    }

    occlusions
        .iter()
        .filter_map(|occlusion| {
            if occlusion.width == 0 || occlusion.height == 0 {
                return None;
            }

            let left = (occlusion.x as f64 - viewport_x as f64 - 1.0).clamp(0.0, viewport_width);
            let top = (occlusion.y as f64 - viewport_y as f64 - 1.0).clamp(0.0, viewport_height);
            let right = (occlusion.x as f64 + occlusion.width as f64 - viewport_x as f64 + 1.0)
                .clamp(0.0, viewport_width);
            let bottom = (occlusion.y as f64 + occlusion.height as f64 - viewport_y as f64 + 1.0)
                .clamp(0.0, viewport_height);

            if right <= left || bottom <= top {
                return None;
            }

            Some(ViewportCutout {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            })
        })
        .collect()
}

fn apply_layer_cutout_mask(layer: &CAMetalLayer, size: NSSize, cutouts: &[ViewportCutout]) {
    if cutouts.is_empty() || size.width <= 0.0 || size.height <= 0.0 {
        clear_layer_cutout_mask(layer);
        return;
    }

    let mask = CAShapeLayer::new();
    let bounds = NSRect::new(NSPoint::new(0.0, 0.0), size);
    let path = CGMutablePath::new();
    unsafe {
        mask.as_super().setFrame(bounds);
        CGMutablePath::add_rect(Some(&path), ptr::null(), bounds);
        for cutout in cutouts {
            let rect = layer_mask_rect_for_cutout(*cutout);
            CGMutablePath::add_rect(Some(&path), ptr::null(), rect);
        }
        mask.setFillRule(kCAFillRuleEvenOdd);
        let path_ref: &CGPath = (*path).as_ref();
        mask.setPath(Some(path_ref));
        layer.as_super().setMask(Some(mask.as_super()));
    }
}

fn layer_mask_rect_for_cutout(cutout: ViewportCutout) -> NSRect {
    NSRect::new(
        NSPoint::new(cutout.x, cutout.y),
        NSSize::new(cutout.width, cutout.height),
    )
}

fn clear_layer_cutout_mask(layer: &CAMetalLayer) {
    unsafe {
        layer.as_super().setMask(None);
    }
}

fn configure_metal_layer(layer: &CAMetalLayer, device: *mut c_void, scale: f64) {
    layer.setFramebufferOnly(false);
    layer.setPresentsWithTransaction(false);
    layer.setAllowsNextDrawableTimeout(true);
    unsafe {
        let _: () = msg_send![layer, setDevice: device.cast::<AnyObject>()];
        let _: () = msg_send![layer, setPixelFormat: pixel_format::BGRA8UNORM];
        let _: () = msg_send![layer, setContentsScale: scale];
        let _: () = msg_send![layer, setOpaque: true];
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ViewportUniforms {
    drawable_width: u32,
    drawable_height: u32,
    source_width: u32,
    source_height: u32,
    dst_x: u32,
    dst_y: u32,
    dst_width: u32,
    dst_height: u32,
}

struct MacMetalViewport {
    device: MetalDevice,
    queue: CommandQueue,
    pipeline: ComputePipelineState,
    uniforms: MetalBuffer,
    source_texture: Option<MetalTexture>,
    source_size: Option<(u32, u32)>,
    upload_bgra: Vec<u8>,
    drawable_size: (u32, u32),
}

impl MacMetalViewport {
    fn new() -> Result<Self, String> {
        let device = MetalDevice::system_default()
            .ok_or_else(|| "no system Metal device is available".to_string())?;
        let queue = device
            .new_command_queue()
            .ok_or_else(|| "failed to create Metal command queue".to_string())?;
        let library = device.new_library_with_source(METAL_SHADER)?;
        let function = library
            .new_function("talos_blit_letterbox")
            .ok_or_else(|| "failed to load Metal viewport shader".to_string())?;
        let pipeline = device.new_compute_pipeline_state(&function)?;
        let uniforms = device
            .new_buffer(
                size_of::<ViewportUniforms>(),
                resource_options::STORAGE_MODE_SHARED | resource_options::CPU_CACHE_MODE_DEFAULT,
            )
            .ok_or_else(|| "failed to allocate Metal viewport uniforms".to_string())?;

        Ok(Self {
            device,
            queue,
            pipeline,
            uniforms,
            source_texture: None,
            source_size: None,
            upload_bgra: Vec::new(),
            drawable_size: (1, 1),
        })
    }

    fn device_ptr(&self) -> *mut c_void {
        self.device.as_ptr()
    }

    fn set_drawable_size(&mut self, width: u32, height: u32) {
        self.drawable_size = (width.max(1), height.max(1));
    }

    fn present(&mut self, layer: &CAMetalLayer, frame: &DecodedFrame) -> Result<(), String> {
        if frame.width == 0 || frame.height == 0 || frame.argb.is_empty() {
            return Ok(());
        }

        self.ensure_source_texture(frame.width, frame.height)?;
        self.upload_frame(frame)?;

        let Some(source_texture) = self.source_texture.as_ref() else {
            return Err("macOS native viewport source texture is missing".to_string());
        };
        let (drawable, drawable_texture) = next_drawable(layer)
            .ok_or_else(|| "CAMetalLayer did not provide a drawable".to_string())?;

        let (drawable_width, drawable_height) = self.drawable_size;
        let (dst_x, dst_y, dst_width, dst_height) =
            letterbox_rect(drawable_width, drawable_height, frame.width, frame.height);
        let uniforms = ViewportUniforms {
            drawable_width,
            drawable_height,
            source_width: frame.width,
            source_height: frame.height,
            dst_x,
            dst_y,
            dst_width,
            dst_height,
        };
        let uniform_bytes = unsafe {
            std::slice::from_raw_parts(
                (&uniforms as *const ViewportUniforms).cast::<u8>(),
                size_of::<ViewportUniforms>(),
            )
        };
        if self.uniforms.write_bytes(uniform_bytes) != uniform_bytes.len() {
            return Err("failed to update Metal viewport uniforms".to_string());
        }

        let command_buffer = self
            .queue
            .new_command_buffer()
            .ok_or_else(|| "failed to create Metal command buffer".to_string())?;
        let encoder = command_buffer
            .new_compute_command_encoder()
            .ok_or_else(|| "failed to create Metal compute encoder".to_string())?;
        encoder.set_compute_pipeline_state(&self.pipeline);
        encoder.set_buffer(&self.uniforms, 0, 0);
        encoder.set_texture(source_texture, 0);
        encoder.set_texture(&drawable_texture, 1);
        let threads = (16usize, 16usize, 1usize);
        let groups = (
            (drawable_width as usize).div_ceil(threads.0),
            (drawable_height as usize).div_ceil(threads.1),
            1usize,
        );
        encoder.dispatch_threadgroups(groups, threads);
        encoder.end_encoding();

        unsafe {
            let _: () = msg_send![
                command_buffer.as_ptr().cast::<AnyObject>(),
                presentDrawable: drawable
            ];
        }
        command_buffer.commit();
        Ok(())
    }

    fn ensure_source_texture(&mut self, width: u32, height: u32) -> Result<(), String> {
        if self.source_size == Some((width, height)) && self.source_texture.is_some() {
            return Ok(());
        }

        let mut descriptor =
            TextureDescriptor::new_2d(width as usize, height as usize, pixel_format::BGRA8UNORM);
        descriptor.usage = texture_usage::SHADER_READ;
        descriptor.storage_mode = storage_mode::SHARED;
        self.source_texture = Some(
            self.device
                .new_texture(descriptor)
                .ok_or_else(|| "failed to allocate Metal source texture".to_string())?,
        );
        self.source_size = Some((width, height));
        Ok(())
    }

    fn upload_frame(&mut self, frame: &DecodedFrame) -> Result<(), String> {
        let expected_pixels = frame.width as usize * frame.height as usize;
        if frame.argb.len() < expected_pixels {
            return Err("decoded frame is smaller than its dimensions".to_string());
        }

        self.upload_bgra.resize(expected_pixels * 4, 0);
        for (pixel, chunk) in frame
            .argb
            .iter()
            .copied()
            .take(expected_pixels)
            .zip(self.upload_bgra.chunks_exact_mut(4))
        {
            chunk[0] = (pixel & 0xff) as u8;
            chunk[1] = ((pixel >> 8) & 0xff) as u8;
            chunk[2] = ((pixel >> 16) & 0xff) as u8;
            chunk[3] = ((pixel >> 24) & 0xff) as u8;
        }

        let Some(texture) = self.source_texture.as_ref() else {
            return Err("macOS native viewport source texture is missing".to_string());
        };
        let bytes_per_row = frame.width as usize * 4;
        if !texture.replace_region_2d(
            &self.upload_bgra,
            bytes_per_row,
            (0, 0),
            (frame.width as usize, frame.height as usize),
            0,
        ) {
            return Err("failed to upload decoded frame to Metal texture".to_string());
        }
        Ok(())
    }
}

fn next_drawable(layer: &CAMetalLayer) -> Option<(*mut AnyObject, ManuallyDrop<MetalTexture>)> {
    unsafe {
        let drawable: *mut AnyObject = msg_send![layer, nextDrawable];
        if drawable.is_null() {
            return None;
        }
        let texture: *mut AnyObject = msg_send![drawable, texture];
        if texture.is_null() {
            return None;
        }
        Some((
            drawable,
            ManuallyDrop::new(MetalTexture::from_raw(texture.cast::<c_void>())),
        ))
    }
}

fn letterbox_rect(
    drawable_width: u32,
    drawable_height: u32,
    source_width: u32,
    source_height: u32,
) -> (u32, u32, u32, u32) {
    if drawable_width == 0 || drawable_height == 0 || source_width == 0 || source_height == 0 {
        return (0, 0, 0, 0);
    }

    let drawable_aspect = drawable_width as f64 / drawable_height as f64;
    let source_aspect = source_width as f64 / source_height as f64;
    if drawable_aspect > source_aspect {
        let height = drawable_height;
        let width = ((height as f64) * source_aspect).round().max(1.0) as u32;
        let x = (drawable_width.saturating_sub(width)) / 2;
        (x, 0, width.min(drawable_width), height)
    } else {
        let width = drawable_width;
        let height = ((width as f64) / source_aspect).round().max(1.0) as u32;
        let y = (drawable_height.saturating_sub(height)) / 2;
        (0, y, width, height.min(drawable_height))
    }
}

struct TalosRemoteDesktopViewIvars {
    control_state: ControlState,
    cutouts: RefCell<Vec<ViewportCutout>>,
    focused: Cell<bool>,
    last_mouse_move_at: Cell<Option<Instant>>,
    modifier_mask: Cell<u8>,
    tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "TalosRemoteDesktopView"]
    #[thread_kind = MainThreadOnly]
    #[ivars = TalosRemoteDesktopViewIvars]
    struct TalosRemoteDesktopView;

    impl TalosRemoteDesktopView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeKeyView))]
        fn can_become_key_view(&self) -> bool {
            true
        }

        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            if self.point_is_cutout(point) {
                return ptr::null_mut();
            }
            unsafe { msg_send![super(self), hitTest: point] }
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            self.ivars().focused.set(true);
            true
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            self.ivars().focused.set(false);
            true
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            unsafe {
                let _: () = msg_send![super(self), updateTrackingAreas];
            }
            self.refresh_tracking_area();
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.refresh_tracking_area();
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {}

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.send_mouse_move(event);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.send_mouse_move(event);
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            self.send_mouse_move(event);
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            self.send_mouse_move(event);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.send_mouse_button(event, 0, true);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.send_mouse_button(event, 0, false);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            self.send_mouse_button(event, 1, true);
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            self.send_mouse_button(event, 1, false);
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            self.send_mouse_button(event, mac_other_button(event), true);
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            self.send_mouse_button(event, mac_other_button(event), false);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            self.send_scroll(event);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.send_key(event, true);
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            self.send_key(event, false);
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            self.send_flags_changed(event);
        }
    }
);

impl TalosRemoteDesktopView {
    unsafe fn init_with_frame(
        this: Allocated<Self>,
        frame: NSRect,
        control_state: ControlState,
    ) -> Retained<Self> {
        let this = this.set_ivars(TalosRemoteDesktopViewIvars {
            control_state,
            cutouts: RefCell::new(Vec::new()),
            focused: Cell::new(false),
            last_mouse_move_at: Cell::new(None),
            modifier_mask: Cell::new(0),
            tracking_area: RefCell::new(None),
        });
        msg_send![super(this), initWithFrame: frame]
    }

    fn set_cutouts(&self, cutouts: Vec<ViewportCutout>) {
        *self.ivars().cutouts.borrow_mut() = cutouts;
    }

    fn point_is_cutout(&self, point: NSPoint) -> bool {
        self.ivars()
            .cutouts
            .borrow()
            .iter()
            .any(|cutout| cutout.contains(point))
    }

    fn focus(&self) {
        if let Some(window) = self.as_super().window() {
            window.makeFirstResponder(Some(self.as_super().as_super()));
        }
        self.ivars().focused.set(true);
    }

    fn refresh_tracking_area(&self) {
        let mut slot = self.ivars().tracking_area.borrow_mut();
        if let Some(existing) = slot.take() {
            self.as_super().removeTrackingArea(&existing);
        }

        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::InVisibleRect
            | NSTrackingAreaOptions::EnabledDuringMouseDrag;
        let owner = unsafe { &*(self as *const Self).cast::<AnyObject>() };
        let tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                self.as_super().bounds(),
                options,
                Some(owner),
                None,
            )
        };
        self.as_super().addTrackingArea(&tracking);
        *slot = Some(tracking);
    }

    fn send_mouse_move(&self, event: &NSEvent) {
        if !self.ivars().focused.get() {
            return;
        }
        let now = Instant::now();
        if self
            .ivars()
            .last_mouse_move_at
            .get()
            .is_some_and(|last| now.duration_since(last) < Duration::from_millis(8))
        {
            return;
        }
        self.ivars().last_mouse_move_at.set(Some(now));
        let Some((x, y, element_width, element_height)) = self.event_position(event) else {
            return;
        };
        self.send_control(ControlEvent::MouseMove {
            x,
            y,
            element_width,
            element_height,
        });
    }

    fn send_mouse_button(&self, event: &NSEvent, button: u8, down: bool) {
        self.focus();
        let Some((x, y, element_width, element_height)) = self.event_position(event) else {
            return;
        };
        self.send_control(ControlEvent::MouseButton {
            button,
            down,
            x,
            y,
            element_width,
            element_height,
        });
    }

    fn send_scroll(&self, event: &NSEvent) {
        if !self.ivars().focused.get() {
            return;
        }
        let Some((x, y, element_width, element_height)) = self.event_position(event) else {
            return;
        };
        let delta_y = event.scrollingDeltaY();
        let delta = if delta_y == 0.0 {
            0
        } else if delta_y > 0.0 {
            120
        } else {
            -120
        };
        if delta == 0 {
            return;
        }
        self.send_control(ControlEvent::MouseWheel {
            delta,
            x,
            y,
            element_width,
            element_height,
        });
    }

    fn send_key(&self, event: &NSEvent, down: bool) {
        self.ivars()
            .modifier_mask
            .set(control_modifiers(event.modifierFlags()));
        let Some(vkey) = mac_key_to_windows_vkey(event.keyCode()) else {
            return;
        };
        let modifiers = self.ivars().modifier_mask.get();
        let event = if down {
            ControlEvent::KeyDown {
                vkey,
                scan: 0,
                modifiers,
            }
        } else {
            ControlEvent::KeyUp {
                vkey,
                scan: 0,
                modifiers,
            }
        };
        self.send_control(event);
    }

    fn send_flags_changed(&self, event: &NSEvent) {
        let old_mask = self.ivars().modifier_mask.get();
        let new_mask = control_modifiers(event.modifierFlags());
        self.ivars().modifier_mask.set(new_mask);
        let Some((bit, vkey)) = mac_modifier_key(event.keyCode()) else {
            return;
        };
        let was_down = old_mask & bit != 0;
        let is_down = new_mask & bit != 0;
        if was_down == is_down {
            return;
        }
        let event = if is_down {
            ControlEvent::KeyDown {
                vkey,
                scan: 0,
                modifiers: new_mask,
            }
        } else {
            ControlEvent::KeyUp {
                vkey,
                scan: 0,
                modifiers: new_mask,
            }
        };
        self.send_control(event);
    }

    fn event_position(&self, event: &NSEvent) -> Option<(u32, u32, u32, u32)> {
        let bounds = self.as_super().bounds();
        let width = bounds.size.width.round().max(0.0) as u32;
        let height = bounds.size.height.round().max(0.0) as u32;
        if width == 0 || height == 0 {
            return None;
        }
        let point = self
            .as_super()
            .convertPoint_fromView(event.locationInWindow(), None);
        if self.point_is_cutout(point) {
            return None;
        }
        let x = point.x.round().clamp(0.0, width as f64) as u32;
        let y = point.y.round().clamp(0.0, height as f64) as u32;
        Some((x, y, width.max(1), height.max(1)))
    }

    fn send_control(&self, event: ControlEvent) {
        send_native_control_with_control_state(&self.ivars().control_state, event);
    }
}

fn mac_other_button(event: &NSEvent) -> u8 {
    match event.buttonNumber() {
        1 => 2,
        value if value >= 0 => value as u8,
        _ => 2,
    }
}

fn control_modifiers(flags: NSEventModifierFlags) -> u8 {
    let mut modifiers = 0u8;
    if flags.contains(NSEventModifierFlags::Control) {
        modifiers |= CONTROL_MOD_CTRL;
    }
    if flags.contains(NSEventModifierFlags::Shift) {
        modifiers |= CONTROL_MOD_SHIFT;
    }
    if flags.contains(NSEventModifierFlags::Option) {
        modifiers |= CONTROL_MOD_ALT;
    }
    if flags.contains(NSEventModifierFlags::Command) {
        modifiers |= CONTROL_MOD_WIN;
    }
    modifiers
}

fn mac_modifier_key(key_code: u16) -> Option<(u8, u16)> {
    match key_code {
        56 | 60 => Some((CONTROL_MOD_SHIFT, 0x10)),
        59 | 62 => Some((CONTROL_MOD_CTRL, 0x11)),
        58 | 61 => Some((CONTROL_MOD_ALT, 0x12)),
        54 | 55 => Some((CONTROL_MOD_WIN, 0x5b)),
        _ => None,
    }
}

fn mac_key_to_windows_vkey(key_code: u16) -> Option<u16> {
    let vkey = match key_code {
        0 => 0x41,
        1 => 0x53,
        2 => 0x44,
        3 => 0x46,
        4 => 0x48,
        5 => 0x47,
        6 => 0x5a,
        7 => 0x58,
        8 => 0x43,
        9 => 0x56,
        11 => 0x42,
        12 => 0x51,
        13 => 0x57,
        14 => 0x45,
        15 => 0x52,
        16 => 0x59,
        17 => 0x54,
        18 => 0x31,
        19 => 0x32,
        20 => 0x33,
        21 => 0x34,
        22 => 0x36,
        23 => 0x35,
        24 => 0xbb,
        25 => 0x39,
        26 => 0x37,
        27 => 0xbd,
        28 => 0x38,
        29 => 0x30,
        30 => 0xdd,
        31 => 0x4f,
        32 => 0x55,
        33 => 0xdb,
        34 => 0x49,
        35 => 0x50,
        36 | 76 => 0x0d,
        37 => 0x4c,
        38 => 0x4a,
        39 => 0xde,
        40 => 0x4b,
        41 => 0xba,
        42 => 0xdc,
        43 => 0xbc,
        44 => 0xbf,
        45 => 0x4e,
        46 => 0x4d,
        47 => 0xbe,
        48 => 0x09,
        49 => 0x20,
        50 => 0xc0,
        51 => 0x08,
        53 => 0x1b,
        54 | 55 => 0x5b,
        56 | 60 => 0x10,
        58 | 61 => 0x12,
        59 | 62 => 0x11,
        65 => 0x6e,
        67 => 0x6a,
        69 => 0x6b,
        71 => 0x0c,
        75 => 0x6f,
        78 => 0x6d,
        81 => 0xbb,
        82 => 0x60,
        83 => 0x61,
        84 => 0x62,
        85 => 0x63,
        86 => 0x64,
        87 => 0x65,
        88 => 0x66,
        89 => 0x67,
        91 => 0x68,
        92 => 0x69,
        96 => 0x74,
        97 => 0x75,
        98 => 0x76,
        99 => 0x72,
        100 => 0x77,
        101 => 0x78,
        103 => 0x7a,
        105 => 0x7c,
        107 => 0x7d,
        109 => 0x79,
        111 => 0x7b,
        113 => 0x7e,
        114 => 0x2d,
        115 => 0x24,
        116 => 0x21,
        117 => 0x2e,
        118 => 0x73,
        119 => 0x23,
        120 => 0x71,
        121 => 0x22,
        122 => 0x70,
        123 => 0x25,
        124 => 0x27,
        125 => 0x28,
        126 => 0x26,
        _ => return None,
    };
    Some(vkey)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox_rect_preserves_source_aspect() {
        assert_eq!(letterbox_rect(1920, 1080, 1280, 720), (0, 0, 1920, 1080));
        assert_eq!(letterbox_rect(1920, 1080, 1024, 768), (240, 0, 1440, 1080));
        assert_eq!(letterbox_rect(1024, 768, 1920, 1080), (0, 96, 1024, 576));
    }

    #[test]
    fn native_frame_from_dom_rect_respects_parent_coordinate_orientation() {
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1000.0, 800.0));

        let flipped = native_frame_from_dom_rect(bounds, true, 0, 50, 1000, 750);
        assert_eq!(flipped.origin.y, 50.0);
        assert_eq!(flipped.size.height, 750.0);

        let unflipped = native_frame_from_dom_rect(bounds, false, 0, 50, 1000, 750);
        assert_eq!(unflipped.origin.y, 0.0);
        assert_eq!(unflipped.size.height, 750.0);
    }

    #[test]
    fn native_frame_from_dom_rect_bottom_aligns_trailing_native_gap() {
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1000.0, 800.0));

        let flipped = native_frame_from_dom_rect(bounds, true, 0, 50, 1000, 720);
        assert_eq!(flipped.origin.y, 80.0);
        assert_eq!(flipped.size.height, 720.0);

        let unflipped = native_frame_from_dom_rect(bounds, false, 0, 50, 1000, 720);
        assert_eq!(unflipped.origin.y, 0.0);
        assert_eq!(unflipped.size.height, 720.0);
    }

    #[test]
    fn viewport_cutouts_map_window_occlusions_into_viewport_space() {
        let occlusions = vec![ViewportOcclusionRect {
            x: 90,
            y: 60,
            width: 40,
            height: 30,
        }];

        let cutouts = viewport_cutouts_from_window_occlusions(100, 50, 300.0, 200.0, &occlusions);

        assert_eq!(
            cutouts,
            vec![ViewportCutout {
                x: 0.0,
                y: 9.0,
                width: 31.0,
                height: 32.0,
            }]
        );
    }

    #[test]
    fn viewport_cutout_contains_points_inside_expanded_occlusion() {
        let cutout = ViewportCutout {
            x: 10.0,
            y: 20.0,
            width: 30.0,
            height: 40.0,
        };

        assert!(cutout.contains(NSPoint::new(10.0, 20.0)));
        assert!(cutout.contains(NSPoint::new(39.0, 59.0)));
        assert!(!cutout.contains(NSPoint::new(40.0, 60.0)));
        assert!(!cutout.contains(NSPoint::new(9.0, 20.0)));
    }

    #[test]
    fn layer_mask_rect_uses_flipped_view_coordinates() {
        let rect = layer_mask_rect_for_cutout(ViewportCutout {
            x: 10.0,
            y: 20.0,
            width: 30.0,
            height: 40.0,
        });

        assert_eq!(rect.origin.x, 10.0);
        assert_eq!(rect.origin.y, 20.0);
        assert_eq!(rect.size.width, 30.0);
        assert_eq!(rect.size.height, 40.0);
    }

    #[test]
    fn maps_common_macos_keys_to_windows_vkeys() {
        assert_eq!(mac_key_to_windows_vkey(0), Some(0x41));
        assert_eq!(mac_key_to_windows_vkey(36), Some(0x0d));
        assert_eq!(mac_key_to_windows_vkey(123), Some(0x25));
        assert_eq!(mac_key_to_windows_vkey(126), Some(0x26));
        assert_eq!(mac_key_to_windows_vkey(117), Some(0x2e));
    }

    #[test]
    fn maps_macos_modifier_keys_to_protocol_modifiers() {
        assert_eq!(mac_modifier_key(56), Some((CONTROL_MOD_SHIFT, 0x10)));
        assert_eq!(mac_modifier_key(59), Some((CONTROL_MOD_CTRL, 0x11)));
        assert_eq!(mac_modifier_key(58), Some((CONTROL_MOD_ALT, 0x12)));
        assert_eq!(mac_modifier_key(55), Some((CONTROL_MOD_WIN, 0x5b)));
    }
}
