#![cfg(target_os = "macos")]

use std::{ffi::c_void, ptr, slice};

use objc2_core_foundation::CFRetained;
use objc2_core_media::{
    kCMBlockBufferAssureMemoryNowFlag, CMBlockBuffer, CMFormatDescription, CMSampleBuffer, CMTime,
    CMVideoFormatDescription, CMVideoFormatDescriptionCreateFromH264ParameterSets,
};
use objc2_core_video::{
    kCVPixelFormatType_32BGRA, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, CVImageBuffer, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferGetWidthOfPlane,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_video_toolbox::{
    VTDecodeFrameFlags, VTDecodeInfoFlags, VTDecompressionOutputCallbackRecord,
    VTDecompressionSession,
};

pub(crate) struct H264Decoder {
    width: u32,
    height: u32,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    format_description: Option<CFRetained<CMFormatDescription>>,
    session: Option<CFRetained<VTDecompressionSession>>,
}

struct DecodeOutput {
    frame: Option<Vec<u8>>,
    error: Option<String>,
    expected_width: u32,
    expected_height: u32,
}

unsafe impl Send for H264Decoder {}

impl H264Decoder {
    pub(crate) fn new(width: u32, height: u32) -> Result<Self, String> {
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(format!(
                "VideoToolbox H.264 decoder requires even non-zero dimensions, got {width}x{height}"
            ));
        }
        Ok(Self {
            width,
            height,
            sps: None,
            pps: None,
            format_description: None,
            session: None,
        })
    }

    pub(crate) fn decode(&mut self, payload: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let nals = annex_b_nals(payload);
        if nals.is_empty() {
            return Ok(None);
        }

        let mut parameter_sets_changed = false;
        for nal in &nals {
            match nal_type(nal) {
                7 if self.sps.as_deref() != Some(*nal) => {
                    self.sps = Some((*nal).to_vec());
                    self.pps = None;
                    parameter_sets_changed = true;
                }
                8 if self.pps.as_deref() != Some(*nal) => {
                    self.pps = Some((*nal).to_vec());
                    parameter_sets_changed = true;
                }
                _ => {}
            }
        }
        if parameter_sets_changed {
            self.reset_session();
        }

        if self.session.is_none() {
            self.create_session()?;
        }

        let sample_payload = avcc_payload(&nals)?;
        if sample_payload.is_empty() || !nals.iter().any(|nal| matches!(nal_type(nal), 1 | 5)) {
            return Ok(None);
        }
        let block = create_block_buffer(&sample_payload)?;
        let sample = create_sample_buffer(&block, self.format_description.as_deref().unwrap())?;
        let mut output = DecodeOutput {
            frame: None,
            error: None,
            expected_width: self.width,
            expected_height: self.height,
        };
        let mut info_flags = VTDecodeInfoFlags::empty();
        let status = unsafe {
            self.session.as_deref().unwrap().decode_frame(
                &sample,
                VTDecodeFrameFlags::Frame_1xRealTimePlayback,
                (&mut output as *mut DecodeOutput).cast(),
                &mut info_flags,
            )
        };
        if status != 0 {
            return Err(format!(
                "VideoToolbox decode frame failed: OSStatus {status}"
            ));
        }
        if info_flags.contains(VTDecodeInfoFlags::Asynchronous) {
            let wait_status = unsafe {
                self.session
                    .as_deref()
                    .unwrap()
                    .wait_for_asynchronous_frames()
            };
            if wait_status != 0 {
                return Err(format!(
                    "VideoToolbox wait for asynchronous decode failed: OSStatus {wait_status}"
                ));
            }
        }
        if info_flags.contains(VTDecodeInfoFlags::FrameDropped) {
            return Ok(None);
        }
        if let Some(err) = output.error {
            return Err(err);
        }
        Ok(output.frame)
    }

    fn create_session(&mut self) -> Result<(), String> {
        let (Some(sps), Some(pps)) = (self.sps.as_mut(), self.pps.as_mut()) else {
            return Err("H.264 atlas stream has not provided SPS/PPS yet".to_string());
        };

        let mut parameter_set_pointers = [
            std::ptr::NonNull::new(sps.as_mut_ptr()).ok_or("missing SPS")?,
            std::ptr::NonNull::new(pps.as_mut_ptr()).ok_or("missing PPS")?,
        ];
        let mut parameter_set_sizes = [sps.len(), pps.len()];
        let mut description: *const CMVideoFormatDescription = ptr::null();
        let status = unsafe {
            CMVideoFormatDescriptionCreateFromH264ParameterSets(
                None,
                parameter_set_pointers.len(),
                std::ptr::NonNull::new(parameter_set_pointers.as_mut_ptr())
                    .ok_or("invalid H.264 parameter-set pointer")?,
                std::ptr::NonNull::new(parameter_set_sizes.as_mut_ptr())
                    .ok_or("invalid H.264 parameter-set size pointer")?,
                4,
                std::ptr::NonNull::new(&mut description as *mut *const CMVideoFormatDescription)
                    .ok_or("invalid H.264 format-description pointer")?
                    .cast(),
            )
        };
        if status != 0 {
            return Err(format!(
                "CMVideoFormatDescriptionCreateFromH264ParameterSets failed: OSStatus {status}"
            ));
        }
        let description = unsafe {
            CFRetained::from_raw(
                std::ptr::NonNull::new(description as *mut CMFormatDescription)
                    .ok_or("VideoToolbox returned null H.264 format description")?,
            )
        };

        let callback = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(decompression_output_callback),
            decompressionOutputRefCon: ptr::null_mut(),
        };
        let mut session: *mut VTDecompressionSession = ptr::null_mut();
        let status = unsafe {
            VTDecompressionSession::create(
                None,
                &description,
                None,
                None,
                &callback,
                std::ptr::NonNull::new(&mut session as *mut *mut VTDecompressionSession)
                    .ok_or("invalid VideoToolbox decoder session pointer")?,
            )
        };
        if status != 0 {
            return Err(format!(
                "VTDecompressionSessionCreate failed: OSStatus {status}"
            ));
        }
        self.session = Some(unsafe {
            CFRetained::from_raw(
                std::ptr::NonNull::new(session)
                    .ok_or("VideoToolbox returned null decoder session")?,
            )
        });
        self.format_description = Some(description);
        Ok(())
    }

    fn reset_session(&mut self) {
        if let Some(session) = self.session.take() {
            unsafe { session.invalidate() };
        }
        self.format_description = None;
    }
}

impl Drop for H264Decoder {
    fn drop(&mut self) {
        if let Some(session) = self.session.as_deref() {
            unsafe { session.invalidate() };
        }
    }
}

unsafe extern "C-unwind" fn decompression_output_callback(
    refcon: *mut c_void,
    source_frame_refcon: *mut c_void,
    status: i32,
    _info_flags: VTDecodeInfoFlags,
    image_buffer: *mut CVImageBuffer,
    _presentation_time_stamp: CMTime,
    _presentation_duration: CMTime,
) {
    let output_refcon = if source_frame_refcon.is_null() {
        refcon
    } else {
        source_frame_refcon
    };
    if output_refcon.is_null() {
        return;
    }
    let output = unsafe { &mut *(output_refcon.cast::<DecodeOutput>()) };
    if status != 0 {
        output.error = Some(format!(
            "VideoToolbox decompression callback failed: OSStatus {status}"
        ));
        return;
    }
    if image_buffer.is_null() {
        return;
    }
    match unsafe {
        copy_pixel_buffer_to_bgra(
            &*image_buffer,
            output.expected_width as usize,
            output.expected_height as usize,
        )
    } {
        Ok(frame) => output.frame = Some(frame),
        Err(err) => output.error = Some(err),
    }
}

fn create_block_buffer(bytes: &[u8]) -> Result<CFRetained<CMBlockBuffer>, String> {
    let mut block: *mut CMBlockBuffer = ptr::null_mut();
    let status = unsafe {
        CMBlockBuffer::create_with_memory_block(
            None,
            ptr::null_mut(),
            bytes.len(),
            None,
            ptr::null(),
            0,
            bytes.len(),
            kCMBlockBufferAssureMemoryNowFlag,
            std::ptr::NonNull::new(&mut block as *mut *mut CMBlockBuffer)
                .ok_or("invalid CMBlockBuffer pointer")?,
        )
    };
    if status != 0 {
        return Err(format!(
            "CMBlockBufferCreateWithMemoryBlock failed: OSStatus {status}"
        ));
    }
    let block = unsafe {
        CFRetained::from_raw(
            std::ptr::NonNull::new(block).ok_or("CoreMedia returned null block buffer")?,
        )
    };
    let status = unsafe {
        CMBlockBuffer::replace_data_bytes(
            std::ptr::NonNull::new(bytes.as_ptr() as *mut c_void)
                .ok_or("invalid CMBlockBuffer source pointer")?,
            &block,
            0,
            bytes.len(),
        )
    };
    if status != 0 {
        return Err(format!(
            "CMBlockBufferReplaceDataBytes failed: OSStatus {status}"
        ));
    }
    Ok(block)
}

fn create_sample_buffer(
    block: &CMBlockBuffer,
    description: &CMFormatDescription,
) -> Result<CFRetained<CMSampleBuffer>, String> {
    let mut sample: *mut CMSampleBuffer = ptr::null_mut();
    let sample_size = unsafe { block.data_length() };
    let status = unsafe {
        CMSampleBuffer::create_ready(
            None,
            Some(block),
            Some(description),
            1,
            0,
            ptr::null(),
            1,
            &sample_size,
            std::ptr::NonNull::new(&mut sample as *mut *mut CMSampleBuffer)
                .ok_or("invalid CMSampleBuffer pointer")?,
        )
    };
    if status != 0 {
        return Err(format!(
            "CMSampleBufferCreateReady failed: OSStatus {status}"
        ));
    }
    Ok(unsafe {
        CFRetained::from_raw(
            std::ptr::NonNull::new(sample).ok_or("CoreMedia returned null sample buffer")?,
        )
    })
}

unsafe fn copy_pixel_buffer_to_bgra(
    pixel_buffer: &CVImageBuffer,
    expected_width: usize,
    expected_height: usize,
) -> Result<Vec<u8>, String> {
    let lock_flags = CVPixelBufferLockFlags::ReadOnly;
    let status = CVPixelBufferLockBaseAddress(pixel_buffer, lock_flags);
    if status != 0 {
        return Err(format!(
            "CVPixelBufferLockBaseAddress failed: CVReturn {status}"
        ));
    }
    let result = copy_locked_pixel_buffer_to_bgra(pixel_buffer, expected_width, expected_height);
    let _ = CVPixelBufferUnlockBaseAddress(pixel_buffer, lock_flags);
    result
}

fn copy_locked_pixel_buffer_to_bgra(
    pixel_buffer: &CVImageBuffer,
    expected_width: usize,
    expected_height: usize,
) -> Result<Vec<u8>, String> {
    let width = CVPixelBufferGetWidth(pixel_buffer);
    let height = CVPixelBufferGetHeight(pixel_buffer);
    if width != expected_width || height != expected_height {
        return Err(format!(
            "VideoToolbox decoded atlas size mismatch: expected {expected_width}x{expected_height}, got {width}x{height}"
        ));
    }
    let format = CVPixelBufferGetPixelFormatType(pixel_buffer);
    if format == kCVPixelFormatType_32BGRA {
        copy_bgra_pixel_buffer(pixel_buffer, width, height)
    } else if format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange {
        copy_nv12_pixel_buffer(pixel_buffer, width, height, false)
    } else if format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange {
        copy_nv12_pixel_buffer(pixel_buffer, width, height, true)
    } else {
        Err(format!(
            "unsupported VideoToolbox decoded pixel format: 0x{format:08x}"
        ))
    }
}

fn copy_bgra_pixel_buffer(
    pixel_buffer: &CVImageBuffer,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, String> {
    let base = CVPixelBufferGetBaseAddress(pixel_buffer);
    if base.is_null() {
        return Err("VideoToolbox BGRA pixel buffer has null base address".to_string());
    }
    let stride = CVPixelBufferGetBytesPerRow(pixel_buffer);
    let row_bytes = width
        .checked_mul(4)
        .ok_or_else(|| "decoded BGRA row byte count overflow".to_string())?;
    if stride < row_bytes {
        return Err("VideoToolbox BGRA stride is smaller than row bytes".to_string());
    }
    let mut out = vec![
        0u8;
        row_bytes
            .checked_mul(height)
            .ok_or_else(|| "decoded BGRA frame byte count overflow".to_string())?
    ];
    for row in 0..height {
        let src =
            unsafe { slice::from_raw_parts((base as *const u8).add(row * stride), row_bytes) };
        let dst = row * row_bytes;
        out[dst..dst + row_bytes].copy_from_slice(src);
    }
    Ok(out)
}

fn copy_nv12_pixel_buffer(
    pixel_buffer: &CVImageBuffer,
    width: usize,
    height: usize,
    full_range: bool,
) -> Result<Vec<u8>, String> {
    if objc2_core_video::CVPixelBufferGetPlaneCount(pixel_buffer) < 2 {
        return Err("VideoToolbox NV12 pixel buffer is missing planes".to_string());
    }
    let y_base = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 0);
    let uv_base = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 1);
    if y_base.is_null() || uv_base.is_null() {
        return Err("VideoToolbox NV12 pixel buffer has null plane address".to_string());
    }
    let y_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 0);
    let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 1);
    let y_width = CVPixelBufferGetWidthOfPlane(pixel_buffer, 0);
    let y_height = CVPixelBufferGetHeightOfPlane(pixel_buffer, 0);
    let uv_width = CVPixelBufferGetWidthOfPlane(pixel_buffer, 1);
    let uv_height = CVPixelBufferGetHeightOfPlane(pixel_buffer, 1);
    if y_width < width || y_height < height || uv_width < width / 2 || uv_height < height / 2 {
        return Err("VideoToolbox NV12 plane dimensions are smaller than atlas".to_string());
    }
    let mut out = vec![
        0u8;
        width
            .checked_mul(height)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| "decoded NV12 frame byte count overflow".to_string())?
    ];
    for y in 0..height {
        for x in 0..width {
            let yy = unsafe { *((y_base as *const u8).add(y * y_stride + x)) };
            let uv = unsafe { (uv_base as *const u8).add((y / 2) * uv_stride + (x / 2) * 2) };
            let cb = unsafe { *uv };
            let cr = unsafe { *uv.add(1) };
            let (r, g, b) = nv12_to_rgb(yy, cb, cr, full_range);
            let dst = (y * width + x) * 4;
            out[dst] = b;
            out[dst + 1] = g;
            out[dst + 2] = r;
            out[dst + 3] = 0xff;
        }
    }
    Ok(out)
}

fn nv12_to_rgb(y: u8, cb: u8, cr: u8, full_range: bool) -> (u8, u8, u8) {
    let cb = i32::from(cb) - 128;
    let cr = i32::from(cr) - 128;
    let c = if full_range {
        i32::from(y) << 8
    } else {
        (i32::from(y) - 16).max(0) * 298
    };
    let r = (c + 409 * cr + 128) >> 8;
    let g = (c - 100 * cb - 208 * cr + 128) >> 8;
    let b = (c + 516 * cb + 128) >> 8;
    (clamp(r), clamp(g), clamp(b))
}

fn clamp(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

fn avcc_payload(nals: &[&[u8]]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for nal in nals {
        if nal.is_empty() || nal_type(nal) == 9 {
            continue;
        }
        let len = u32::try_from(nal.len())
            .map_err(|_| "H.264 NAL unit is too large for AVCC length prefix".to_string())?;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nal);
    }
    Ok(out)
}

fn annex_b_nals(bytes: &[u8]) -> Vec<&[u8]> {
    let starts = annex_b_start_codes(bytes);
    let mut nals = Vec::new();
    for (index, (prefix_start, prefix_len)) in starts.iter().enumerate() {
        let nal_start = prefix_start + prefix_len;
        let nal_end = starts
            .get(index + 1)
            .map(|(next_start, _)| *next_start)
            .unwrap_or(bytes.len());
        if nal_start < nal_end {
            nals.push(trim_trailing_zeroes(&bytes[nal_start..nal_end]));
        }
    }
    nals.into_iter().filter(|nal| !nal.is_empty()).collect()
}

fn annex_b_start_codes(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 3 <= bytes.len() {
        if bytes[i] == 0 && bytes[i + 1] == 0 {
            if bytes[i + 2] == 1 {
                out.push((i, 3));
                i += 3;
                continue;
            }
            if i + 4 <= bytes.len() && bytes[i + 2] == 0 && bytes[i + 3] == 1 {
                out.push((i, 4));
                i += 4;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn trim_trailing_zeroes(mut bytes: &[u8]) -> &[u8] {
    while bytes.last() == Some(&0) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn nal_type(nal: &[u8]) -> u8 {
    nal.first().copied().unwrap_or(0) & 0x1f
}
