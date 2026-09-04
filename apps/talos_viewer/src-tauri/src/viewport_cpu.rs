use std::{io::Cursor, ptr, slice};

use talos_protocol::{decode_display_record, DisplayAtlasRect, DisplayRecord};
use vpx_sys::*;

#[cfg(target_os = "macos")]
use crate::vt_h264::H264Decoder;
#[cfg(target_os = "macos")]
use apple_metal::{resource_options, CommandQueue, ComputePipelineState, MetalBuffer, MetalDevice};

#[derive(Clone)]
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
        (fps_num / fps_den).max(1)
    };
    if width == 0 || height == 0 {
        None
    } else {
        Some((width, height, fps))
    }
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
    let mut argb = vec![0u32; width as usize * height as usize];
    for row in 0..height as usize {
        for col in 0..width as usize {
            let yy = y[row * y_stride + col] as i32;
            let uu = u[(row / 2) * u_stride + (col / 2)] as i32 - 128;
            let vv = v[(row / 2) * v_stride + (col / 2)] as i32 - 128;
            let c = yy - 16;
            let r = clamp((298 * c + 409 * vv + 128) >> 8);
            let g = clamp((298 * c - 100 * uu - 208 * vv + 128) >> 8);
            let b = clamp((298 * c + 516 * uu + 128) >> 8);
            argb[row * width as usize + col] =
                0xff00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
        }
    }
    argb
}

fn clamp(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

pub(crate) struct ModernDisplayCompositor {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
    current_frame_id: Option<u64>,
    #[cfg(target_os = "macos")]
    h264_decoder: Option<H264Decoder>,
    #[cfg(target_os = "macos")]
    h264_decoder_size: Option<(u32, u32)>,
    #[cfg(target_os = "macos")]
    metal_atx2: Option<MetalAtx2Decoder>,
}

impl ModernDisplayCompositor {
    pub(crate) fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            bgra: Vec::new(),
            current_frame_id: None,
            #[cfg(target_os = "macos")]
            h264_decoder: None,
            #[cfg(target_os = "macos")]
            h264_decoder_size: None,
            #[cfg(target_os = "macos")]
            metal_atx2: if std::env::var_os("TALOS_VIEWER_DISABLE_METAL_ATX2").is_some() {
                None
            } else {
                MetalAtx2Decoder::new().ok()
            },
        }
    }

    pub(crate) fn dimensions(&self) -> Option<(u32, u32)> {
        (self.width > 0 && self.height > 0).then_some((self.width, self.height))
    }

    pub(crate) fn handle_record(&mut self, record: &[u8]) -> Result<Option<DecodedFrame>, String> {
        match decode_display_record(record).map_err(|err| err.to_string())? {
            DisplayRecord::FrameBegin {
                frame_id,
                width,
                height,
            } => {
                self.begin_frame(frame_id, width, height)?;
                Ok(None)
            }
            DisplayRecord::FrameEnd { frame_id } => {
                self.ensure_frame(frame_id)?;
                self.current_frame_id = None;
                Ok(Some(self.decoded_frame()))
            }
            DisplayRecord::Keyframe {
                frame_id,
                width,
                height,
                raw_len,
                payload,
            } => {
                self.begin_frame(frame_id, width, height)?;
                let expected = frame_len(width, height)?;
                let bgra = if payload.len() == expected {
                    payload
                } else if raw_len as usize == expected {
                    zstd::stream::decode_all(Cursor::new(payload))
                        .map_err(|err| format!("decode zstd keyframe: {err}"))?
                } else {
                    return Err("display keyframe payload length mismatch".to_string());
                };
                if bgra.len() != expected {
                    return Err("display keyframe decoded length mismatch".to_string());
                }
                self.bgra.copy_from_slice(&bgra);
                Ok(None)
            }
            DisplayRecord::MoveRect {
                frame_id,
                src_x,
                src_y,
                dst_x,
                dst_y,
                width,
                height,
            } => {
                self.ensure_frame(frame_id)?;
                self.apply_move(src_x, src_y, dst_x, dst_y, width, height)?;
                Ok(None)
            }
            DisplayRecord::ExperimentalAtlasCommands {
                frame_id,
                atlas_width,
                atlas_height,
                rects,
                tile_commands,
            } => {
                self.ensure_frame(frame_id)?;
                self.apply_atx2_atlas(atlas_width, atlas_height, &rects, &tile_commands)?;
                Ok(None)
            }
            DisplayRecord::ExperimentalAtlasCommandsChunk {
                frame_id,
                flags,
                atlas_width,
                atlas_height,
                rects,
                tile_commands,
                ..
            } => {
                self.ensure_frame(frame_id)?;
                self.apply_atx2_atlas(atlas_width, atlas_height, &rects, &tile_commands)?;
                let _ = flags;
                Ok(None)
            }
            DisplayRecord::AtlasH264 {
                frame_id,
                atlas_width,
                atlas_height,
                rects,
                payload,
                ..
            } => {
                self.ensure_frame(frame_id)?;
                self.apply_h264_atlas(atlas_width, atlas_height, &rects, &payload)?;
                Ok(None)
            }
        }
    }

    fn begin_frame(&mut self, frame_id: u64, width: u32, height: u32) -> Result<(), String> {
        let len = frame_len(width, height)?;
        if self.width != width || self.height != height || self.bgra.len() != len {
            self.bgra = vec![0; len];
            self.width = width;
            self.height = height;
        }
        self.current_frame_id = Some(frame_id);
        Ok(())
    }

    fn ensure_frame(&self, frame_id: u64) -> Result<(), String> {
        if self.current_frame_id != Some(frame_id) {
            return Err("display record frame id does not match active frame".to_string());
        }
        Ok(())
    }

    fn apply_move(
        &mut self,
        src_x: u32,
        src_y: u32,
        dst_x: u32,
        dst_y: u32,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        validate_rect(src_x, src_y, width, height, self.width, self.height)?;
        validate_rect(dst_x, dst_y, width, height, self.width, self.height)?;
        let previous = self.bgra.clone();
        let stride = self.width as usize * 4;
        let row_bytes = width as usize * 4;
        for row in 0..height as usize {
            let src = ((src_y as usize + row) * stride) + src_x as usize * 4;
            let dst = ((dst_y as usize + row) * stride) + dst_x as usize * 4;
            self.bgra[dst..dst + row_bytes].copy_from_slice(&previous[src..src + row_bytes]);
        }
        Ok(())
    }

    fn apply_atx2_atlas(
        &mut self,
        atlas_width: u32,
        atlas_height: u32,
        rects: &[DisplayAtlasRect],
        tile_commands: &[u8],
    ) -> Result<(), String> {
        if tile_commands.is_empty() {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        let atlas = if let Some(decoder) = self.metal_atx2.as_mut() {
            match decoder.decode_atlas(
                tile_commands,
                atlas_width,
                atlas_height,
                Some(&self.bgra),
                self.width,
                self.height,
            ) {
                Ok(atlas) => atlas,
                Err(_) => reconstruct_atlas_from_atx2(
                    tile_commands,
                    atlas_width,
                    atlas_height,
                    Some(&self.bgra),
                    self.width,
                    self.height,
                )?,
            }
        } else {
            reconstruct_atlas_from_atx2(
                tile_commands,
                atlas_width,
                atlas_height,
                Some(&self.bgra),
                self.width,
                self.height,
            )?
        };
        #[cfg(not(target_os = "macos"))]
        let atlas = reconstruct_atlas_from_atx2(
            tile_commands,
            atlas_width,
            atlas_height,
            Some(&self.bgra),
            self.width,
            self.height,
        )?;
        let desktop_stride = self.width as usize * 4;
        let atlas_stride = atlas_width as usize * 4;
        for rect in rects {
            validate_rect(
                rect.dst_x,
                rect.dst_y,
                rect.width,
                rect.height,
                self.width,
                self.height,
            )?;
            validate_rect(
                rect.atlas_x,
                rect.atlas_y,
                rect.width,
                rect.height,
                atlas_width,
                atlas_height,
            )?;
            let row_bytes = rect.width as usize * 4;
            for row in 0..rect.height as usize {
                let src =
                    ((rect.atlas_y as usize + row) * atlas_stride) + rect.atlas_x as usize * 4;
                let dst = ((rect.dst_y as usize + row) * desktop_stride) + rect.dst_x as usize * 4;
                self.bgra[dst..dst + row_bytes].copy_from_slice(&atlas[src..src + row_bytes]);
            }
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn apply_h264_atlas(
        &mut self,
        atlas_width: u32,
        atlas_height: u32,
        rects: &[DisplayAtlasRect],
        payload: &[u8],
    ) -> Result<(), String> {
        if self.h264_decoder_size != Some((atlas_width, atlas_height)) {
            self.h264_decoder = Some(H264Decoder::new(atlas_width, atlas_height)?);
            self.h264_decoder_size = Some((atlas_width, atlas_height));
        }
        let Some(decoder) = self.h264_decoder.as_mut() else {
            return Err("VideoToolbox H.264 decoder unavailable".to_string());
        };
        let Some(atlas) = decoder.decode(payload)? else {
            return Ok(());
        };
        let expected = frame_len(atlas_width, atlas_height)?;
        if atlas.len() != expected {
            return Err("decoded H.264 atlas length mismatch".to_string());
        }
        let desktop_stride = self.width as usize * 4;
        let atlas_stride = atlas_width as usize * 4;
        for rect in rects {
            validate_rect(
                rect.dst_x,
                rect.dst_y,
                rect.width,
                rect.height,
                self.width,
                self.height,
            )?;
            validate_rect(
                rect.atlas_x,
                rect.atlas_y,
                rect.width,
                rect.height,
                atlas_width,
                atlas_height,
            )?;
            let row_bytes = rect.width as usize * 4;
            for row in 0..rect.height as usize {
                let src =
                    ((rect.atlas_y as usize + row) * atlas_stride) + rect.atlas_x as usize * 4;
                let dst = ((rect.dst_y as usize + row) * desktop_stride) + rect.dst_x as usize * 4;
                self.bgra[dst..dst + row_bytes].copy_from_slice(&atlas[src..src + row_bytes]);
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn apply_h264_atlas(
        &mut self,
        _atlas_width: u32,
        _atlas_height: u32,
        _rects: &[DisplayAtlasRect],
        _payload: &[u8],
    ) -> Result<(), String> {
        Err("H.264 atlas display-delta records are only supported on macOS/Windows viewers".into())
    }

    fn decoded_frame(&self) -> DecodedFrame {
        DecodedFrame {
            width: self.width,
            height: self.height,
            fps: 0,
            argb: bgra_to_argb(&self.bgra),
        }
    }
}

fn bgra_to_argb(bgra: &[u8]) -> Vec<u32> {
    bgra.chunks_exact(4)
        .map(|px| {
            u32::from(px[0])
                | (u32::from(px[1]) << 8)
                | (u32::from(px[2]) << 16)
                | (u32::from(px[3]) << 24)
        })
        .collect()
}

const TILE_COMMAND_STREAM_MAGIC: u32 = 0x3258_5441;
const TILE_COMMAND_STREAM_VERSION: u32 = 4;
const TILE_COMMAND_STREAM_HEADER_BYTES: usize = 32;
const TILE_COMMAND_HEADER_BYTES: usize = 24;
const TILE_COMMAND_RAW_BGRA: u32 = 1;
const TILE_COMMAND_SOLID_COLOR: u32 = 2;
const TILE_COMMAND_XOR_RAW: u32 = 3;
const TILE_COMMAND_XOR_SPARSE: u32 = 4;
const TILE_COMMAND_MASKED_QUANT_DELTA: u32 = 5;
const TILE_COMMAND_LOSSY_UI_BLOCK: u32 = 6;
const TILE_COMMAND_SHARP_UI_BLOCK: u32 = 7;

#[cfg(target_os = "macos")]
struct MetalAtx2Decoder {
    device: MetalDevice,
    queue: CommandQueue,
    pipeline: ComputePipelineState,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MetalAtx2Params {
    atlas_width: u32,
    atlas_height: u32,
    desktop_width: u32,
    desktop_height: u32,
    command_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

#[cfg(target_os = "macos")]
impl MetalAtx2Decoder {
    fn new() -> Result<Self, String> {
        let device =
            MetalDevice::system_default().ok_or_else(|| "Metal device unavailable".to_string())?;
        let queue = device
            .new_command_queue()
            .ok_or_else(|| "Metal command queue unavailable".to_string())?;
        let library = device.new_library_with_source(ATX2_METAL_SHADER)?;
        let function = library
            .new_function("decode_atx2")
            .ok_or_else(|| "Metal ATX2 kernel unavailable".to_string())?;
        let pipeline = device.new_compute_pipeline_state(&function)?;
        Ok(Self {
            device,
            queue,
            pipeline,
        })
    }

    fn decode_atlas(
        &mut self,
        bytes: &[u8],
        atlas_width: u32,
        atlas_height: u32,
        previous_desktop: Option<&[u8]>,
        desktop_width: u32,
        desktop_height: u32,
    ) -> Result<Vec<u8>, String> {
        validate_basic_atx2_stream(bytes)?;
        let command_offsets = parse_command_word_offsets(bytes)?;
        if command_offsets.is_empty() {
            return frame_len(atlas_width, atlas_height).map(|len| vec![0; len]);
        }
        if stream_requires_previous(bytes)? && previous_desktop.is_none() {
            return Err("ATX2 Metal decode requires previous desktop".to_string());
        }

        let params = MetalAtx2Params {
            atlas_width,
            atlas_height,
            desktop_width,
            desktop_height,
            command_count: command_offsets.len() as u32,
            ..MetalAtx2Params::default()
        };
        let params_bytes = unsafe {
            std::slice::from_raw_parts(
                (&params as *const MetalAtx2Params).cast::<u8>(),
                std::mem::size_of::<MetalAtx2Params>(),
            )
        };
        let previous = previous_desktop.unwrap_or(&[]);
        let atlas_len = frame_len(atlas_width, atlas_height)?;
        let params_buffer = self.new_shared_buffer(params_bytes.len(), "ATX2 params")?;
        let offsets_buffer = self.new_shared_buffer(
            command_offsets.len() * std::mem::size_of::<u32>(),
            "ATX2 offsets",
        )?;
        let stream_buffer = self.new_shared_buffer(bytes.len(), "ATX2 stream")?;
        let previous_buffer = self.new_shared_buffer(previous.len().max(4), "ATX2 previous")?;
        let atlas_buffer = self.new_shared_buffer(atlas_len, "ATX2 atlas")?;

        write_buffer(&params_buffer, params_bytes)?;
        write_u32_buffer(&offsets_buffer, &command_offsets)?;
        write_buffer(&stream_buffer, bytes)?;
        if !previous.is_empty() {
            write_buffer(&previous_buffer, previous)?;
        }

        let command_buffer = self
            .queue
            .new_command_buffer()
            .ok_or_else(|| "Metal command buffer unavailable".to_string())?;
        let pixels = (atlas_width as usize)
            .checked_mul(atlas_height as usize)
            .ok_or_else(|| "ATX2 atlas dimensions overflow".to_string())?;
        let threads_per_group = 256usize;
        let threadgroups = pixels.div_ceil(threads_per_group).max(1);
        if !command_buffer.dispatch_compute_1d(
            &self.pipeline,
            &[
                &params_buffer,
                &offsets_buffer,
                &stream_buffer,
                &previous_buffer,
                &atlas_buffer,
            ],
            threadgroups,
            threads_per_group,
        ) {
            return Err("Metal ATX2 dispatch failed".to_string());
        }
        command_buffer.commit();
        command_buffer.wait_until_completed();
        read_buffer(&atlas_buffer, atlas_len)
    }

    fn new_shared_buffer(&self, len: usize, label: &str) -> Result<MetalBuffer, String> {
        self.device
            .new_buffer(len.max(4), resource_options::STORAGE_MODE_SHARED)
            .ok_or_else(|| format!("Metal buffer allocation failed: {label}"))
    }
}

#[cfg(target_os = "macos")]
fn write_buffer(buffer: &MetalBuffer, bytes: &[u8]) -> Result<(), String> {
    if buffer.write_bytes(bytes) != bytes.len() {
        return Err("Metal buffer write was truncated".to_string());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn write_u32_buffer(buffer: &MetalBuffer, words: &[u32]) -> Result<(), String> {
    // SAFETY: `u32` has no padding, the pointer remains valid for `words`, and the byte slice uses
    // exactly the same allocation and total size without outliving the input slice.
    let bytes = unsafe {
        std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), std::mem::size_of_val(words))
    };
    write_buffer(buffer, bytes)
}

#[cfg(target_os = "macos")]
fn read_buffer(buffer: &MetalBuffer, len: usize) -> Result<Vec<u8>, String> {
    let ptr = buffer
        .contents()
        .ok_or_else(|| "Metal shared buffer is not CPU visible".to_string())?
        .cast::<u8>();
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec())
}

#[cfg(target_os = "macos")]
fn parse_command_word_offsets(bytes: &[u8]) -> Result<Vec<u32>, String> {
    let mut offsets = Vec::new();
    let mut offset = TILE_COMMAND_STREAM_HEADER_BYTES;
    while offset < bytes.len() {
        if offset + TILE_COMMAND_HEADER_BYTES > bytes.len() {
            return Err("ATX2 command header is truncated".to_string());
        }
        let payload_len = read_u32(bytes, offset + 16)? as usize;
        let payload_end = offset
            .checked_add(TILE_COMMAND_HEADER_BYTES)
            .and_then(|value| value.checked_add(payload_len))
            .ok_or_else(|| "ATX2 command payload offset overflow".to_string())?;
        if payload_end > bytes.len() {
            return Err("ATX2 command payload is truncated".to_string());
        }
        offsets.push((offset / 4) as u32);
        offset = payload_end;
    }
    Ok(offsets)
}

#[cfg(target_os = "macos")]
fn stream_requires_previous(bytes: &[u8]) -> Result<bool, String> {
    let mut offset = TILE_COMMAND_STREAM_HEADER_BYTES;
    while offset < bytes.len() {
        let kind = read_u32(bytes, offset)?;
        if matches!(
            kind,
            TILE_COMMAND_XOR_RAW | TILE_COMMAND_XOR_SPARSE | TILE_COMMAND_MASKED_QUANT_DELTA
        ) {
            return Ok(true);
        }
        let payload_len = read_u32(bytes, offset + 16)? as usize;
        offset = offset
            .checked_add(TILE_COMMAND_HEADER_BYTES)
            .and_then(|value| value.checked_add(payload_len))
            .ok_or_else(|| "ATX2 command payload offset overflow".to_string())?;
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
fn validate_basic_atx2_stream(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < TILE_COMMAND_STREAM_HEADER_BYTES || bytes.len() % 4 != 0 {
        return Err("ATX2 stream is truncated or unaligned".to_string());
    }
    if read_u32(bytes, 0)? != TILE_COMMAND_STREAM_MAGIC {
        return Err("bad ATX2 stream magic".to_string());
    }
    if read_u32(bytes, 4)? != TILE_COMMAND_STREAM_VERSION {
        return Err("unsupported ATX2 stream version".to_string());
    }
    if read_u32(bytes, 20)? as usize != bytes.len() {
        return Err("ATX2 byte length mismatch".to_string());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
const ATX2_METAL_SHADER: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Params {
    uint atlas_width;
    uint atlas_height;
    uint desktop_width;
    uint desktop_height;
    uint command_count;
    uint pad0;
    uint pad1;
    uint pad2;
};

static inline int sx(uint value, uint bits) {
    uint sign_bit = 1u << (bits - 1);
    return (value & sign_bit) == 0 ? int(value) : int(value) - int(1u << bits);
}

static inline uint clamp8(int value) {
    return uint(clamp(value, 0, 255));
}

static inline uint rgb565_to_bgra(uint value) {
    uint b5 = value & 0x1f;
    uint g6 = (value >> 5) & 0x3f;
    uint r5 = (value >> 11) & 0x1f;
    uint b = (b5 << 3) | (b5 >> 2);
    uint g = (g6 << 2) | (g6 >> 4);
    uint r = (r5 << 3) | (r5 >> 2);
    return b | (g << 8) | (r << 16) | 0xff000000u;
}

static inline uint ycocg_to_bgra(int y, int co, int cg) {
    int tmp = y - (cg >> 1);
    int g = cg + tmp;
    int b = tmp - (co >> 1);
    int r = b + co;
    return clamp8(b) | (clamp8(g) << 8) | (clamp8(r) << 16) | 0xff000000u;
}

static inline uint apply_masked_quant_delta(uint previous, uint packed, uint shift) {
    int step = 1 << shift;
    int previous_b = int(previous & 0xff);
    int previous_g = int((previous >> 8) & 0xff);
    int previous_r = int((previous >> 16) & 0xff);
    uint alpha = previous & 0xff000000u;
    int delta_b = sx(packed & 0x1f, 5) * step;
    int delta_g = sx((packed >> 5) & 0x3f, 6) * step;
    int delta_r = sx((packed >> 11) & 0x1f, 5) * step;
    return clamp8(previous_b + delta_b)
        | (clamp8(previous_g + delta_g) << 8)
        | (clamp8(previous_r + delta_r) << 16)
        | alpha;
}

kernel void decode_atx2(constant Params &params [[buffer(0)]],
                        device const uint *command_offsets [[buffer(1)]],
                        device const uint *stream [[buffer(2)]],
                        device const uint *previous [[buffer(3)]],
                        device uint *atlas [[buffer(4)]],
                        uint gid [[thread_position_in_grid]]) {
    uint atlas_pixels = params.atlas_width * params.atlas_height;
    if (gid >= atlas_pixels) {
        return;
    }
    uint x = gid % params.atlas_width;
    uint y = gid / params.atlas_width;
    uint color = 0u;
    for (uint i = 0; i < params.command_count; i++) {
        uint base = command_offsets[i];
        uint kind = stream[base + 0];
        uint atlas_xy = stream[base + 1];
        uint desktop_xy = stream[base + 2];
        uint wh = stream[base + 3];
        uint payload_len = stream[base + 4];
        uint changed_count = stream[base + 5];
        uint atlas_x = atlas_xy & 0xffffu;
        uint atlas_y = atlas_xy >> 16;
        uint desktop_x = desktop_xy & 0xffffu;
        uint desktop_y = desktop_xy >> 16;
        uint width = wh & 0xffffu;
        uint height = wh >> 16;
        if (x < atlas_x || y < atlas_y || x >= atlas_x + width || y >= atlas_y + height) {
            continue;
        }
        uint lx = x - atlas_x;
        uint ly = y - atlas_y;
        uint pixel = ly * width + lx;
        uint payload = base + 6;
        if (kind == 1u) {
            color = stream[payload + pixel];
        } else if (kind == 2u) {
            color = stream[payload];
        } else if (kind == 3u) {
            uint prev = previous[(desktop_y + ly) * params.desktop_width + desktop_x + lx];
            color = prev ^ stream[payload + pixel];
        } else if (kind == 4u) {
            color = previous[(desktop_y + ly) * params.desktop_width + desktop_x + lx];
            uint entries = payload_len / 8u;
            for (uint e = 0; e < entries; e++) {
                uint sparse_pixel = stream[payload + e * 2u];
                if (sparse_pixel == pixel) {
                    uint prev = previous[(desktop_y + ly) * params.desktop_width + desktop_x + lx];
                    color = prev ^ stream[payload + e * 2u + 1u];
                    break;
                }
            }
        } else if (kind == 5u) {
            uint prev = previous[(desktop_y + ly) * params.desktop_width + desktop_x + lx];
            uint qshift = stream[payload] & 0xffu;
            uint pixel_count = width * height;
            uint mask_words = (pixel_count + 31u) / 32u;
            uint mask_word = stream[payload + 1u + pixel / 32u];
            if ((mask_word & (1u << (pixel & 31u))) != 0u) {
                uint residual = stream[payload + 1u + mask_words + pixel / 2u];
                uint packed = (pixel & 1u) == 0u ? (residual & 0xffffu) : (residual >> 16);
                color = apply_masked_quant_delta(prev, packed, qshift);
            } else {
                color = prev;
            }
            (void)changed_count;
        } else if (kind == 6u) {
            uint pixel_count = width * height;
            uint chroma_width = (width + 3u) / 4u;
            uint y_words = (((pixel_count + 1u) / 2u) + 3u) / 4u;
            uint y_word = stream[payload + 1u + pixel / 8u];
            uint y4 = (y_word >> ((pixel & 7u) * 4u)) & 0x0fu;
            int yy = int((y4 << 4u) | y4);
            uint chroma_index = (ly / 4u) * chroma_width + (lx / 4u);
            uint chroma_word = stream[payload + 1u + y_words + chroma_index / 4u];
            uint chroma_byte = (chroma_word >> ((chroma_index & 3u) * 8u)) & 0xffu;
            int co = sx(chroma_byte & 0x0fu, 4) * 32;
            int cg = sx((chroma_byte >> 4) & 0x0fu, 4) * 32;
            color = ycocg_to_bgra(yy, co, cg);
        } else if (kind == 7u) {
            uint word = stream[payload + pixel / 2u];
            uint packed = (pixel & 1u) == 0u ? (word & 0xffffu) : (word >> 16);
            color = rgb565_to_bgra(packed);
        }
        break;
    }
    atlas[gid] = color;
}
"#;

fn reconstruct_atlas_from_atx2(
    bytes: &[u8],
    atlas_width: u32,
    atlas_height: u32,
    previous_desktop: Option<&[u8]>,
    desktop_width: u32,
    desktop_height: u32,
) -> Result<Vec<u8>, String> {
    if bytes.len() < TILE_COMMAND_STREAM_HEADER_BYTES || bytes.len() % 4 != 0 {
        return Err("ATX2 stream is truncated or unaligned".to_string());
    }
    if read_u32(bytes, 0)? != TILE_COMMAND_STREAM_MAGIC {
        return Err("bad ATX2 stream magic".to_string());
    }
    if read_u32(bytes, 4)? != TILE_COMMAND_STREAM_VERSION {
        return Err("unsupported ATX2 stream version".to_string());
    }
    if read_u32(bytes, 20)? as usize != bytes.len() {
        return Err("ATX2 byte length mismatch".to_string());
    }
    let mut atlas = vec![0u8; frame_len(atlas_width, atlas_height)?];
    let atlas_stride = atlas_width as usize * 4;
    let mut offset = TILE_COMMAND_STREAM_HEADER_BYTES;
    while offset < bytes.len() {
        if offset + TILE_COMMAND_HEADER_BYTES > bytes.len() {
            return Err("ATX2 command header is truncated".to_string());
        }
        let kind = read_u32(bytes, offset)?;
        let atlas_xy = read_u32(bytes, offset + 4)?;
        let desktop_xy = read_u32(bytes, offset + 8)?;
        let wh = read_u32(bytes, offset + 12)?;
        let payload_len = read_u32(bytes, offset + 16)? as usize;
        let changed_count = read_u32(bytes, offset + 20)? as usize;
        let atlas_x = atlas_xy & 0xffff;
        let atlas_y = atlas_xy >> 16;
        let desktop_x = desktop_xy & 0xffff;
        let desktop_y = desktop_xy >> 16;
        let width = wh & 0xffff;
        let height = wh >> 16;
        let payload_offset = offset + TILE_COMMAND_HEADER_BYTES;
        let payload_end = payload_offset
            .checked_add(payload_len)
            .ok_or_else(|| "ATX2 payload offset overflow".to_string())?;
        if payload_end > bytes.len() {
            return Err("ATX2 command payload is truncated".to_string());
        }
        validate_rect(atlas_x, atlas_y, width, height, atlas_width, atlas_height)?;
        let payload = &bytes[payload_offset..payload_end];
        match kind {
            TILE_COMMAND_SOLID_COLOR => {
                if payload.len() != 4 {
                    return Err("ATX2 solid-color payload length mismatch".to_string());
                }
                let color = read_u32(payload, 0)?.to_le_bytes();
                fill_rect(
                    &mut atlas,
                    atlas_stride,
                    atlas_x,
                    atlas_y,
                    width,
                    height,
                    color,
                );
            }
            TILE_COMMAND_RAW_BGRA => {
                let row_bytes = width as usize * 4;
                if payload.len() != row_bytes * height as usize {
                    return Err("ATX2 raw payload length mismatch".to_string());
                }
                for row in 0..height as usize {
                    let src = row * row_bytes;
                    let dst = ((atlas_y as usize + row) * atlas_stride) + atlas_x as usize * 4;
                    atlas[dst..dst + row_bytes].copy_from_slice(&payload[src..src + row_bytes]);
                }
            }
            TILE_COMMAND_XOR_RAW | TILE_COMMAND_XOR_SPARSE => {
                let previous = previous_desktop
                    .ok_or_else(|| "ATX2 delta command needs previous frame".to_string())?;
                validate_rect(
                    desktop_x,
                    desktop_y,
                    width,
                    height,
                    desktop_width,
                    desktop_height,
                )?;
                copy_previous_rect(
                    &mut atlas,
                    atlas_stride,
                    atlas_x,
                    atlas_y,
                    width,
                    height,
                    previous,
                    desktop_width,
                    desktop_x,
                    desktop_y,
                )?;
                if kind == TILE_COMMAND_XOR_RAW {
                    let pixel_count = width as usize * height as usize;
                    if payload.len() != pixel_count * 4 {
                        return Err("ATX2 XOR raw payload length mismatch".to_string());
                    }
                    for pixel in 0..pixel_count {
                        let prev = read_previous_pixel(
                            previous,
                            desktop_width,
                            desktop_x,
                            desktop_y,
                            width,
                            pixel,
                        )?;
                        let color = (prev ^ read_u32(payload, pixel * 4)?).to_le_bytes();
                        write_pixel(
                            &mut atlas,
                            atlas_stride,
                            atlas_x,
                            atlas_y,
                            width,
                            pixel,
                            color,
                        );
                    }
                } else {
                    if payload.len() % 8 != 0 {
                        return Err("ATX2 XOR sparse payload is unaligned".to_string());
                    }
                    let pixel_count = width as usize * height as usize;
                    for entry_offset in (0..payload.len()).step_by(8) {
                        let pixel = read_u32(payload, entry_offset)? as usize;
                        if pixel >= pixel_count {
                            return Err("ATX2 XOR sparse pixel index exceeds tile".to_string());
                        }
                        let prev = read_previous_pixel(
                            previous,
                            desktop_width,
                            desktop_x,
                            desktop_y,
                            width,
                            pixel,
                        )?;
                        let color = (prev ^ read_u32(payload, entry_offset + 4)?).to_le_bytes();
                        write_pixel(
                            &mut atlas,
                            atlas_stride,
                            atlas_x,
                            atlas_y,
                            width,
                            pixel,
                            color,
                        );
                    }
                }
            }
            TILE_COMMAND_MASKED_QUANT_DELTA => decode_masked_quant_delta_payload(
                &mut atlas,
                atlas_stride,
                atlas_x,
                atlas_y,
                desktop_x,
                desktop_y,
                width,
                height,
                payload,
                changed_count,
                previous_desktop,
                desktop_width,
                desktop_height,
            )?,
            TILE_COMMAND_LOSSY_UI_BLOCK => decode_lossy_ui_block_payload(
                &mut atlas,
                atlas_stride,
                atlas_x,
                atlas_y,
                width,
                height,
                payload,
            )?,
            TILE_COMMAND_SHARP_UI_BLOCK => decode_sharp_ui_block_payload(
                &mut atlas,
                atlas_stride,
                atlas_x,
                atlas_y,
                width,
                height,
                payload,
            )?,
            other => return Err(format!("unsupported ATX2 command kind {other}")),
        }
        offset = payload_end;
    }
    Ok(atlas)
}

fn decode_masked_quant_delta_payload(
    atlas: &mut [u8],
    atlas_stride: usize,
    atlas_x: u32,
    atlas_y: u32,
    desktop_x: u32,
    desktop_y: u32,
    width: u32,
    height: u32,
    payload: &[u8],
    changed_count: usize,
    previous_desktop: Option<&[u8]>,
    desktop_width: u32,
    desktop_height: u32,
) -> Result<(), String> {
    let previous = previous_desktop
        .ok_or_else(|| "ATX2 masked delta command needs previous frame".to_string())?;
    validate_rect(
        desktop_x,
        desktop_y,
        width,
        height,
        desktop_width,
        desktop_height,
    )?;
    let pixel_count = width as usize * height as usize;
    if changed_count > pixel_count {
        return Err("ATX2 masked delta changed count exceeds tile".to_string());
    }
    let mask_words = pixel_count.div_ceil(32);
    let mask_bytes = mask_words
        .checked_mul(4)
        .ok_or_else(|| "ATX2 mask byte length overflow".to_string())?;
    let residual_bytes = pixel_count
        .checked_mul(2)
        .ok_or_else(|| "ATX2 residual byte length overflow".to_string())?;
    let expected = align_usize_to_4(
        4usize
            .checked_add(mask_bytes)
            .and_then(|value| value.checked_add(residual_bytes))
            .ok_or_else(|| "ATX2 masked delta payload length overflow".to_string())?,
    );
    if payload.len() != expected {
        return Err("ATX2 masked delta payload length mismatch".to_string());
    }
    let quant_shift = read_u32(payload, 0)? & 0xff;
    if quant_shift > 4 {
        return Err("ATX2 masked delta shift is invalid".to_string());
    }
    let mask_offset = 4usize;
    let residual_offset = mask_offset + mask_bytes;
    let mut seen_changed = 0usize;
    for pixel in 0..pixel_count {
        let previous_pixel =
            read_previous_pixel(previous, desktop_width, desktop_x, desktop_y, width, pixel)?;
        let mask_word = read_u32(payload, mask_offset + (pixel / 32) * 4)?;
        let color = if (mask_word & (1u32 << (pixel & 31))) != 0 {
            seen_changed = seen_changed.saturating_add(1);
            let residual_word = read_u32(payload, residual_offset + (pixel / 2) * 4)?;
            let packed = if pixel & 1 == 0 {
                residual_word & 0xffff
            } else {
                residual_word >> 16
            };
            apply_masked_quant_delta(previous_pixel, packed, quant_shift)
        } else {
            previous_pixel
        };
        write_pixel(
            atlas,
            atlas_stride,
            atlas_x,
            atlas_y,
            width,
            pixel,
            color.to_le_bytes(),
        );
    }
    if seen_changed != changed_count {
        return Err("ATX2 masked delta changed mask count mismatch".to_string());
    }
    Ok(())
}

fn decode_lossy_ui_block_payload(
    atlas: &mut [u8],
    atlas_stride: usize,
    atlas_x: u32,
    atlas_y: u32,
    width: u32,
    height: u32,
    payload: &[u8],
) -> Result<(), String> {
    let pixel_count = width as usize * height as usize;
    let chroma_width = (width as usize).div_ceil(4);
    let chroma_height = (height as usize).div_ceil(4);
    let chroma_count = chroma_width
        .checked_mul(chroma_height)
        .ok_or_else(|| "ATX2 lossy block chroma count overflow".to_string())?;
    let y_bytes = align_usize_to_4(pixel_count.div_ceil(2));
    let chroma_bytes = align_usize_to_4(chroma_count);
    let expected = 4usize
        .checked_add(y_bytes)
        .and_then(|value| value.checked_add(chroma_bytes))
        .ok_or_else(|| "ATX2 lossy block payload length overflow".to_string())?;
    if payload.len() != expected {
        return Err("ATX2 lossy block payload length mismatch".to_string());
    }
    let header = read_u32(payload, 0)?;
    if (header & 0xffff) as usize != chroma_width || (header >> 16) as usize != chroma_height {
        return Err("ATX2 lossy block chroma dimensions mismatch".to_string());
    }
    let y_offset = 4usize;
    let chroma_offset = y_offset + y_bytes;
    for pixel in 0..pixel_count {
        let y_word = read_u32(payload, y_offset + (pixel / 8) * 4)?;
        let y4 = (y_word >> ((pixel & 7) * 4)) & 0x0f;
        let y = ((y4 << 4) | y4) as i32;
        let px = pixel % width as usize;
        let py = pixel / width as usize;
        let chroma_index = (py / 4) * chroma_width + (px / 4);
        let chroma_word = read_u32(payload, chroma_offset + (chroma_index / 4) * 4)?;
        let chroma_byte = (chroma_word >> ((chroma_index & 3) * 8)) & 0xff;
        let co = sign_extend(chroma_byte & 0x0f, 4) * 32;
        let cg = sign_extend((chroma_byte >> 4) & 0x0f, 4) * 32;
        write_pixel(
            atlas,
            atlas_stride,
            atlas_x,
            atlas_y,
            width,
            pixel,
            ycocg_to_bgra(y, co, cg).to_le_bytes(),
        );
    }
    Ok(())
}

fn decode_sharp_ui_block_payload(
    atlas: &mut [u8],
    atlas_stride: usize,
    atlas_x: u32,
    atlas_y: u32,
    width: u32,
    height: u32,
    payload: &[u8],
) -> Result<(), String> {
    let pixel_count = width as usize * height as usize;
    let expected = align_usize_to_4(
        pixel_count
            .checked_mul(2)
            .ok_or_else(|| "ATX2 sharp block payload length overflow".to_string())?,
    );
    if payload.len() != expected {
        return Err("ATX2 sharp block payload length mismatch".to_string());
    }
    for pixel in 0..pixel_count {
        let word = read_u32(payload, (pixel / 2) * 4)?;
        let packed = if pixel & 1 == 0 {
            word & 0xffff
        } else {
            word >> 16
        };
        write_pixel(
            atlas,
            atlas_stride,
            atlas_x,
            atlas_y,
            width,
            pixel,
            rgb565_to_bgra(packed).to_le_bytes(),
        );
    }
    Ok(())
}

fn apply_masked_quant_delta(previous_pixel: u32, packed_delta: u32, quant_shift: u32) -> u32 {
    let step = 1i32 << quant_shift;
    let previous_b = (previous_pixel & 0xff) as i32;
    let previous_g = ((previous_pixel >> 8) & 0xff) as i32;
    let previous_r = ((previous_pixel >> 16) & 0xff) as i32;
    let alpha = previous_pixel & 0xff00_0000;
    let delta_b = sign_extend(packed_delta & 0x1f, 5) * step;
    let delta_g = sign_extend((packed_delta >> 5) & 0x3f, 6) * step;
    let delta_r = sign_extend((packed_delta >> 11) & 0x1f, 5) * step;
    u32::from(clamp(previous_b + delta_b))
        | (u32::from(clamp(previous_g + delta_g)) << 8)
        | (u32::from(clamp(previous_r + delta_r)) << 16)
        | alpha
}

fn rgb565_to_bgra(value: u32) -> u32 {
    let b5 = value & 0x1f;
    let g6 = (value >> 5) & 0x3f;
    let r5 = (value >> 11) & 0x1f;
    let b = (b5 << 3) | (b5 >> 2);
    let g = (g6 << 2) | (g6 >> 4);
    let r = (r5 << 3) | (r5 >> 2);
    b | (g << 8) | (r << 16) | 0xff00_0000
}

fn ycocg_to_bgra(y: i32, co: i32, cg: i32) -> u32 {
    let tmp = y - (cg >> 1);
    let g = cg + tmp;
    let b = tmp - (co >> 1);
    let r = b + co;
    u32::from(clamp(b)) | (u32::from(clamp(g)) << 8) | (u32::from(clamp(r)) << 16) | 0xff00_0000
}

fn sign_extend(value: u32, bits: u32) -> i32 {
    let sign_bit = 1u32 << (bits - 1);
    if value & sign_bit == 0 {
        value as i32
    } else {
        value as i32 - (1i32 << bits)
    }
}

fn align_usize_to_4(value: usize) -> usize {
    (value + 3) & !3
}

fn copy_previous_rect(
    atlas: &mut [u8],
    atlas_stride: usize,
    atlas_x: u32,
    atlas_y: u32,
    width: u32,
    height: u32,
    previous: &[u8],
    desktop_width: u32,
    desktop_x: u32,
    desktop_y: u32,
) -> Result<(), String> {
    let desktop_stride = desktop_width as usize * 4;
    let row_bytes = width as usize * 4;
    for row in 0..height as usize {
        let src = ((desktop_y as usize + row) * desktop_stride) + desktop_x as usize * 4;
        let dst = ((atlas_y as usize + row) * atlas_stride) + atlas_x as usize * 4;
        atlas[dst..dst + row_bytes].copy_from_slice(&previous[src..src + row_bytes]);
    }
    Ok(())
}

fn read_previous_pixel(
    previous: &[u8],
    desktop_width: u32,
    desktop_x: u32,
    desktop_y: u32,
    tile_width: u32,
    pixel: usize,
) -> Result<u32, String> {
    let row = pixel / tile_width as usize;
    let col = pixel % tile_width as usize;
    let offset = ((desktop_y as usize + row) * desktop_width as usize + desktop_x as usize + col)
        .checked_mul(4)
        .ok_or_else(|| "previous desktop pixel offset overflow".to_string())?;
    read_u32(previous, offset)
}

fn fill_rect(
    atlas: &mut [u8],
    atlas_stride: usize,
    atlas_x: u32,
    atlas_y: u32,
    width: u32,
    height: u32,
    color: [u8; 4],
) {
    for row in 0..height as usize {
        let dst = ((atlas_y as usize + row) * atlas_stride) + atlas_x as usize * 4;
        for pixel in 0..width as usize {
            let start = dst + pixel * 4;
            atlas[start..start + 4].copy_from_slice(&color);
        }
    }
}

fn write_pixel(
    atlas: &mut [u8],
    atlas_stride: usize,
    atlas_x: u32,
    atlas_y: u32,
    width: u32,
    pixel: usize,
    color: [u8; 4],
) {
    let row = pixel / width as usize;
    let col = pixel % width as usize;
    let dst = ((atlas_y as usize + row) * atlas_stride) + (atlas_x as usize + col) * 4;
    atlas[dst..dst + 4].copy_from_slice(&color);
}

fn validate_rect(
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    limit_width: u32,
    limit_height: u32,
) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("rect is empty".to_string());
    }
    if x.checked_add(width).is_none_or(|right| right > limit_width)
        || y.checked_add(height)
            .is_none_or(|bottom| bottom > limit_height)
    {
        return Err("rect exceeds bounds".to_string());
    }
    Ok(())
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| "u32 read offset overflow".to_string())?;
    let value = bytes
        .get(offset..end)
        .ok_or_else(|| "u32 read exceeds byte stream".to_string())?;
    Ok(u32::from_le_bytes(
        value.try_into().expect("slice has length 4"),
    ))
}

fn frame_len(width: u32, height: u32) -> Result<usize, String> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "frame dimensions overflow".to_string())
}

#[cfg(test)]
mod cpu_tests {
    use super::*;
    use talos_protocol::{
        build_display_experimental_atlas_commands_chunk, build_display_frame_begin,
        build_display_frame_end, DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL,
        DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE,
    };

    #[test]
    fn compositor_applies_atx2_row_band_chunks() {
        let mut compositor = ModernDisplayCompositor::new();
        let frame_id = 7;
        compositor
            .handle_record(&build_display_frame_begin(frame_id, 4, 3))
            .expect("begin frame");

        let top_payload = [
            0x01, 0x02, 0x03, 0xff, 0x04, 0x05, 0x06, 0xff, 0x07, 0x08, 0x09, 0xff, 0x0a, 0x0b,
            0x0c, 0xff,
        ];
        let bottom_payload = [
            0x11, 0x12, 0x13, 0xff, 0x14, 0x15, 0x16, 0xff, 0x17, 0x18, 0x19, 0xff, 0x1a, 0x1b,
            0x1c, 0xff,
        ];
        let top_rect = DisplayAtlasRect {
            dst_x: 0,
            dst_y: 0,
            width: 4,
            height: 1,
            atlas_x: 0,
            atlas_y: 0,
        };
        let bottom_rect = DisplayAtlasRect {
            dst_x: 0,
            dst_y: 2,
            width: 4,
            height: 1,
            atlas_x: 0,
            atlas_y: 0,
        };
        compositor
            .handle_record(&build_display_experimental_atlas_commands_chunk(
                frame_id,
                DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE,
                0,
                2,
                4,
                1,
                &[top_rect],
                &single_raw_atx2_stream(4, 1, &top_payload),
            ))
            .expect("top chunk");
        compositor
            .handle_record(&build_display_experimental_atlas_commands_chunk(
                frame_id,
                DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE
                    | DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL,
                1,
                2,
                4,
                1,
                &[bottom_rect],
                &single_raw_atx2_stream(4, 1, &bottom_payload),
            ))
            .expect("bottom chunk");

        let decoded = compositor
            .handle_record(&build_display_frame_end(frame_id))
            .expect("end frame")
            .expect("decoded frame");
        let mut expected_bgra = Vec::new();
        expected_bgra.extend_from_slice(&top_payload);
        expected_bgra.extend_from_slice(&[0; 16]);
        expected_bgra.extend_from_slice(&bottom_payload);
        assert_eq!(decoded.width, 4);
        assert_eq!(decoded.height, 3);
        assert_eq!(decoded.argb, bgra_to_argb(&expected_bgra));
    }

    fn single_raw_atx2_stream(width: u32, height: u32, payload: &[u8]) -> Vec<u8> {
        let byte_len = TILE_COMMAND_STREAM_HEADER_BYTES + TILE_COMMAND_HEADER_BYTES + payload.len();
        let mut bytes = vec![0u8; byte_len];
        write_u32_test(&mut bytes, 0, TILE_COMMAND_STREAM_MAGIC);
        write_u32_test(&mut bytes, 4, TILE_COMMAND_STREAM_VERSION);
        write_u32_test(&mut bytes, 8, pack_xy(width, height));
        write_u32_test(&mut bytes, 12, pack_xy(32, 1));
        write_u32_test(&mut bytes, 16, 1);
        write_u32_test(&mut bytes, 20, byte_len as u32);
        write_u32_test(&mut bytes, 24, 1);
        write_u32_test(
            &mut bytes,
            TILE_COMMAND_STREAM_HEADER_BYTES,
            TILE_COMMAND_RAW_BGRA,
        );
        write_u32_test(
            &mut bytes,
            TILE_COMMAND_STREAM_HEADER_BYTES + 12,
            pack_xy(width, height),
        );
        write_u32_test(
            &mut bytes,
            TILE_COMMAND_STREAM_HEADER_BYTES + 16,
            payload.len() as u32,
        );
        write_u32_test(
            &mut bytes,
            TILE_COMMAND_STREAM_HEADER_BYTES + 20,
            width * height,
        );
        bytes[TILE_COMMAND_STREAM_HEADER_BYTES + TILE_COMMAND_HEADER_BYTES..]
            .copy_from_slice(payload);
        bytes
    }

    fn write_u32_test(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn pack_xy(x: u32, y: u32) -> u32 {
        (x & 0xffff) | ((y & 0xffff) << 16)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn metal_atx2_decoder_handles_solid_and_raw_commands() {
        let mut decoder = MetalAtx2Decoder::new().expect("Metal ATX2 decoder");
        let solid =
            build_single_command_stream(2, 2, TILE_COMMAND_SOLID_COLOR, &[0x44, 0x33, 0x22, 0xff]);
        let decoded = decoder
            .decode_atlas(&solid, 2, 2, None, 2, 2)
            .expect("decode solid ATX2");
        assert_eq!(
            decoded,
            [
                0x44, 0x33, 0x22, 0xff, 0x44, 0x33, 0x22, 0xff, 0x44, 0x33, 0x22, 0xff, 0x44, 0x33,
                0x22, 0xff,
            ]
        );

        let raw_payload = [
            0x01, 0x02, 0x03, 0xff, 0x04, 0x05, 0x06, 0xff, 0x07, 0x08, 0x09, 0xff, 0x0a, 0x0b,
            0x0c, 0xff,
        ];
        let raw = build_single_command_stream(2, 2, TILE_COMMAND_RAW_BGRA, &raw_payload);
        let decoded = decoder
            .decode_atlas(&raw, 2, 2, None, 2, 2)
            .expect("decode raw ATX2");
        assert_eq!(decoded, raw_payload);
    }

    #[test]
    fn metal_atx2_decoder_handles_xor_sparse_delta() {
        let mut decoder = MetalAtx2Decoder::new().expect("Metal ATX2 decoder");
        let previous = [
            0x10, 0x20, 0x30, 0xff, 0x11, 0x21, 0x31, 0xff, 0x12, 0x22, 0x32, 0xff, 0x13, 0x23,
            0x33, 0xff,
        ];
        let mut payload = Vec::new();
        payload.extend_from_slice(&2u32.to_le_bytes());
        payload.extend_from_slice(&0x0000_00ffu32.to_le_bytes());
        let stream = build_single_command_stream(2, 2, TILE_COMMAND_XOR_SPARSE, &payload);
        let decoded = decoder
            .decode_atlas(&stream, 2, 2, Some(&previous), 2, 2)
            .expect("decode sparse ATX2");
        let mut expected = previous;
        let previous_pixel = u32::from_le_bytes(expected[8..12].try_into().unwrap());
        expected[8..12].copy_from_slice(&(previous_pixel ^ 0x0000_00ff).to_le_bytes());
        assert_eq!(decoded, expected);
    }

    #[test]
    fn metal_atx2_decoder_matches_cpu_for_delta_and_ui_blocks() {
        let mut decoder = MetalAtx2Decoder::new().expect("Metal ATX2 decoder");
        let previous = [
            0x10, 0x20, 0x30, 0xff, 0x22, 0x32, 0x42, 0xff, 0x34, 0x44, 0x54, 0xff, 0x46, 0x56,
            0x66, 0xff,
        ];

        let xor_raw_payload = [
            0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x04, 0x00,
            0x00, 0x00,
        ];
        assert_metal_matches_cpu(
            &mut decoder,
            &build_single_command_stream(2, 2, TILE_COMMAND_XOR_RAW, &xor_raw_payload),
            Some(&previous),
        );

        let mut masked_payload = Vec::new();
        masked_payload.extend_from_slice(&1u32.to_le_bytes());
        masked_payload.extend_from_slice(&0b0101u32.to_le_bytes());
        masked_payload.extend_from_slice(&0x0000_0001u32.to_le_bytes());
        masked_payload.extend_from_slice(&0x0000_f800u32.to_le_bytes());
        assert_metal_matches_cpu(
            &mut decoder,
            &build_single_command_stream_with_changed_count(
                2,
                2,
                TILE_COMMAND_MASKED_QUANT_DELTA,
                &masked_payload,
                2,
            ),
            Some(&previous),
        );

        let mut lossy_payload = Vec::new();
        lossy_payload.extend_from_slice(&pack_xy(1, 1).to_le_bytes());
        lossy_payload.extend_from_slice(&0x3210u32.to_le_bytes());
        lossy_payload.extend_from_slice(&0x0000_00f1u32.to_le_bytes());
        assert_metal_matches_cpu(
            &mut decoder,
            &build_single_command_stream(2, 2, TILE_COMMAND_LOSSY_UI_BLOCK, &lossy_payload),
            None,
        );

        let sharp_payload = [0x1f, 0x00, 0xe0, 0x07, 0x00, 0xf8, 0xff, 0xff];
        assert_metal_matches_cpu(
            &mut decoder,
            &build_single_command_stream(2, 2, TILE_COMMAND_SHARP_UI_BLOCK, &sharp_payload),
            None,
        );
    }

    fn assert_metal_matches_cpu(
        decoder: &mut MetalAtx2Decoder,
        stream: &[u8],
        previous: Option<&[u8]>,
    ) {
        let metal = decoder
            .decode_atlas(stream, 2, 2, previous, 2, 2)
            .expect("decode ATX2 with Metal");
        let cpu =
            reconstruct_atlas_from_atx2(stream, 2, 2, previous, 2, 2).expect("decode ATX2 on CPU");
        assert_eq!(metal, cpu);
    }

    fn build_single_command_stream(width: u32, height: u32, kind: u32, payload: &[u8]) -> Vec<u8> {
        build_single_command_stream_with_changed_count(width, height, kind, payload, width * height)
    }

    fn build_single_command_stream_with_changed_count(
        width: u32,
        height: u32,
        kind: u32,
        payload: &[u8],
        changed_count: u32,
    ) -> Vec<u8> {
        let mut bytes = vec![0u8; TILE_COMMAND_STREAM_HEADER_BYTES];
        let byte_len =
            (TILE_COMMAND_STREAM_HEADER_BYTES + TILE_COMMAND_HEADER_BYTES + payload.len()) as u32;
        write_u32_test(&mut bytes, 0, TILE_COMMAND_STREAM_MAGIC);
        write_u32_test(&mut bytes, 4, TILE_COMMAND_STREAM_VERSION);
        write_u32_test(&mut bytes, 8, pack_xy(width, height));
        write_u32_test(&mut bytes, 12, pack_xy(32, 1));
        write_u32_test(&mut bytes, 16, 1);
        write_u32_test(&mut bytes, 20, byte_len);
        write_u32_test(&mut bytes, 24, 1);
        bytes.resize(
            TILE_COMMAND_STREAM_HEADER_BYTES + TILE_COMMAND_HEADER_BYTES,
            0,
        );
        let offset = TILE_COMMAND_STREAM_HEADER_BYTES;
        write_u32_test(&mut bytes, offset, kind);
        write_u32_test(&mut bytes, offset + 4, pack_xy(0, 0));
        write_u32_test(&mut bytes, offset + 8, pack_xy(0, 0));
        write_u32_test(&mut bytes, offset + 12, pack_xy(width, height));
        write_u32_test(&mut bytes, offset + 16, payload.len() as u32);
        write_u32_test(&mut bytes, offset + 20, changed_count);
        bytes.extend_from_slice(payload);
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        let len = bytes.len() as u32;
        write_u32_test(&mut bytes, 20, len);
        bytes
    }

    fn pack_xy(x: u32, y: u32) -> u32 {
        (x & 0xffff) | ((y & 0xffff) << 16)
    }

    fn write_u32_test(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}
