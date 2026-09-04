use std::num::NonZeroU32;
use std::ptr;
use std::slice;
use std::time::Instant;

use softbuffer::Rect;
use tracing::{debug, error, warn};
use vpx_sys::*;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromWindow, DEVMODEW, ENUM_CURRENT_SETTINGS,
    MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};

use crate::{
    display_processing::{effective_display_processing_mode, DisplayProcessingMode},
    viewport_d3d11::{D3d11Viewport, ExperimentalCompositeStats, ExperimentalMoveRect},
    ViewportInner,
};

pub(crate) struct CachedFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) argb: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameDamageRect {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) struct DecodedFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) fps: u32,
    pub(crate) argb: Vec<u32>,
}

pub(crate) struct Vp8Decoder {
    ctx: vpx_codec_ctx,
    iter: vpx_codec_iter_t,
}

unsafe impl Send for Vp8Decoder {}

impl Vp8Decoder {
    pub(crate) fn new() -> Result<Self, String> {
        let mut ctx = std::mem::MaybeUninit::uninit();
        let cfg = std::mem::MaybeUninit::zeroed();
        let ret = unsafe {
            vpx_codec_dec_init_ver(
                ctx.as_mut_ptr(),
                vpx_codec_vp8_dx(),
                cfg.as_ptr(),
                0,
                VPX_DECODER_ABI_VERSION as i32,
            )
        };
        if ret != vpx_codec_err_t::VPX_CODEC_OK {
            return Err("VP8 decoder init failed".to_string());
        }
        Ok(Self {
            ctx: unsafe { ctx.assume_init() },
            iter: ptr::null(),
        })
    }

    pub(crate) fn decode(&mut self, payload: &[u8]) -> Result<Option<DecodedFrame>, String> {
        let ret = unsafe {
            vpx_codec_decode(
                &mut self.ctx,
                payload.as_ptr(),
                payload.len() as u32,
                ptr::null_mut(),
                0,
            )
        };
        self.iter = ptr::null();
        if ret != vpx_codec_err_t::VPX_CODEC_OK {
            return Err(vpx_error_to_str(&mut self.ctx));
        }
        let img_ptr = unsafe { vpx_codec_get_frame(&mut self.ctx, &mut self.iter) };
        if img_ptr.is_null() {
            return Ok(None);
        }
        let img = unsafe { *img_ptr };
        if img.fmt != vpx_img_fmt::VPX_IMG_FMT_I420 {
            return Err("unsupported VP8 pixel format".to_string());
        }
        let width = img.d_w as u32;
        let height = img.d_h as u32;
        let y_stride = img.stride[0] as usize;
        let u_stride = img.stride[1] as usize;
        let v_stride = img.stride[2] as usize;
        let y_len = y_stride * height as usize;
        let uv_height = height.div_ceil(2) as usize;
        let u_len = u_stride * uv_height;
        let v_len = v_stride * uv_height;
        let y = unsafe { slice::from_raw_parts(img.planes[0] as *const u8, y_len) };
        let u = unsafe { slice::from_raw_parts(img.planes[1] as *const u8, u_len) };
        let v = unsafe { slice::from_raw_parts(img.planes[2] as *const u8, v_len) };
        let argb = convert_i420_to_argb(y, u, v, y_stride, u_stride, v_stride, width, height);

        Ok(Some(DecodedFrame {
            width,
            height,
            fps: 0,
            argb,
        }))
    }
}

impl Drop for Vp8Decoder {
    fn drop(&mut self) {
        unsafe { vpx_codec_destroy(&mut self.ctx) };
    }
}
fn vpx_error_to_str(ctx: &mut vpx_codec_ctx) -> String {
    unsafe {
        let c_str = vpx_codec_error(ctx);
        if c_str.is_null() {
            "libvpx error".to_string()
        } else {
            std::ffi::CStr::from_ptr(c_str)
                .to_string_lossy()
                .into_owned()
        }
    }
}

pub(crate) fn parse_ivf_header(header: &[u8]) -> Option<(u32, u32, u32)> {
    if header.len() < 24 {
        return None;
    }
    let width = u16::from_le_bytes([header[12], header[13]]) as u32;
    let height = u16::from_le_bytes([header[14], header[15]]) as u32;
    let fps_num = u32::from_le_bytes([header[16], header[17], header[18], header[19]]);
    let fps_den = u32::from_le_bytes([header[20], header[21], header[22], header[23]]);
    let fps = if fps_num == 0 {
        0
    } else if fps_den <= 1 {
        fps_num
    } else {
        let value = fps_num / fps_den;
        if value == 0 {
            1
        } else {
            value
        }
    };
    if width == 0 || height == 0 {
        None
    } else {
        Some((width, height, fps))
    }
}

pub(crate) fn present_decoded_frame(
    inner: &mut ViewportInner,
    frame: &DecodedFrame,
) -> Result<(), String> {
    let mode = effective_display_processing_mode("viewer viewport present");
    debug!(
        mode = ?mode,
        frame_width = frame.width,
        frame_height = frame.height,
        fps = frame.fps,
        has_last_rect = inner.last_rect.is_some(),
        has_surface = inner.surface.is_some(),
        has_gpu_viewport = inner.gpu_viewport.is_some(),
        gpu_disabled = inner.gpu_disabled,
        "viewer viewport present decoded frame"
    );
    if mode.prefers_gpu() {
        if ensure_gpu_viewport(inner, "viewer viewport present")? {
            present_decoded_frame_gpu(inner, frame)
        } else {
            present_decoded_frame_cpu(inner, frame)
        }
    } else {
        present_decoded_frame_cpu(inner, frame)
    }
}

pub(crate) fn ensure_gpu_viewport(
    inner: &mut ViewportInner,
    context: &'static str,
) -> Result<bool, String> {
    let mode = effective_display_processing_mode(context);
    if !mode.prefers_gpu() {
        return Ok(false);
    }
    if inner.gpu_disabled {
        if mode.allows_cpu_fallback() {
            return Ok(false);
        }
        return Err(
            "viewer GPU compositor is disabled after a prior strict GPU failure".to_string(),
        );
    }
    let Some(child_hwnd) = inner.child_hwnd else {
        if mode.allows_cpu_fallback() {
            return Ok(false);
        }
        return Err("viewer GPU compositor requires a child window in strict GPU mode".to_string());
    };
    if inner.gpu_viewport.is_none() {
        match D3d11Viewport::new(child_hwnd) {
            Ok(viewport) => {
                debug!(
                    context,
                    driver = viewport.driver_label(),
                    strict_gpu = !mode.allows_cpu_fallback(),
                    "viewer GPU compositor initialized"
                );
                inner.gpu_viewport = Some(viewport);
            }
            Err(err) => {
                inner.gpu_disabled = true;
                if mode.allows_cpu_fallback() {
                    warn!(context, error = %err, "viewer GPU compositor init failed; falling back to CPU");
                    return Ok(false);
                }
                error!(
                    context,
                    error = %err,
                    "viewer GPU compositor init failed; strict GPU mode forbids CPU fallback"
                );
                return Err(format!(
                    "viewer GPU compositor init failed in strict GPU mode: {err}"
                ));
            }
        }
    }
    Ok(true)
}

pub(crate) fn present_decoded_frame_gpu(
    inner: &mut ViewportInner,
    frame: &DecodedFrame,
) -> Result<(), String> {
    let present_start = Instant::now();
    if let Some(last_move) = inner.last_move_event {
        if present_start.duration_since(last_move) < std::time::Duration::from_millis(50) {
            debug!(
                elapsed_ms = present_start.duration_since(last_move).as_millis(),
                "viewer GPU present skipped after viewport move"
            );
            return Ok(());
        }
    }
    let Some((_, _, viewport_w, viewport_h)) = inner.last_rect else {
        debug!("viewer GPU present skipped; viewport rect missing");
        return Ok(());
    };
    let target_fps = match (frame.fps, inner.child_hwnd) {
        (0, Some(hwnd)) => monitor_refresh_rate(hwnd).unwrap_or(0),
        (fps, Some(hwnd)) => match monitor_refresh_rate(hwnd) {
            Some(viewer_hz) => fps.min(viewer_hz),
            None => fps,
        },
        (fps, None) => fps,
    };
    if target_fps > 0 {
        let now = Instant::now();
        if let Some(last) = inner.last_present {
            let min_delta = std::time::Duration::from_secs_f64(1.0 / target_fps as f64);
            if now.duration_since(last) < min_delta {
                debug!(
                    target_fps,
                    elapsed_ms = now.duration_since(last).as_millis(),
                    "viewer GPU present skipped by frame throttle"
                );
                return Ok(());
            }
        }
        inner.last_present = Some(now);
    }
    let Some(viewport) = inner.gpu_viewport.as_mut() else {
        debug!("viewer GPU present missing compositor; falling back to CPU");
        return present_decoded_frame_cpu(inner, frame);
    };
    debug!(
        frame_width = frame.width,
        frame_height = frame.height,
        viewport_w,
        viewport_h,
        "viewer GPU present upload start"
    );
    viewport.upload_full_argb_words(frame.width, frame.height, &frame.argb)?;
    viewport.present(viewport_w, viewport_h)?;
    cache_decoded_frame(inner, frame);
    debug!(
        frame_width = frame.width,
        frame_height = frame.height,
        viewport_w,
        viewport_h,
        "viewer GPU present completed"
    );
    Ok(())
}

pub(crate) fn present_decoded_frame_cpu(
    inner: &mut ViewportInner,
    frame: &DecodedFrame,
) -> Result<(), String> {
    if let Some(viewport) = inner.gpu_viewport.as_mut() {
        viewport.clear_desktop_frame();
    }
    let present_start = Instant::now();
    if let Some(last_move) = inner.last_move_event {
        if present_start.duration_since(last_move) < std::time::Duration::from_millis(50) {
            debug!(
                elapsed_ms = present_start.duration_since(last_move).as_millis(),
                "viewer CPU present skipped after viewport move"
            );
            return Ok(());
        }
    }
    let Some((_, _, viewport_w, viewport_h)) = inner.last_rect else {
        debug!("viewer CPU present skipped; viewport rect missing");
        return Ok(());
    };
    let Some(surface) = inner.surface.as_mut() else {
        debug!("viewer CPU present skipped; softbuffer surface missing");
        return Ok(());
    };
    let target_fps = match (frame.fps, inner.child_hwnd) {
        (0, Some(hwnd)) => monitor_refresh_rate(hwnd).unwrap_or(0),
        (fps, Some(hwnd)) => match monitor_refresh_rate(hwnd) {
            Some(viewer_hz) => fps.min(viewer_hz),
            None => fps,
        },
        (fps, None) => fps,
    };
    if target_fps > 0 {
        let now = Instant::now();
        if let Some(last) = inner.last_present {
            let min_delta = std::time::Duration::from_secs_f64(1.0 / target_fps as f64);
            if now.duration_since(last) < min_delta {
                debug!(
                    target_fps,
                    elapsed_ms = now.duration_since(last).as_millis(),
                    "viewer CPU present skipped by frame throttle"
                );
                return Ok(());
            }
        }
        inner.last_present = Some(now);
    }
    let size_changed = inner.last_size != Some((viewport_w, viewport_h));
    if size_changed {
        surface
            .resize(
                NonZeroU32::new(viewport_w).ok_or("invalid viewport width")?,
                NonZeroU32::new(viewport_h).ok_or("invalid viewport height")?,
            )
            .map_err(|err| format!("viewport resize failed: {err}"))?;
        inner.last_size = Some((viewport_w, viewport_h));
        debug!(
            viewport_w,
            viewport_h, "viewer CPU present resized softbuffer surface"
        );
    }

    debug!(
        frame_width = frame.width,
        frame_height = frame.height,
        viewport_w,
        viewport_h,
        "viewer CPU present compositing frame"
    );
    let mut buffer = surface
        .buffer_mut()
        .map_err(|err| format!("viewport buffer failed: {err}"))?;
    scale_argb_letterbox(
        &frame.argb,
        frame.width,
        frame.height,
        &mut buffer,
        viewport_w,
        viewport_h,
    );

    buffer
        .present()
        .map_err(|err| format!("viewport present failed: {err}"))?;
    debug!(
        frame_width = frame.width,
        frame_height = frame.height,
        viewport_w,
        viewport_h,
        "viewer CPU present completed"
    );

    // Cache a recent frame so we can repaint immediately on window resizes without
    // waiting for the next decoded frame (prevents "needs click to refresh").
    cache_decoded_frame(inner, frame);

    Ok(())
}

fn cache_decoded_frame(inner: &mut ViewportInner, frame: &DecodedFrame) {
    let cache_now = Instant::now();
    let should_cache = inner
        .last_cache_at
        .map(|last| cache_now.duration_since(last) > std::time::Duration::from_millis(250))
        .unwrap_or(true);
    if should_cache {
        inner.cached_frame = Some(CachedFrame {
            width: frame.width,
            height: frame.height,
            argb: frame.argb.clone(),
        });
        inner.last_cache_at = Some(cache_now);
    }
}

pub(crate) fn present_cached_frame(inner: &mut ViewportInner) -> Result<(), String> {
    debug!(
        has_cached_frame = inner.cached_frame.is_some(),
        has_gpu_viewport = inner.gpu_viewport.is_some(),
        has_gpu_desktop_frame = inner
            .gpu_viewport
            .as_ref()
            .map(|viewport| viewport.has_desktop_frame())
            .unwrap_or(false),
        "viewer viewport cached present requested"
    );
    if ensure_gpu_viewport(inner, "viewer viewport cached present")?
        && inner
            .gpu_viewport
            .as_ref()
            .map(|viewport| viewport.has_desktop_frame())
            .unwrap_or(false)
    {
        present_cached_frame_with_damage(inner, &[], true)?;
        return Ok(());
    }
    let Some(cached) = inner.cached_frame.as_ref() else {
        return Ok(());
    };
    let damage = [FrameDamageRect {
        x: 0,
        y: 0,
        width: cached.width,
        height: cached.height,
    }];
    present_cached_frame_with_damage(inner, &damage, true)?;
    Ok(())
}

pub(crate) fn present_cached_frame_with_damage(
    inner: &mut ViewportInner,
    source_damage: &[FrameDamageRect],
    force_full_redraw: bool,
) -> Result<(), String> {
    let mode = effective_display_processing_mode("viewer viewport composite");
    debug!(
        mode = ?mode,
        damage_rects = source_damage.len(),
        force_full_redraw,
        has_cached_frame = inner.cached_frame.is_some(),
        has_last_rect = inner.last_rect.is_some(),
        has_surface = inner.surface.is_some(),
        has_gpu_viewport = inner.gpu_viewport.is_some(),
        "viewer viewport composite requested"
    );
    if mode.prefers_gpu() {
        if ensure_gpu_viewport(inner, "viewer viewport composite")? {
            let has_gpu_frame = inner
                .gpu_viewport
                .as_ref()
                .map(|viewport| viewport.has_desktop_frame())
                .unwrap_or(false);
            if has_gpu_frame {
                present_cached_frame_with_damage_gpu(inner, source_damage, force_full_redraw)
            } else if mode.allows_cpu_fallback() {
                present_cached_frame_with_damage_cpu(inner, source_damage, force_full_redraw)
            } else {
                present_cached_frame_with_damage_gpu(inner, source_damage, force_full_redraw)
            }
        } else {
            present_cached_frame_with_damage_cpu(inner, source_damage, force_full_redraw)
        }
    } else {
        present_cached_frame_with_damage_cpu(inner, source_damage, force_full_redraw)
    }
}

pub(crate) fn present_cached_frame_with_damage_gpu(
    inner: &mut ViewportInner,
    source_damage: &[FrameDamageRect],
    force_full_redraw: bool,
) -> Result<(), String> {
    let _ = (source_damage, force_full_redraw);
    let Some((_, _, viewport_w, viewport_h)) = inner.last_rect else {
        debug!("viewer GPU cached present skipped; viewport rect missing");
        return Ok(());
    };
    let Some(viewport) = inner.gpu_viewport.as_mut() else {
        debug!("viewer GPU cached present skipped; GPU viewport missing");
        return Ok(());
    };
    if !viewport.has_desktop_frame() {
        debug!("viewer GPU cached present skipped; desktop frame missing");
        return Ok(());
    }
    viewport.present(viewport_w, viewport_h)?;
    debug!(
        viewport_w,
        viewport_h, "viewer GPU cached present completed"
    );
    Ok(())
}

pub(crate) fn present_experimental_atlas_commands_gpu(
    inner: &mut ViewportInner,
    desktop_width: u32,
    desktop_height: u32,
    atlas_width: u32,
    atlas_height: u32,
    rects: &[talos_protocol::DisplayAtlasRect],
    moves: &[ExperimentalMoveRect],
    tile_commands: &[u8],
    present_after_composite: bool,
) -> Result<ExperimentalPresentResult, String> {
    let mode = effective_display_processing_mode("viewer experimental display composite");
    if !mode.prefers_gpu() {
        return Err("viewer experimental display requires GPU processing".to_string());
    }
    if inner.child_hwnd.is_none() {
        debug!("viewer experimental GPU present deferred; child viewport window missing");
        return Ok(ExperimentalPresentResult::Deferred(
            "child viewport window missing",
        ));
    }
    if !ensure_gpu_viewport(inner, "viewer experimental display composite")? {
        if inner.gpu_disabled {
            return Err(
                "viewer experimental display requires an available GPU viewport".to_string(),
            );
        }
        debug!("viewer experimental GPU present deferred; GPU viewport not ready");
        return Ok(ExperimentalPresentResult::Deferred(
            "GPU viewport not ready",
        ));
    }
    let viewport_rect = if present_after_composite {
        let Some(rect) = inner.last_rect else {
            debug!("viewer experimental GPU present deferred; viewport rect missing");
            return Ok(ExperimentalPresentResult::Deferred("viewport rect missing"));
        };
        Some(rect)
    } else {
        None
    };
    let Some(viewport) = inner.gpu_viewport.as_mut() else {
        debug!("viewer experimental GPU present deferred; GPU viewport missing after init");
        return Ok(ExperimentalPresentResult::Deferred("GPU viewport missing"));
    };
    if !viewport.has_experimental_desktop_frame(desktop_width, desktop_height) {
        if let Some(cached) = inner
            .cached_frame
            .as_ref()
            .filter(|cached| cached.width == desktop_width && cached.height == desktop_height)
        {
            viewport.upload_full_argb_words_as_experimental(
                cached.width,
                cached.height,
                &cached.argb,
            )?;
            debug!(
                desktop_width,
                desktop_height, "viewer experimental GPU seeded ATX2 desktop from cached frame"
            );
        }
    }

    let composite = viewport.composite_experimental_atlas_commands(
        desktop_width,
        desktop_height,
        atlas_width,
        atlas_height,
        rects,
        moves,
        tile_commands,
    )?;
    let present = if present_after_composite {
        let Some((_, _, viewport_w, viewport_h)) = viewport_rect else {
            return Ok(ExperimentalPresentResult::Deferred("viewport rect missing"));
        };
        let present_started = Instant::now();
        viewport.present(viewport_w, viewport_h)?;
        inner.last_present = Some(Instant::now());
        present_started.elapsed()
    } else {
        std::time::Duration::ZERO
    };
    debug!(
        desktop_width,
        desktop_height,
        atlas_width,
        atlas_height,
        dirty_rects = rects.len(),
        move_rects = moves.len(),
        command_bytes = tile_commands.len(),
        command_count = composite.command_count,
        present_after_composite,
        "viewer experimental GPU present completed"
    );
    Ok(ExperimentalPresentResult::Presented(
        ExperimentalPresentStats { composite, present },
    ))
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ExperimentalPresentResult {
    Presented(ExperimentalPresentStats),
    Deferred(&'static str),
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ExperimentalPresentStats {
    pub(crate) composite: ExperimentalCompositeStats,
    pub(crate) present: std::time::Duration,
}

pub(crate) fn present_cached_frame_with_damage_cpu(
    inner: &mut ViewportInner,
    source_damage: &[FrameDamageRect],
    force_full_redraw: bool,
) -> Result<(), String> {
    let Some(cached) = inner.cached_frame.as_ref() else {
        debug!("viewer CPU cached present skipped; cached frame missing");
        return Ok(());
    };
    let Some((_, _, viewport_w, viewport_h)) = inner.last_rect else {
        debug!("viewer CPU cached present skipped; viewport rect missing");
        return Ok(());
    };
    let Some(surface) = inner.surface.as_mut() else {
        debug!("viewer CPU cached present skipped; softbuffer surface missing");
        return Ok(());
    };

    let size_changed = inner.last_size != Some((viewport_w, viewport_h));
    if size_changed {
        surface
            .resize(
                NonZeroU32::new(viewport_w).ok_or("invalid viewport width")?,
                NonZeroU32::new(viewport_h).ok_or("invalid viewport height")?,
            )
            .map_err(|err| format!("viewport resize failed: {err}"))?;
        inner.last_size = Some((viewport_w, viewport_h));
        debug!(
            viewport_w,
            viewport_h, "viewer CPU cached present resized softbuffer surface"
        );
    }

    let transform =
        compute_letterbox_transform(cached.width, cached.height, viewport_w, viewport_h);
    let mut buffer = surface
        .buffer_mut()
        .map_err(|err| format!("viewport buffer failed: {err}"))?;
    let buffer_age = buffer.age();
    let full_redraw =
        force_full_redraw || size_changed || buffer_age == 0 || source_damage.is_empty();
    if full_redraw {
        debug!(
            cached_width = cached.width,
            cached_height = cached.height,
            viewport_w,
            viewport_h,
            buffer_age,
            "viewer CPU cached present full redraw"
        );
        scale_argb_letterbox(
            &cached.argb,
            cached.width,
            cached.height,
            &mut buffer,
            viewport_w,
            viewport_h,
        );
        buffer
            .present()
            .map_err(|err| format!("viewport present failed: {err}"))?;
        debug!(
            viewport_w,
            viewport_h, "viewer CPU cached present completed full redraw"
        );
        return Ok(());
    }

    let damage_rects = apply_letterbox_damage(
        &cached.argb,
        cached.width,
        cached.height,
        &mut buffer,
        viewport_w,
        viewport_h,
        &transform,
        source_damage,
    );
    if damage_rects.is_empty() {
        debug!(
            source_damage_rects = source_damage.len(),
            "viewer CPU cached present skipped; mapped damage empty"
        );
        return Ok(());
    }
    debug!(
        source_damage_rects = source_damage.len(),
        mapped_damage_rects = damage_rects.len(),
        buffer_age,
        "viewer CPU cached present damage redraw"
    );
    buffer
        .present_with_damage(&damage_rects)
        .map_err(|err| format!("viewport present with damage failed: {err}"))?;
    debug!(
        mapped_damage_rects = damage_rects.len(),
        "viewer CPU cached present completed damage redraw"
    );
    Ok(())
}

#[derive(Clone, Copy)]
struct LetterboxTransform {
    offset_x: u32,
    offset_y: u32,
    scaled_w: u32,
    scaled_h: u32,
}

fn compute_letterbox_transform(
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> LetterboxTransform {
    let scale_x = dst_w as f64 / src_w as f64;
    let scale_y = dst_h as f64 / src_h as f64;
    let scale = scale_x.min(scale_y);
    let scaled_w = (src_w as f64 * scale).round().max(1.0) as u32;
    let scaled_h = (src_h as f64 * scale).round().max(1.0) as u32;
    LetterboxTransform {
        offset_x: dst_w.saturating_sub(scaled_w) / 2,
        offset_y: dst_h.saturating_sub(scaled_h) / 2,
        scaled_w,
        scaled_h,
    }
}

fn apply_letterbox_damage(
    src: &[u32],
    src_w: u32,
    src_h: u32,
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
    transform: &LetterboxTransform,
    source_damage: &[FrameDamageRect],
) -> Vec<Rect> {
    match effective_display_processing_mode("viewer viewport damage scale") {
        DisplayProcessingMode::Legacy
        | DisplayProcessingMode::Auto
        | DisplayProcessingMode::Gpu => apply_letterbox_damage_cpu(
            src,
            src_w,
            src_h,
            dst,
            dst_w,
            dst_h,
            transform,
            source_damage,
        ),
    }
}

fn apply_letterbox_damage_cpu(
    src: &[u32],
    src_w: u32,
    src_h: u32,
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
    transform: &LetterboxTransform,
    source_damage: &[FrameDamageRect],
) -> Vec<Rect> {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return Vec::new();
    }
    let src_w_usize = src_w as usize;
    let dst_w_usize = dst_w as usize;
    let mut damage_rects = Vec::new();
    for damage in source_damage {
        let Some(mapped) = map_source_damage_to_dest(*damage, src_w, src_h, transform) else {
            continue;
        };
        let x0 = mapped.x as usize;
        let y0 = mapped.y as usize;
        let x1 = x0 + mapped.width.get() as usize;
        let y1 = y0 + mapped.height.get() as usize;
        for dy in y0..y1 {
            let sy = (((dy as u32).saturating_sub(transform.offset_y) as u64) * src_h as u64
                / transform.scaled_h as u64) as usize;
            let sy = sy.min(src_h.saturating_sub(1) as usize);
            let src_row = sy * src_w_usize;
            let dst_row = dy * dst_w_usize;
            for dx in x0..x1 {
                let sx = (((dx as u32).saturating_sub(transform.offset_x) as u64) * src_w as u64
                    / transform.scaled_w as u64) as usize;
                let sx = sx.min(src_w.saturating_sub(1) as usize);
                dst[dst_row + dx] = src[src_row + sx];
            }
        }
        damage_rects.push(mapped);
    }
    damage_rects
}

fn map_source_damage_to_dest(
    damage: FrameDamageRect,
    src_w: u32,
    src_h: u32,
    transform: &LetterboxTransform,
) -> Option<Rect> {
    if damage.width == 0 || damage.height == 0 || src_w == 0 || src_h == 0 {
        return None;
    }
    let x0 =
        transform.offset_x + ((damage.x as u64 * transform.scaled_w as u64) / src_w as u64) as u32;
    let y0 =
        transform.offset_y + ((damage.y as u64 * transform.scaled_h as u64) / src_h as u64) as u32;
    let x1 = transform.offset_x
        + (damage.x.saturating_add(damage.width) as u64 * transform.scaled_w as u64)
            .div_ceil(src_w as u64) as u32;
    let y1 = transform.offset_y
        + (damage.y.saturating_add(damage.height) as u64 * transform.scaled_h as u64)
            .div_ceil(src_h as u64) as u32;
    let width = x1.saturating_sub(x0);
    let height = y1.saturating_sub(y0);
    Some(Rect {
        x: x0,
        y: y0,
        width: NonZeroU32::new(width.max(1))?,
        height: NonZeroU32::new(height.max(1))?,
    })
}

fn convert_i420_to_argb(
    y: &[u8],
    u: &[u8],
    v: &[u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    width: u32,
    height: u32,
) -> Vec<u32> {
    match effective_display_processing_mode("viewer viewport yuv convert") {
        DisplayProcessingMode::Legacy
        | DisplayProcessingMode::Auto
        | DisplayProcessingMode::Gpu => {
            i420_to_argb_cpu(y, u, v, y_stride, u_stride, v_stride, width, height)
        }
    }
}

fn i420_to_argb_cpu(
    y: &[u8],
    u: &[u8],
    v: &[u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    width: u32,
    height: u32,
) -> Vec<u32> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u32; w * h];
    for row in 0..h {
        let y_off = row * y_stride;
        let uv_off = (row / 2) * u_stride;
        for col in 0..w {
            let y_val = y[y_off + col] as i32;
            let u_val = u[uv_off + (col / 2)] as i32;
            let v_val = v[(row / 2) * v_stride + (col / 2)] as i32;
            let c = y_val - 16;
            let d = u_val - 128;
            let e = v_val - 128;
            let r = (298 * c + 409 * e + 128) >> 8;
            let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
            let b = (298 * c + 516 * d + 128) >> 8;
            let r = r.clamp(0, 255) as u32;
            let g = g.clamp(0, 255) as u32;
            let b = b.clamp(0, 255) as u32;
            out[row * w + col] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }
    }
    out
}

fn scale_argb_letterbox(
    src: &[u32],
    src_w: u32,
    src_h: u32,
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
) {
    match effective_display_processing_mode("viewer viewport scale") {
        DisplayProcessingMode::Legacy
        | DisplayProcessingMode::Auto
        | DisplayProcessingMode::Gpu => {
            scale_argb_letterbox_cpu(src, src_w, src_h, dst, dst_w, dst_h)
        }
    }
}

fn scale_argb_letterbox_cpu(
    src: &[u32],
    src_w: u32,
    src_h: u32,
    dst: &mut [u32],
    dst_w: u32,
    dst_h: u32,
) {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return;
    }
    let src_w_usize = src_w as usize;
    let src_h_usize = src_h as usize;
    let dst_w_usize = dst_w as usize;
    dst.fill(0xFF00_0000);
    let transform = compute_letterbox_transform(src_w, src_h, dst_w, dst_h);
    let offset_x = transform.offset_x as usize;
    let offset_y = transform.offset_y as usize;
    let scaled_w_usize = transform.scaled_w as usize;
    let scaled_h_usize = transform.scaled_h as usize;
    let row_scale = src_h as u64;
    let col_scale = src_w as u64;
    for dy in 0..scaled_h_usize {
        let sy = (dy as u64 * row_scale / transform.scaled_h as u64) as usize;
        let sy = sy.min(src_h_usize.saturating_sub(1));
        let src_row = sy * src_w_usize;
        let dst_row = (dy + offset_y) * dst_w_usize;
        for dx in 0..scaled_w_usize {
            let sx = (dx as u64 * col_scale / transform.scaled_w as u64) as usize;
            let sx = sx.min(src_w_usize.saturating_sub(1));
            dst[dst_row + offset_x + dx] = src[src_row + sx];
        }
    }
}

fn monitor_refresh_rate(hwnd: HWND) -> Option<u32> {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }
    let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe { GetMonitorInfoW(monitor, &mut info as *mut _ as *mut _) != 0 };
    if !ok {
        return None;
    }
    let mut devmode = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..unsafe { std::mem::zeroed() }
    };
    let ok = unsafe {
        EnumDisplaySettingsW(info.szDevice.as_ptr(), ENUM_CURRENT_SETTINGS, &mut devmode) != 0
    };
    if !ok {
        return None;
    }
    if devmode.dmDisplayFrequency == 0 {
        None
    } else {
        Some(devmode.dmDisplayFrequency)
    }
}
