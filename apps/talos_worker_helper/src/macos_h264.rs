#![cfg(target_os = "macos")]

use std::{ffi::c_void, ptr, slice, sync::Mutex};

use anyhow::{anyhow, ensure, Result};
use objc2_core_foundation::{
    kCFBooleanFalse, kCFBooleanTrue, CFDictionary, CFNumber, CFNumberType, CFRetained, CFString,
    CFType, Type,
};
use objc2_core_media::{
    kCMTimeInvalid, kCMVideoCodecType_H264, CMSampleBuffer, CMTime, CMVideoCodecType,
    CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
};
use objc2_core_video::{
    kCVPixelFormatType_32BGRA, CVImageBuffer, CVPixelBuffer, CVPixelBufferCreateWithBytes,
    CVPixelBufferReleaseBytesCallback,
};
use objc2_video_toolbox::{
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_ProfileLevel, kVTCompressionPropertyKey_RealTime,
    kVTEncodeFrameOptionKey_ForceKeyFrame, kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel,
    VTCompressionOutputCallback, VTCompressionSession, VTEncodeInfoFlags, VTSessionSetProperty,
};

pub(crate) struct EncodedH264Frame {
    pub(crate) payload: Vec<u8>,
    pub(crate) keyframe: bool,
}

pub(crate) struct MacosH264Encoder {
    width: u32,
    height: u32,
    fps: u32,
    frame_index: i64,
    key_interval_frames: u32,
    session: CFRetained<VTCompressionSession>,
    output: Box<EncoderOutput>,
}

struct EncoderOutput {
    samples: Mutex<Vec<Result<Vec<u8>, String>>>,
}

unsafe impl Send for MacosH264Encoder {}

impl MacosH264Encoder {
    pub(crate) fn new(width: u32, height: u32, fps: u32, bitrate_bps: u32) -> Result<Self> {
        ensure!(
            width > 0 && height > 0 && width % 2 == 0 && height % 2 == 0,
            "VideoToolbox H.264 encoder requires even non-zero dimensions, got {width}x{height}"
        );
        let fps = fps.max(1);
        let output = Box::new(EncoderOutput {
            samples: Mutex::new(Vec::new()),
        });
        let mut session: *mut VTCompressionSession = ptr::null_mut();
        let status = unsafe {
            VTCompressionSession::create(
                None,
                width as i32,
                height as i32,
                kCMVideoCodecType_H264 as CMVideoCodecType,
                None,
                None,
                None,
                compression_callback(),
                &*output as *const EncoderOutput as *mut c_void,
                std::ptr::NonNull::new(&mut session as *mut *mut VTCompressionSession)
                    .ok_or_else(|| anyhow!("invalid VideoToolbox compression session pointer"))?,
            )
        };
        ensure!(
            status == 0,
            "VTCompressionSessionCreate failed: OSStatus {status}"
        );
        let session = unsafe {
            CFRetained::from_raw(
                std::ptr::NonNull::new(session)
                    .ok_or_else(|| anyhow!("VideoToolbox returned null compression session"))?,
            )
        };

        set_bool_property(
            &session,
            unsafe { kVTCompressionPropertyKey_RealTime },
            true,
        );
        set_bool_property(
            &session,
            unsafe { kVTCompressionPropertyKey_AllowFrameReordering },
            false,
        );
        set_i32_property(
            &session,
            unsafe { kVTCompressionPropertyKey_ExpectedFrameRate },
            fps as i32,
        );
        set_i32_property(
            &session,
            unsafe { kVTCompressionPropertyKey_AverageBitRate },
            bitrate_bps.min(i32::MAX as u32) as i32,
        );
        let key_interval_frames = fps.saturating_mul(2).max(1);
        set_i32_property(
            &session,
            unsafe { kVTCompressionPropertyKey_MaxKeyFrameInterval },
            key_interval_frames.min(i32::MAX as u32) as i32,
        );
        set_string_property(
            &session,
            unsafe { kVTCompressionPropertyKey_ProfileLevel },
            unsafe { kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel },
        );
        let status = unsafe { session.prepare_to_encode_frames() };
        ensure!(
            status == 0,
            "VTCompressionSessionPrepareToEncodeFrames failed: OSStatus {status}"
        );

        Ok(Self {
            width,
            height,
            fps,
            frame_index: 0,
            key_interval_frames,
            session,
            output,
        })
    }

    pub(crate) fn encode_bgra(
        &mut self,
        bgra: &[u8],
        stride: u32,
        force_keyframe: bool,
    ) -> Result<Option<EncodedH264Frame>> {
        {
            let mut samples = self
                .output
                .samples
                .lock()
                .map_err(|_| anyhow!("VideoToolbox encoder output mutex poisoned"))?;
            samples.clear();
        }

        let pixel_buffer = create_bgra_pixel_buffer(bgra, self.width, self.height, stride)?;
        let periodic_keyframe =
            self.frame_index == 0 || self.frame_index as u32 % self.key_interval_frames == 0;
        let force_keyframe = force_keyframe || periodic_keyframe;
        let frame_properties = force_keyframe.then(force_keyframe_properties);
        let pts = unsafe { CMTime::new(self.frame_index, self.fps as i32) };
        let duration = unsafe { CMTime::new(1, self.fps as i32) };
        let mut info_flags = VTEncodeInfoFlags::empty();
        let frame_properties_ref = frame_properties
            .as_deref()
            .map(|properties| properties.as_ref());
        let status = unsafe {
            self.session.encode_frame(
                &pixel_buffer as &CVImageBuffer,
                pts,
                duration,
                frame_properties_ref,
                ptr::null_mut(),
                &mut info_flags,
            )
        };
        ensure!(
            status == 0,
            "VTCompressionSessionEncodeFrame failed: OSStatus {status}"
        );
        let status = unsafe { self.session.complete_frames(kCMTimeInvalid) };
        ensure!(
            status == 0,
            "VTCompressionSessionCompleteFrames failed: OSStatus {status}"
        );

        self.frame_index = self.frame_index.saturating_add(1);
        let mut samples = self
            .output
            .samples
            .lock()
            .map_err(|_| anyhow!("VideoToolbox encoder output mutex poisoned"))?;
        if samples.is_empty() {
            return Ok(None);
        }
        let mut payload = Vec::new();
        for sample in samples.drain(..) {
            payload.extend_from_slice(&sample.map_err(anyhow::Error::msg)?);
        }
        if payload.is_empty() {
            return Ok(None);
        }
        let keyframe = annex_b_nals(&payload).iter().any(|nal| nal_type(nal) == 5);
        Ok(Some(EncodedH264Frame { payload, keyframe }))
    }
}

impl Drop for MacosH264Encoder {
    fn drop(&mut self) {
        unsafe { self.session.invalidate() };
    }
}

unsafe extern "C-unwind" fn compression_output_callback(
    output_refcon: *mut c_void,
    _source_frame_refcon: *mut c_void,
    status: i32,
    _info_flags: VTEncodeInfoFlags,
    sample_buffer: *mut CMSampleBuffer,
) {
    if output_refcon.is_null() {
        return;
    }
    let output = unsafe { &*(output_refcon.cast::<EncoderOutput>()) };
    let result = if status != 0 {
        Err(format!(
            "VideoToolbox compression callback failed: OSStatus {status}"
        ))
    } else if sample_buffer.is_null() {
        Ok(Vec::new())
    } else {
        unsafe { sample_buffer_to_annex_b(&*sample_buffer) }
    };
    if let Ok(mut samples) = output.samples.lock() {
        samples.push(result);
    }
}

unsafe fn sample_buffer_to_annex_b(sample_buffer: &CMSampleBuffer) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    if let Some(description) = sample_buffer.format_description() {
        append_h264_parameter_sets(&description, &mut out)?;
    }
    let data = sample_buffer
        .data_buffer()
        .ok_or_else(|| "VideoToolbox sample buffer has no data buffer".to_string())?;
    let len = data.data_length();
    if len == 0 {
        return Ok(out);
    }
    let mut avcc = vec![0u8; len];
    data.copy_data_bytes(
        0,
        len,
        std::ptr::NonNull::new(avcc.as_mut_ptr().cast())
            .ok_or_else(|| "invalid AVCC copy pointer".to_string())?,
    );
    append_avcc_as_annex_b(&avcc, &mut out)?;
    Ok(out)
}

fn append_h264_parameter_sets(
    description: &objc2_core_media::CMFormatDescription,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let mut count = 0usize;
    let mut nal_header_len = 0i32;
    let status = unsafe {
        CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            description,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut count,
            &mut nal_header_len,
        )
    };
    if status != 0 {
        return Ok(());
    }
    for index in 0..count {
        let mut ptr_out: *const u8 = ptr::null();
        let mut size = 0usize;
        let status = unsafe {
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                description,
                index,
                &mut ptr_out,
                &mut size,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if status == 0 && !ptr_out.is_null() && size > 0 {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(unsafe { slice::from_raw_parts(ptr_out, size) });
        }
    }
    Ok(())
}

fn append_avcc_as_annex_b(avcc: &[u8], out: &mut Vec<u8>) -> Result<(), String> {
    let mut offset = 0usize;
    while offset + 4 <= avcc.len() {
        let len = u32::from_be_bytes([
            avcc[offset],
            avcc[offset + 1],
            avcc[offset + 2],
            avcc[offset + 3],
        ]) as usize;
        offset += 4;
        if len == 0 {
            continue;
        }
        let end = offset
            .checked_add(len)
            .ok_or_else(|| "AVCC NAL length overflow".to_string())?;
        if end > avcc.len() {
            return Err("AVCC NAL length exceeds sample buffer".to_string());
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&avcc[offset..end]);
        offset = end;
    }
    if offset != avcc.len() {
        return Err("AVCC sample has trailing partial NAL length".to_string());
    }
    Ok(())
}

fn create_bgra_pixel_buffer(
    bgra: &[u8],
    width: u32,
    height: u32,
    stride: u32,
) -> Result<CFRetained<CVPixelBuffer>> {
    let row_bytes = width as usize * 4;
    ensure!(
        stride as usize >= row_bytes,
        "BGRA stride is smaller than visible row bytes"
    );
    let mut contiguous = vec![0u8; row_bytes * height as usize];
    for row in 0..height as usize {
        let src_start = row
            .checked_mul(stride as usize)
            .ok_or_else(|| anyhow!("BGRA source row offset overflow"))?;
        let src_end = src_start
            .checked_add(row_bytes)
            .ok_or_else(|| anyhow!("BGRA source row end overflow"))?;
        ensure!(src_end <= bgra.len(), "BGRA source frame is truncated");
        let dst_start = row * row_bytes;
        contiguous[dst_start..dst_start + row_bytes].copy_from_slice(&bgra[src_start..src_end]);
    }
    let mut boxed = Box::new(contiguous);
    let base = std::ptr::NonNull::new(boxed.as_mut_ptr().cast::<c_void>())
        .ok_or_else(|| anyhow!("invalid BGRA pixel buffer base pointer"))?;
    let release_refcon = Box::into_raw(boxed).cast::<c_void>();
    let mut pixel_buffer: *mut CVPixelBuffer = ptr::null_mut();
    let status = unsafe {
        CVPixelBufferCreateWithBytes(
            None,
            width as usize,
            height as usize,
            kCVPixelFormatType_32BGRA,
            base,
            row_bytes,
            release_bytes_callback(),
            release_refcon,
            None,
            std::ptr::NonNull::new(&mut pixel_buffer as *mut *mut CVPixelBuffer)
                .ok_or_else(|| anyhow!("invalid CVPixelBuffer pointer"))?,
        )
    };
    if status != 0 {
        unsafe {
            drop(Box::from_raw(release_refcon.cast::<Vec<u8>>()));
        }
        return Err(anyhow!(
            "CVPixelBufferCreateWithBytes failed: CVReturn {status}"
        ));
    }
    Ok(unsafe {
        CFRetained::from_raw(
            std::ptr::NonNull::new(pixel_buffer)
                .ok_or_else(|| anyhow!("CoreVideo returned null pixel buffer"))?,
        )
    })
}

unsafe extern "C-unwind" fn release_pixel_buffer_bytes(
    release_refcon: *mut c_void,
    _base_address: *const c_void,
) {
    if !release_refcon.is_null() {
        unsafe {
            drop(Box::from_raw(release_refcon.cast::<Vec<u8>>()));
        }
    }
}

fn force_keyframe_properties() -> CFRetained<CFDictionary<CFString, CFType>> {
    let value: CFRetained<CFType> = unsafe { kCFBooleanTrue.unwrap().retain().into() };
    let key = unsafe { kVTEncodeFrameOptionKey_ForceKeyFrame };
    CFDictionary::from_slices(&[key], &[&value])
}

fn set_bool_property(session: &VTCompressionSession, key: &CFString, value: bool) {
    let Some(boolean) = (if value {
        unsafe { kCFBooleanTrue }
    } else {
        unsafe { kCFBooleanFalse }
    }) else {
        return;
    };
    let value: CFRetained<CFType> = boolean.retain().into();
    let _ = unsafe { VTSessionSetProperty(session_as_cf_type(session), key, Some(&value)) };
}

fn set_i32_property(session: &VTCompressionSession, key: &CFString, value: i32) {
    let Some(number) = (unsafe {
        CFNumber::new(
            None,
            CFNumberType::SInt32Type,
            (&value as *const i32).cast::<c_void>(),
        )
    }) else {
        return;
    };
    let value: CFRetained<CFType> = number.into();
    let _ = unsafe { VTSessionSetProperty(session_as_cf_type(session), key, Some(&value)) };
}

fn set_string_property(session: &VTCompressionSession, key: &CFString, value: &CFString) {
    let value: CFRetained<CFType> = value.retain().into();
    let _ = unsafe { VTSessionSetProperty(session_as_cf_type(session), key, Some(&value)) };
}

fn compression_callback() -> VTCompressionOutputCallback {
    Some(compression_output_callback)
}

fn release_bytes_callback() -> CVPixelBufferReleaseBytesCallback {
    Some(release_pixel_buffer_bytes)
}

fn session_as_cf_type(session: &VTCompressionSession) -> &CFType {
    unsafe { &*(session as *const VTCompressionSession).cast::<CFType>() }
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
