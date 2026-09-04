#![cfg(windows)]

use std::sync::OnceLock;
use tracing::{debug, info, warn};
use windows::core::Error as WinError;

use windows::{
    core::{Interface, VARIANT},
    Win32::{
        Media::MediaFoundation::{
            eAVEncCommonRateControlMode_CBR, eAVEncH264VProfile_Base, CMSH264EncoderMFT,
            CODECAPI_AVEncCommonBufferSize, CODECAPI_AVEncCommonMaxBitRate,
            CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonQualityVsSpeed,
            CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncMPVDefaultBPictureCount,
            CODECAPI_AVEncMPVGOPSize, CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode,
            ICodecAPI, IMFMediaBuffer, IMFMediaType, IMFSample, IMFTransform, MFCreateMediaType,
            MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFSampleExtension_CleanPoint,
            MFStartup, MFVideoFormat_H264, MFVideoFormat_I420, MFVideoFormat_NV12,
            MFVideoInterlace_Progressive, MFSTARTUP_FULL, MFT_MESSAGE_COMMAND_FLUSH,
            MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_END_OF_STREAM,
            MFT_MESSAGE_NOTIFY_END_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
            MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
            MF_E_TRANSFORM_NEED_MORE_INPUT, MF_LOW_LATENCY, MF_MT_ALL_SAMPLES_INDEPENDENT,
            MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FIXED_SIZE_SAMPLES, MF_MT_FRAME_RATE,
            MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE,
            MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SAMPLE_SIZE, MF_MT_SUBTYPE,
            MF_VERSION,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
        },
    },
};

static MF_RUNTIME_INIT: OnceLock<Result<(), String>> = OnceLock::new();

/// Tracing target for Media Foundation / H.264 MFT (`CMSH264EncoderMFT`) events. Use
/// `RMM_LOGLEVEL=info` for encoder lifecycle, `debug` for detailed steps, `warn` always includes failures.
const MF_LOG_TARGET: &str = "rmm_media_foundation";

fn log_mf_win_err(step: &'static str, err: &WinError) {
    warn!(
        target: MF_LOG_TARGET,
        step,
        hresult = format_args!("0x{:08X}", err.code().0 as u32),
        error = %err,
        "Media Foundation call failed"
    );
}

fn log_mf_str_err(step: &'static str, message: &str) {
    warn!(
        target: MF_LOG_TARGET,
        step,
        error = %message,
        "Media Foundation error"
    );
}

const H264_LOW_LATENCY_BUFFER_WINDOW_MS: u64 = 100;
const H264_LOW_LATENCY_MIN_BUFFER_BYTES: u32 = 64 * 1024;
const H264_LOW_LATENCY_MAX_BUFFER_BYTES: u32 = 512 * 1024;
const H264_LOW_LATENCY_MAX_GOP_FRAMES: u32 = 30;
const H264_LOW_LATENCY_QUALITY_VS_SPEED: u32 = 0;

#[derive(Clone, Copy)]
struct H264LowLatencySettings {
    mean_bitrate: u32,
    max_bitrate: u32,
    buffer_size_bytes: u32,
    gop_size: u32,
    quality_vs_speed: u32,
}

pub struct EncodedH264Frame {
    pub payload: Vec<u8>,
    pub clean_point: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum H264InputFormat {
    Nv12,
    I420,
}

impl H264InputFormat {
    fn subtype(self) -> windows::core::GUID {
        match self {
            Self::Nv12 => MFVideoFormat_NV12,
            Self::I420 => MFVideoFormat_I420,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Nv12 => "NV12",
            Self::I420 => "I420",
        }
    }
}

pub struct H264Encoder {
    transform: IMFTransform,
    codec_api: Option<ICodecAPI>,
    width: u32,
    height: u32,
    input_format: H264InputFormat,
    frame_duration_100ns: i64,
    next_sample_time_100ns: i64,
    output_buffer_len: u32,
    output_provides_samples: bool,
    sequence_header: Vec<u8>,
}

impl H264Encoder {
    pub fn new(
        width: u32,
        height: u32,
        fps: u32,
        bitrate_bps: Option<u32>,
    ) -> Result<Self, String> {
        ensure_media_foundation_started()?;
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            let msg =
                format!("h264 encoder requires even non-zero dimensions, got {width}x{height}");
            log_mf_str_err("H264Encoder::new_validate_dimensions", &msg);
            return Err(msg);
        }
        let fps = fps.max(1);
        let frame_duration_100ns = (10_000_000u64 / fps as u64) as i64;
        info!(
            target: MF_LOG_TARGET,
            encoder = "cpu_mft",
            width,
            height,
            fps,
            bitrate_bps,
            "creating Media Foundation H.264 encoder (software input path)"
        );
        let transform: IMFTransform = unsafe {
            CoCreateInstance(&CMSH264EncoderMFT, None, CLSCTX_INPROC_SERVER).map_err(|err| {
                log_mf_win_err("CoCreateInstance_CMSH264EncoderMFT", &err);
                format!("CoCreateInstance(CMSH264EncoderMFT) failed: {err}")
            })?
        };
        let codec_api = transform.cast::<ICodecAPI>().ok();
        enable_transform_low_latency(&transform);
        let settings = default_h264_low_latency_settings(width, height, fps, bitrate_bps);
        apply_h264_low_latency_codec_settings(codec_api.as_ref(), settings);
        let output_type = unsafe { MFCreateMediaType() }.map_err(|err| {
            log_mf_win_err("MFCreateMediaType_output_cpu_mft", &err);
            format!("MFCreateMediaType output failed: {err}")
        })?;
        set_common_video_type_attributes(&output_type, width, height, fps)?;
        unsafe {
            output_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|err| {
                    log_mf_win_err("output_SetGUID_MAJOR_TYPE_cpu", &err);
                    format!("set output major type failed: {err}")
                })?;
            output_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(|err| {
                    log_mf_win_err("output_SetGUID_SUBTYPE_H264_cpu", &err);
                    format!("set output subtype failed: {err}")
                })?;
            output_type
                .SetUINT32(&MF_MT_AVG_BITRATE, settings.mean_bitrate)
                .map_err(|err| {
                    log_mf_win_err("output_SetUINT32_AVG_BITRATE_cpu", &err);
                    format!("set output bitrate failed: {err}")
                })?;
            output_type
                .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)
                .map_err(|err| {
                    log_mf_win_err("output_SetUINT32_MPEG2_PROFILE_cpu", &err);
                    format!("set output profile failed: {err}")
                })?;
            output_type
                .SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)
                .map_err(|err| {
                    log_mf_win_err("output_SetUINT32_ALL_SAMPLES_INDEPENDENT_cpu", &err);
                    format!("set output sample independence failed: {err}")
                })?;
            transform.SetOutputType(0, &output_type, 0).map_err(|err| {
                log_mf_win_err("SetOutputType_H264_cpu", &err);
                format!("SetOutputType(H264) failed: {err}")
            })?;
        }

        let input_type = unsafe { MFCreateMediaType() }.map_err(|err| {
            log_mf_win_err("MFCreateMediaType_input_cpu", &err);
            format!("MFCreateMediaType input failed: {err}")
        })?;
        let input_format = configure_transform_input_type(
            &transform,
            &input_type,
            width,
            height,
            fps,
            &[H264InputFormat::Nv12, H264InputFormat::I420],
        )?;
        info!(
            target: MF_LOG_TARGET,
            encoder = "cpu_mft",
            width,
            height,
            fps,
            input_format = input_format.label(),
            mean_bitrate = settings.mean_bitrate,
            "Media Foundation H.264 encoder input type negotiated"
        );

        send_startup_process_messages(&transform, "media foundation h264 encoder")?;

        let output_info = unsafe { transform.GetOutputStreamInfo(0) }.map_err(|err| {
            log_mf_win_err("GetOutputStreamInfo_cpu", &err);
            format!("GetOutputStreamInfo failed: {err}")
        })?;
        let sequence_header = read_blob_attribute(
            unsafe { transform.GetOutputCurrentType(0) }.map_err(|err| {
                log_mf_win_err("GetOutputCurrentType_cpu", &err);
                format!("GetOutputCurrentType failed: {err}")
            })?,
            &MF_MT_MPEG_SEQUENCE_HEADER,
        )
        .unwrap_or_default();

        info!(
            target: MF_LOG_TARGET,
            encoder = "cpu_mft",
            width,
            height,
            fps,
            input_format = input_format.label(),
            mean_bitrate = settings.mean_bitrate,
            output_buffer_len = output_info.cbSize,
            output_provides_samples = (output_info.dwFlags
                & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32)
                != 0,
            sequence_header_bytes = sequence_header.len(),
            "Media Foundation H.264 encoder ready"
        );
        Ok(Self {
            transform,
            codec_api,
            width,
            height,
            input_format,
            frame_duration_100ns,
            next_sample_time_100ns: 0,
            output_buffer_len: output_info.cbSize.max((width * height).max(4096)),
            output_provides_samples: (output_info.dwFlags
                & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32)
                != 0,
            sequence_header,
        })
    }

    pub fn encode_i420(
        &mut self,
        i420: &[u8],
        force_keyframe: bool,
    ) -> Result<Option<EncodedH264Frame>, String> {
        match self.input_format {
            H264InputFormat::I420 => {
                self.encode_input_payload(H264InputFormat::I420, i420, force_keyframe)
            }
            H264InputFormat::Nv12 => {
                let nv12 = super::libyuv_convert::i420_to_nv12(i420, self.width, self.height)
                    .map_err(|err| {
                        format!("convert I420 to NV12 for h264 encoder failed: {err}")
                    })?;
                self.encode_input_payload(H264InputFormat::Nv12, &nv12, force_keyframe)
            }
        }
    }

    fn encode_input_payload(
        &mut self,
        payload: H264InputFormat,
        input_bytes: &[u8],
        force_keyframe: bool,
    ) -> Result<Option<EncodedH264Frame>, String> {
        if self.input_format != payload {
            return Err(format!(
                "h264 encoder input format mismatch: selected {}, got {}",
                self.input_format.label(),
                payload.label()
            ));
        }
        let expected = (self.width as usize * self.height as usize * 3) / 2;
        if input_bytes.len() != expected {
            return Err(format!(
                "h264 encoder input length mismatch: expected {expected}, got {}",
                input_bytes.len()
            ));
        }
        if force_keyframe {
            if let Some(codec_api) = self.codec_api.as_ref() {
                let _ = set_codec_bool(codec_api, &CODECAPI_AVEncVideoForceKeyFrame, true);
            }
        }

        let sample = create_media_sample(
            input_bytes,
            self.next_sample_time_100ns,
            self.frame_duration_100ns,
        )?;
        self.next_sample_time_100ns = self
            .next_sample_time_100ns
            .saturating_add(self.frame_duration_100ns);
        unsafe {
            self.transform.ProcessInput(0, &sample, 0).map_err(|err| {
                log_mf_win_err("H264Encoder_ProcessInput", &err);
                format!("encoder ProcessInput({}) failed: {err}", payload.label())
            })?;
        }

        let output_sample = if self.output_provides_samples {
            None
        } else {
            Some(create_empty_sample(self.output_buffer_len)?)
        };
        let mut output = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: std::mem::ManuallyDrop::new(output_sample),
            dwStatus: 0,
            pEvents: std::mem::ManuallyDrop::new(None),
        }];
        let mut output_status = 0u32;
        match unsafe {
            self.transform
                .ProcessOutput(0, &mut output, &mut output_status)
        } {
            Ok(()) => {}
            Err(err) if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                cleanup_output_buffer(&mut output[0]);
                return Ok(None);
            }
            Err(err) => {
                cleanup_output_buffer(&mut output[0]);
                log_mf_win_err("H264Encoder_ProcessOutput", &err);
                return Err(format!(
                    "encoder ProcessOutput({}) failed: {err}",
                    payload.label()
                ));
            }
        }

        let sample = unsafe { std::mem::ManuallyDrop::take(&mut output[0].pSample) };
        let _ = unsafe { std::mem::ManuallyDrop::take(&mut output[0].pEvents) };
        let sample = sample.ok_or_else(|| {
            let msg = "encoder returned no output sample".to_string();
            log_mf_str_err("H264Encoder_encode_input_payload", &msg);
            msg
        })?;
        let mut payload = read_sample_bytes(&sample)?;
        let clean_point =
            unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0 };
        if (force_keyframe || clean_point) && !self.sequence_header.is_empty() {
            let mut prefixed = self.sequence_header.clone();
            prefixed.extend_from_slice(&payload);
            payload = prefixed;
        }
        Ok(Some(EncodedH264Frame {
            payload,
            clean_point,
        }))
    }

    pub fn input_format(&self) -> H264InputFormat {
        self.input_format
    }
}

pub(crate) fn probe_h264_dirty_rect_support() -> Result<(), String> {
    info!(
        target: MF_LOG_TARGET,
        step = "probe_h264_dirty_rect_support",
        "probing Media Foundation H.264 encoder (dirty-rect capability check)"
    );
    ensure_media_foundation_started()?;
    let width = 64;
    let height = 64;
    let fps = 15;
    let transform: IMFTransform = unsafe {
        CoCreateInstance(&CMSH264EncoderMFT, None, CLSCTX_INPROC_SERVER).map_err(|err| {
            log_mf_win_err("probe_CoCreateInstance_CMSH264EncoderMFT", &err);
            format!("CoCreateInstance(CMSH264EncoderMFT) failed: {err}")
        })?
    };
    let output_type = unsafe { MFCreateMediaType() }.map_err(|err| {
        log_mf_win_err("probe_MFCreateMediaType_output", &err);
        format!("MFCreateMediaType output failed: {err}")
    })?;
    let settings = default_h264_low_latency_settings(width, height, fps, None);
    set_common_video_type_attributes(&output_type, width, height, fps)?;
    unsafe {
        output_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|err| {
                log_mf_win_err("probe_output_SetGUID_MAJOR_TYPE", &err);
                format!("set output major type failed: {err}")
            })?;
        output_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
            .map_err(|err| {
                log_mf_win_err("probe_output_SetGUID_SUBTYPE_H264", &err);
                format!("set output subtype failed: {err}")
            })?;
        output_type
            .SetUINT32(&MF_MT_AVG_BITRATE, settings.mean_bitrate)
            .map_err(|err| {
                log_mf_win_err("probe_output_SetUINT32_AVG_BITRATE", &err);
                format!("set output bitrate failed: {err}")
            })?;
        output_type
            .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)
            .map_err(|err| {
                log_mf_win_err("probe_output_SetUINT32_MPEG2_PROFILE", &err);
                format!("set output profile failed: {err}")
            })?;
        output_type
            .SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)
            .map_err(|err| {
                log_mf_win_err("probe_output_SetUINT32_ALL_SAMPLES_INDEPENDENT", &err);
                format!("set output sample independence failed: {err}")
            })?;
        transform.SetOutputType(0, &output_type, 0).map_err(|err| {
            log_mf_win_err("probe_SetOutputType_H264", &err);
            format!("SetOutputType(H264) failed: {err}")
        })?;
    }

    let input_type = unsafe { MFCreateMediaType() }.map_err(|err| {
        log_mf_win_err("probe_MFCreateMediaType_input", &err);
        format!("MFCreateMediaType input failed: {err}")
    })?;
    let _ = configure_transform_input_type(
        &transform,
        &input_type,
        width,
        height,
        fps,
        &[H264InputFormat::Nv12, H264InputFormat::I420],
    )?;

    info!(
        target: MF_LOG_TARGET,
        step = "probe_h264_dirty_rect_support",
        width,
        height,
        fps,
        "Media Foundation H.264 encoder probe succeeded"
    );
    Ok(())
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        debug!(
            target: MF_LOG_TARGET,
            encoder = "cpu_mft",
            "Media Foundation MFT: NOTIFY_END_OF_STREAM / NOTIFY_END_STREAMING"
        );
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

fn ensure_media_foundation_started() -> Result<(), String> {
    let init = MF_RUNTIME_INIT.get_or_init(|| {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        match unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) } {
            Ok(()) => {
                info!(
                    target: MF_LOG_TARGET,
                    mf_version = MF_VERSION,
                    "MFStartup ok (Media Foundation runtime ready)"
                );
                Ok(())
            }
            Err(err) => {
                log_mf_win_err("MFStartup", &err);
                Err(format!("MFStartup failed: {err}"))
            }
        }
    });
    match init {
        Ok(()) => Ok(()),
        Err(err) => Err(err.clone()),
    }
}

fn set_common_video_type_attributes(
    media_type: &IMFMediaType,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(), String> {
    let frame_size = ((width as u64) << 32) | height as u64;
    let frame_rate = ((fps as u64) << 32) | 1u64;
    let pixel_aspect_ratio = (1u64 << 32) | 1u64;
    unsafe {
        media_type
            .SetUINT64(&MF_MT_FRAME_SIZE, frame_size)
            .map_err(|err| {
                log_mf_win_err("MF_MT_FRAME_SIZE", &err);
                format!("set frame size failed: {err}")
            })?;
        media_type
            .SetUINT64(&MF_MT_FRAME_RATE, frame_rate)
            .map_err(|err| {
                log_mf_win_err("MF_MT_FRAME_RATE", &err);
                format!("set frame rate failed: {err}")
            })?;
        media_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pixel_aspect_ratio)
            .map_err(|err| {
                log_mf_win_err("MF_MT_PIXEL_ASPECT_RATIO", &err);
                format!("set pixel aspect ratio failed: {err}")
            })?;
        media_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|err| {
                log_mf_win_err("MF_MT_INTERLACE_MODE", &err);
                format!("set interlace mode failed: {err}")
            })?;
    }
    Ok(())
}

fn send_startup_process_messages(
    transform: &IMFTransform,
    encoder_label: &'static str,
) -> Result<(), String> {
    unsafe {
        if let Err(err) = transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0) {
            warn!(
                target: MF_LOG_TARGET,
                encoder = encoder_label,
                error = %err,
                hresult = format_args!("0x{:08X}", err.code().0 as u32),
                "MFT_MESSAGE_COMMAND_FLUSH rejected (continuing)"
            );
        }
        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .map_err(|err| {
                log_mf_win_err("MFT_MESSAGE_NOTIFY_BEGIN_STREAMING", &err);
                format!("encoder begin streaming failed: {err}")
            })?;
        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
            .map_err(|err| {
                log_mf_win_err("MFT_MESSAGE_NOTIFY_START_OF_STREAM", &err);
                format!("encoder start stream failed: {err}")
            })?;
    }
    Ok(())
}

fn configure_transform_input_type(
    transform: &IMFTransform,
    media_type: &IMFMediaType,
    width: u32,
    height: u32,
    fps: u32,
    preferred_formats: &[H264InputFormat],
) -> Result<H264InputFormat, String> {
    let available_formats = enumerate_transform_input_formats(transform)?;
    let mut set_errors = Vec::new();
    for format in preferred_formats {
        if !available_formats.is_empty() && !available_formats.contains(format) {
            continue;
        }
        set_common_video_type_attributes(media_type, width, height, fps)?;
        set_raw_input_video_type_attributes(media_type, width, height, *format)?;
        unsafe {
            media_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|err| {
                    log_mf_win_err("input_SetGUID_MAJOR_TYPE", &err);
                    format!("set input major type failed: {err}")
                })?;
            media_type
                .SetGUID(&MF_MT_SUBTYPE, &format.subtype())
                .map_err(|err| {
                    log_mf_win_err("input_SetGUID_SUBTYPE", &err);
                    format!("set input subtype failed: {err}")
                })?;
            if let Err(err) = transform.SetInputType(0, media_type, 0) {
                log_mf_win_err("SetInputType", &err);
                set_errors.push(format!("{}: {err}", format.label()));
                continue;
            }
        }
        return Ok(*format);
    }

    let msg = format!(
        "encoder transform does not support preferred input formats {}; available subtypes: {}; set errors: {}",
        preferred_formats
            .iter()
            .map(|format| format.label())
            .collect::<Vec<_>>()
            .join(", "),
        if available_formats.is_empty() {
            "none discovered".to_string()
        } else {
            available_formats
                .iter()
                .map(|format| format.label())
                .collect::<Vec<_>>()
                .join(", ")
        },
        if set_errors.is_empty() {
            "none".to_string()
        } else {
            set_errors.join("; ")
        }
    );
    log_mf_str_err("configure_transform_input_type", &msg);
    Err(msg)
}

fn set_raw_input_video_type_attributes(
    media_type: &IMFMediaType,
    width: u32,
    height: u32,
    format: H264InputFormat,
) -> Result<(), String> {
    let sample_size = match format {
        H264InputFormat::Nv12 | H264InputFormat::I420 => u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(3))
            .map(|bytes| bytes / 2)
            .filter(|bytes| *bytes <= u64::from(u32::MAX))
            .ok_or_else(|| format!("raw input sample size overflow for {width}x{height}"))?
            as u32,
    };
    unsafe {
        media_type
            .SetUINT32(&MF_MT_FIXED_SIZE_SAMPLES, 1)
            .map_err(|err| {
                log_mf_win_err("input_SetUINT32_FIXED_SIZE_SAMPLES", &err);
                format!("set input fixed sample size failed: {err}")
            })?;
        media_type
            .SetUINT32(&MF_MT_SAMPLE_SIZE, sample_size)
            .map_err(|err| {
                log_mf_win_err("input_SetUINT32_SAMPLE_SIZE", &err);
                format!("set input sample size failed: {err}")
            })?;
        media_type
            .SetUINT32(&MF_MT_DEFAULT_STRIDE, width)
            .map_err(|err| {
                log_mf_win_err("input_SetUINT32_DEFAULT_STRIDE", &err);
                format!("set input default stride failed: {err}")
            })?;
        media_type
            .SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)
            .map_err(|err| {
                log_mf_win_err("input_SetUINT32_ALL_SAMPLES_INDEPENDENT", &err);
                format!("set input sample independence failed: {err}")
            })?;
    }
    Ok(())
}

fn enumerate_transform_input_formats(
    transform: &IMFTransform,
) -> Result<Vec<H264InputFormat>, String> {
    let mut formats = Vec::new();
    let mut first_error: Option<String> = None;
    for index in 0..32 {
        match unsafe { transform.GetInputAvailableType(0, index) } {
            Ok(media_type) => {
                let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) }.map_err(|err| {
                    log_mf_win_err("GetInputAvailableType_GetGUID_SUBTYPE", &err);
                    format!("GetGUID(MF_MT_SUBTYPE) failed: {err}")
                })?;
                if subtype == MFVideoFormat_NV12 && !formats.contains(&H264InputFormat::Nv12) {
                    formats.push(H264InputFormat::Nv12);
                } else if subtype == MFVideoFormat_I420 && !formats.contains(&H264InputFormat::I420)
                {
                    formats.push(H264InputFormat::I420);
                }
            }
            Err(err) => {
                if index == 0 {
                    let s = format!("GetInputAvailableType(0) failed: {err}");
                    log_mf_win_err("GetInputAvailableType", &err);
                    first_error = Some(s);
                }
                break;
            }
        }
    }

    if formats.is_empty() {
        if let Some(err) = first_error {
            log_mf_str_err("enumerate_transform_input_formats", &err);
        }
    }
    Ok(formats)
}

fn set_codec_u32(
    codec_api: &ICodecAPI,
    key: &windows::core::GUID,
    value: u32,
) -> Result<(), String> {
    let variant = VARIANT::from(value);
    unsafe {
        codec_api
            .SetValue(key, &variant)
            .map_err(|err| format!("ICodecAPI::SetValue({key:?}) failed: {err}"))
    }
}

fn set_codec_bool(
    codec_api: &ICodecAPI,
    key: &windows::core::GUID,
    value: bool,
) -> Result<(), String> {
    let variant = VARIANT::from(value);
    unsafe {
        codec_api
            .SetValue(key, &variant)
            .map_err(|err| format!("ICodecAPI::SetValue({key:?}) failed: {err}"))
    }
}

fn create_media_sample(
    payload: &[u8],
    sample_time_100ns: i64,
    sample_duration_100ns: i64,
) -> Result<IMFSample, String> {
    let sample = create_empty_sample(payload.len() as u32)?;
    let buffer = unsafe { sample.GetBufferByIndex(0) }
        .map_err(|err| format!("GetBufferByIndex failed: {err}"))?;
    write_media_buffer(&buffer, payload)?;
    unsafe {
        sample
            .SetSampleTime(sample_time_100ns)
            .map_err(|err| format!("SetSampleTime failed: {err}"))?;
        sample
            .SetSampleDuration(sample_duration_100ns)
            .map_err(|err| format!("SetSampleDuration failed: {err}"))?;
    }
    Ok(sample)
}

fn create_empty_sample(buffer_len: u32) -> Result<IMFSample, String> {
    let sample =
        unsafe { MFCreateSample() }.map_err(|err| format!("MFCreateSample failed: {err}"))?;
    let buffer = unsafe { MFCreateMemoryBuffer(buffer_len) }
        .map_err(|err| format!("MFCreateMemoryBuffer failed: {err}"))?;
    unsafe {
        sample
            .AddBuffer(&buffer)
            .map_err(|err| format!("sample AddBuffer failed: {err}"))?;
    }
    Ok(sample)
}

fn write_media_buffer(buffer: &IMFMediaBuffer, payload: &[u8]) -> Result<(), String> {
    let mut dst = std::ptr::null_mut();
    let mut max_len = 0u32;
    unsafe {
        buffer
            .Lock(&mut dst, Some(&mut max_len), None)
            .map_err(|err| format!("IMFMediaBuffer::Lock failed: {err}"))?;
    }
    let result = if max_len < payload.len() as u32 || dst.is_null() {
        Err(format!(
            "media buffer too small: need {}, got {}",
            payload.len(),
            max_len
        ))
    } else {
        unsafe {
            std::ptr::copy_nonoverlapping(payload.as_ptr(), dst, payload.len());
        }
        unsafe {
            buffer
                .SetCurrentLength(payload.len() as u32)
                .map_err(|err| format!("SetCurrentLength failed: {err}"))
        }
        .map_err(|err| err.to_string())
    };
    unsafe {
        let _ = buffer.Unlock();
    }
    result
}

fn read_sample_bytes(sample: &IMFSample) -> Result<Vec<u8>, String> {
    let buffer = unsafe { sample.ConvertToContiguousBuffer() }
        .map_err(|err| format!("ConvertToContiguousBuffer failed: {err}"))?;
    read_media_buffer(&buffer)
}

fn read_media_buffer(buffer: &IMFMediaBuffer) -> Result<Vec<u8>, String> {
    let mut src = std::ptr::null_mut();
    let mut current_len = 0u32;
    unsafe {
        buffer
            .Lock(&mut src, None, Some(&mut current_len))
            .map_err(|err| format!("IMFMediaBuffer::Lock failed: {err}"))?;
    }
    let result = if src.is_null() {
        Err("media buffer returned null pointer".to_string())
    } else {
        Ok(unsafe { std::slice::from_raw_parts(src, current_len as usize) }.to_vec())
    };
    unsafe {
        let _ = buffer.Unlock();
    }
    result
}

fn read_blob_attribute(media_type: IMFMediaType, key: &windows::core::GUID) -> Option<Vec<u8>> {
    let blob_size = unsafe { media_type.GetBlobSize(key).ok()? };
    if blob_size == 0 {
        return Some(Vec::new());
    }
    let mut blob = vec![0u8; blob_size as usize];
    unsafe {
        media_type.GetBlob(key, &mut blob, None).ok()?;
    }
    Some(blob)
}

fn cleanup_output_buffer(output: &mut MFT_OUTPUT_DATA_BUFFER) {
    let _ = unsafe { std::mem::ManuallyDrop::take(&mut output.pSample) };
    let _ = unsafe { std::mem::ManuallyDrop::take(&mut output.pEvents) };
}

fn default_h264_bitrate(width: u32, height: u32, fps: u32) -> u32 {
    let pixels_per_second = width as u64 * height as u64 * fps.max(1) as u64;
    ((pixels_per_second / 8).clamp(2_000_000, 20_000_000)) as u32
}

fn default_h264_low_latency_settings(
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: Option<u32>,
) -> H264LowLatencySettings {
    let mean_bitrate = bitrate_bps
        .filter(|value| *value > 0)
        .unwrap_or_else(|| default_h264_bitrate(width, height, fps));
    let max_bitrate = mean_bitrate;
    let buffer_size_bytes = ((mean_bitrate as u64 * H264_LOW_LATENCY_BUFFER_WINDOW_MS) / 8_000)
        .clamp(
            H264_LOW_LATENCY_MIN_BUFFER_BYTES as u64,
            H264_LOW_LATENCY_MAX_BUFFER_BYTES as u64,
        ) as u32;
    let gop_size = fps.max(1).min(H264_LOW_LATENCY_MAX_GOP_FRAMES);
    H264LowLatencySettings {
        mean_bitrate,
        max_bitrate,
        buffer_size_bytes,
        gop_size,
        quality_vs_speed: H264_LOW_LATENCY_QUALITY_VS_SPEED,
    }
}

fn enable_transform_low_latency(transform: &IMFTransform) {
    if let Ok(attributes) = unsafe { transform.GetAttributes() } {
        let _ = unsafe { attributes.SetUINT32(&MF_LOW_LATENCY, 1) };
    }
}

fn apply_h264_low_latency_codec_settings(
    codec_api: Option<&ICodecAPI>,
    settings: H264LowLatencySettings,
) {
    let Some(codec_api) = codec_api else {
        return;
    };
    let _ = set_codec_u32(
        codec_api,
        &CODECAPI_AVEncCommonRateControlMode,
        eAVEncCommonRateControlMode_CBR.0 as u32,
    );
    let _ = set_codec_u32(
        codec_api,
        &CODECAPI_AVEncCommonMeanBitRate,
        settings.mean_bitrate,
    );
    let _ = set_codec_u32(
        codec_api,
        &CODECAPI_AVEncCommonMaxBitRate,
        settings.max_bitrate,
    );
    let _ = set_codec_u32(
        codec_api,
        &CODECAPI_AVEncCommonBufferSize,
        settings.buffer_size_bytes,
    );
    let _ = set_codec_u32(
        codec_api,
        &CODECAPI_AVEncCommonQualityVsSpeed,
        settings.quality_vs_speed,
    );
    let _ = set_codec_bool(codec_api, &CODECAPI_AVLowLatencyMode, true);
    let _ = set_codec_u32(codec_api, &CODECAPI_AVEncMPVDefaultBPictureCount, 0);
    let _ = set_codec_u32(codec_api, &CODECAPI_AVEncMPVGOPSize, settings.gop_size);
}
