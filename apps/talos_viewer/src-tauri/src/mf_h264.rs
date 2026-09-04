#![cfg(windows)]
#![allow(dead_code)]

use std::{os::raw::c_int, sync::OnceLock};

use tracing::debug;
use windows::{
    core::{IUnknown, Interface, Type},
    Win32::{
        Foundation::VARIANT_BOOL,
        Graphics::{
            Direct3D11::{
                ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_DEFAULT,
            },
            Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC},
        },
        Media::MediaFoundation::{
            CMSH264DecoderMFT, CODECAPI_AVDecVideoAcceleration_H264, CODECAPI_AVLowLatencyMode,
            ICodecAPI, IMFDXGIBuffer, IMFDXGIDeviceManager, IMFMediaBuffer, IMFMediaType,
            IMFSample, IMFTransform, MFCreateDXGIDeviceManager, MFCreateDXGISurfaceBuffer,
            MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFStartup,
            MFVideoFormat_H264, MFVideoFormat_I420, MFVideoFormat_NV12,
            MFVideoInterlace_Progressive, MFSTARTUP_FULL,
            MFT_DECODER_EXPOSE_OUTPUT_TYPES_IN_NATIVE_ORDER, MFT_MESSAGE_COMMAND_FLUSH,
            MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_END_OF_STREAM,
            MFT_MESSAGE_NOTIFY_END_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
            MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER,
            MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MF_E_TRANSFORM_NEED_MORE_INPUT,
            MF_E_TRANSFORM_STREAM_CHANGE, MF_LOW_LATENCY, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
            MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
            MF_SA_D3D11_AWARE, MF_VERSION,
        },
        System::{
            Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED},
            Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL},
        },
    },
};

static MF_RUNTIME_INIT: OnceLock<Result<(), String>> = OnceLock::new();

pub struct H264Decoder {
    transform: IMFTransform,
    width: u32,
    height: u32,
    output_buffer_len: u32,
    output_provides_samples: bool,
}

unsafe impl Send for H264Decoder {}

pub struct DecodedSurfaceFrame {
    pub texture: ID3D11Texture2D,
}

pub struct D3d11H264Decoder {
    transform: IMFTransform,
    _device_manager: IMFDXGIDeviceManager,
    decoded_texture: ID3D11Texture2D,
    width: u32,
    height: u32,
    output_provides_samples: bool,
}

unsafe impl Send for D3d11H264Decoder {}

impl H264Decoder {
    pub fn new(width: u32, height: u32, fps: u32) -> Result<Self, String> {
        ensure_media_foundation_started()?;
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(format!(
                "h264 decoder requires even non-zero dimensions, got {width}x{height}"
            ));
        }
        let fps = fps.max(1);
        let transform: IMFTransform = unsafe {
            CoCreateInstance(&CMSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)
                .map_err(|err| format!("CoCreateInstance(CMSH264DecoderMFT) failed: {err}"))?
        };
        let codec_api = transform.cast::<ICodecAPI>().ok();
        if let Ok(attributes) = unsafe { transform.GetAttributes() } {
            let _ = unsafe {
                attributes.SetUINT32(&MFT_DECODER_EXPOSE_OUTPUT_TYPES_IN_NATIVE_ORDER, 1)
            };
            let _ = unsafe { attributes.SetUINT32(&MF_LOW_LATENCY, 1) };
        }
        if let Some(codec_api) = codec_api.as_ref() {
            let _ = set_codec_bool(codec_api, &CODECAPI_AVDecVideoAcceleration_H264, true);
            let _ = set_codec_bool(codec_api, &CODECAPI_AVLowLatencyMode, true);
        }

        let input_type = unsafe { MFCreateMediaType() }
            .map_err(|err| format!("MFCreateMediaType input failed: {err}"))?;
        set_common_video_type_attributes(&input_type, width, height, fps)?;
        unsafe {
            input_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|err| format!("set decoder input major type failed: {err}"))?;
            input_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(|err| format!("set decoder input subtype failed: {err}"))?;
            transform
                .SetInputType(0, &input_type, 0)
                .map_err(|err| format!("decoder SetInputType(H264) failed: {err}"))?;
        }

        set_i420_output_type(&transform, width, height, fps)?;
        unsafe {
            transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)
                .map_err(|err| format!("decoder flush failed: {err}"))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|err| format!("decoder begin streaming failed: {err}"))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|err| format!("decoder start stream failed: {err}"))?;
        }
        let output_info = unsafe { transform.GetOutputStreamInfo(0) }
            .map_err(|err| format!("decoder GetOutputStreamInfo failed: {err}"))?;

        let decoder = Self {
            transform,
            width,
            height,
            output_buffer_len: output_info.cbSize.max((width * height * 3 / 2).max(4096)),
            output_provides_samples: (output_info.dwFlags
                & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32)
                != 0,
        };
        debug!(
            width,
            height,
            fps,
            output_buffer_len = decoder.output_buffer_len,
            output_provides_samples = decoder.output_provides_samples,
            "viewer h264 cpu decoder initialized"
        );
        Ok(decoder)
    }

    pub fn decode(&mut self, payload: &[u8]) -> Result<Option<Vec<u8>>, String> {
        debug!(
            payload_len = payload.len(),
            width = self.width,
            height = self.height,
            "viewer h264 cpu decoder input"
        );
        let sample = create_media_sample(payload)?;
        unsafe {
            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(|err| format!("decoder ProcessInput failed: {err}"))?;
        }
        self.process_output()
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    fn process_output(&mut self) -> Result<Option<Vec<u8>>, String> {
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
                debug!(
                    width = self.width,
                    height = self.height,
                    "viewer h264 cpu decoder needs more input"
                );
                return Ok(None);
            }
            Err(err) if err.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                cleanup_output_buffer(&mut output[0]);
                debug!(
                    width = self.width,
                    height = self.height,
                    "viewer h264 cpu decoder stream change"
                );
                set_i420_output_type(&self.transform, self.width, self.height, 30)?;
                return self.process_output();
            }
            Err(err) => {
                cleanup_output_buffer(&mut output[0]);
                return Err(format!("decoder ProcessOutput failed: {err}"));
            }
        }
        let sample = unsafe { std::mem::ManuallyDrop::take(&mut output[0].pSample) };
        let _ = unsafe { std::mem::ManuallyDrop::take(&mut output[0].pEvents) };
        let sample = sample.ok_or_else(|| "decoder returned no output sample".to_string())?;
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }
            .map_err(|err| format!("decoder ConvertToContiguousBuffer failed: {err}"))?;
        let i420 = read_media_buffer(&buffer)?;
        let expected = (self.width as usize * self.height as usize * 3) / 2;
        if i420.len() < expected {
            return Err(format!(
                "decoder output length mismatch: expected at least {expected}, got {}",
                i420.len()
            ));
        }
        let bgra = i420_to_bgra(&i420[..expected], self.width, self.height)?;
        debug!(
            width = self.width,
            height = self.height,
            i420_len = i420.len(),
            bgra_len = bgra.len(),
            "viewer h264 cpu decoder output"
        );
        Ok(Some(bgra))
    }
}

impl D3d11H264Decoder {
    pub fn new(
        device: ID3D11Device,
        _device_context: ID3D11DeviceContext,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self, String> {
        ensure_media_foundation_started()?;
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(format!(
                "d3d11 h264 decoder requires even non-zero dimensions, got {width}x{height}"
            ));
        }
        let fps = fps.max(1);
        let decoded_texture = create_texture(&device, width, height, DXGI_FORMAT_NV12, 0)?;
        let device_manager = create_dxgi_device_manager(&device)?;

        let transform: IMFTransform = unsafe {
            CoCreateInstance(&CMSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)
                .map_err(|err| format!("CoCreateInstance(CMSH264DecoderMFT) failed: {err}"))?
        };
        let codec_api = transform.cast::<ICodecAPI>().ok();
        let attributes = unsafe { transform.GetAttributes() }
            .map_err(|err| format!("decoder GetAttributes failed: {err}"))?;
        let d3d11_aware = unsafe { attributes.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) != 0 };
        if !d3d11_aware {
            return Err("decoder transform is not D3D11-aware".to_string());
        }
        let _ =
            unsafe { attributes.SetUINT32(&MFT_DECODER_EXPOSE_OUTPUT_TYPES_IN_NATIVE_ORDER, 1) };
        let _ = unsafe { attributes.SetUINT32(&MF_LOW_LATENCY, 1) };
        unsafe {
            transform
                .ProcessMessage(
                    MFT_MESSAGE_SET_D3D_MANAGER,
                    device_manager.as_raw() as usize,
                )
                .map_err(|err| format!("decoder set d3d manager failed: {err}"))?;
        }
        if let Some(codec_api) = codec_api.as_ref() {
            let _ = set_codec_bool(codec_api, &CODECAPI_AVDecVideoAcceleration_H264, true);
            let _ = set_codec_bool(codec_api, &CODECAPI_AVLowLatencyMode, true);
        }

        let input_type = unsafe { MFCreateMediaType() }
            .map_err(|err| format!("MFCreateMediaType input failed: {err}"))?;
        set_common_video_type_attributes(&input_type, width, height, fps)?;
        unsafe {
            input_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|err| format!("set decoder input major type failed: {err}"))?;
            input_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(|err| format!("set decoder input subtype failed: {err}"))?;
            transform
                .SetInputType(0, &input_type, 0)
                .map_err(|err| format!("decoder SetInputType(H264) failed: {err}"))?;
        }

        set_nv12_output_type(&transform, width, height, fps)?;
        unsafe {
            transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)
                .map_err(|err| format!("decoder flush failed: {err}"))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|err| format!("decoder begin streaming failed: {err}"))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|err| format!("decoder start stream failed: {err}"))?;
        }

        let output_info = unsafe { transform.GetOutputStreamInfo(0) }
            .map_err(|err| format!("decoder GetOutputStreamInfo failed: {err}"))?;

        let decoder = Self {
            transform,
            _device_manager: device_manager,
            decoded_texture,
            width,
            height,
            output_provides_samples: (output_info.dwFlags
                & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32)
                != 0,
        };
        debug!(
            width,
            height,
            fps,
            output_provides_samples = decoder.output_provides_samples,
            "viewer h264 d3d11 decoder initialized"
        );
        Ok(decoder)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn decode(&mut self, payload: &[u8]) -> Result<Option<DecodedSurfaceFrame>, String> {
        debug!(
            payload_len = payload.len(),
            width = self.width,
            height = self.height,
            "viewer h264 d3d11 decoder input"
        );
        let sample = create_media_sample(payload)?;
        unsafe {
            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(|err| format!("decoder ProcessInput failed: {err}"))?;
        }
        self.process_output()
    }

    fn process_output(&mut self) -> Result<Option<DecodedSurfaceFrame>, String> {
        let output_sample = if self.output_provides_samples {
            None
        } else {
            Some(create_dxgi_surface_sample(&self.decoded_texture)?)
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
                debug!(
                    width = self.width,
                    height = self.height,
                    "viewer h264 d3d11 decoder needs more input"
                );
                return Ok(None);
            }
            Err(err) if err.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                cleanup_output_buffer(&mut output[0]);
                debug!(
                    width = self.width,
                    height = self.height,
                    "viewer h264 d3d11 decoder stream change"
                );
                set_nv12_output_type(&self.transform, self.width, self.height, 30)?;
                return self.process_output();
            }
            Err(err) => {
                cleanup_output_buffer(&mut output[0]);
                return Err(format!("decoder ProcessOutput failed: {err}"));
            }
        }
        let sample = unsafe { std::mem::ManuallyDrop::take(&mut output[0].pSample) };
        let _ = unsafe { std::mem::ManuallyDrop::take(&mut output[0].pEvents) };
        let sample = sample.ok_or_else(|| "decoder returned no output sample".to_string())?;
        let texture = sample_texture(&sample)?;
        debug!(
            width = self.width,
            height = self.height,
            "viewer h264 d3d11 decoder output texture"
        );
        Ok(Some(DecodedSurfaceFrame { texture }))
    }
}

impl Drop for H264Decoder {
    fn drop(&mut self) {
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

impl Drop for D3d11H264Decoder {
    fn drop(&mut self) {
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
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }
            .map_err(|err| format!("MFStartup failed: {err}"))
    });
    match init {
        Ok(()) => Ok(()),
        Err(err) => Err(err.clone()),
    }
}

fn set_i420_output_type(
    transform: &IMFTransform,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(), String> {
    let output_type = unsafe { MFCreateMediaType() }
        .map_err(|err| format!("MFCreateMediaType output failed: {err}"))?;
    set_common_video_type_attributes(&output_type, width, height, fps)?;
    unsafe {
        output_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|err| format!("set decoder output major type failed: {err}"))?;
        output_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_I420)
            .map_err(|err| format!("set decoder output subtype failed: {err}"))?;
        transform
            .SetOutputType(0, &output_type, 0)
            .map_err(|err| format!("decoder SetOutputType(I420) failed: {err}"))?;
    }
    Ok(())
}

fn set_nv12_output_type(
    transform: &IMFTransform,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(), String> {
    let output_type = unsafe { MFCreateMediaType() }
        .map_err(|err| format!("MFCreateMediaType output failed: {err}"))?;
    set_common_video_type_attributes(&output_type, width, height, fps)?;
    unsafe {
        output_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|err| format!("set decoder output major type failed: {err}"))?;
        output_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(|err| format!("set decoder output subtype failed: {err}"))?;
        transform
            .SetOutputType(0, &output_type, 0)
            .map_err(|err| format!("decoder SetOutputType(NV12) failed: {err}"))?;
    }
    Ok(())
}

fn set_common_video_type_attributes(
    media_type: &IMFMediaType,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(), String> {
    let frame_size = ((width as u64) << 32) | height as u64;
    let frame_rate = ((fps.max(1) as u64) << 32) | 1u64;
    let pixel_aspect_ratio = (1u64 << 32) | 1u64;
    unsafe {
        media_type
            .SetUINT64(&MF_MT_FRAME_SIZE, frame_size)
            .map_err(|err| format!("set frame size failed: {err}"))?;
        media_type
            .SetUINT64(&MF_MT_FRAME_RATE, frame_rate)
            .map_err(|err| format!("set frame rate failed: {err}"))?;
        media_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pixel_aspect_ratio)
            .map_err(|err| format!("set pixel aspect ratio failed: {err}"))?;
        media_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|err| format!("set interlace mode failed: {err}"))?;
    }
    Ok(())
}

fn set_codec_bool(
    codec_api: &ICodecAPI,
    key: &windows::core::GUID,
    value: bool,
) -> Result<(), String> {
    let variant = variant_from_bool(value);
    unsafe {
        codec_api
            .SetValue(key, &variant)
            .map_err(|err| format!("ICodecAPI::SetValue({key:?}) failed: {err}"))
    }
}

fn variant_from_bool(value: bool) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_BOOL,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 {
                    boolVal: VARIANT_BOOL(if value { -1 } else { 0 }),
                },
            }),
        },
    }
}

fn create_media_sample(payload: &[u8]) -> Result<IMFSample, String> {
    let sample = create_empty_sample(payload.len() as u32)?;
    let buffer = unsafe { sample.GetBufferByIndex(0) }
        .map_err(|err| format!("GetBufferByIndex failed: {err}"))?;
    write_media_buffer(&buffer, payload)?;
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

fn create_dxgi_surface_sample(texture: &ID3D11Texture2D) -> Result<IMFSample, String> {
    let sample =
        unsafe { MFCreateSample() }.map_err(|err| format!("MFCreateSample failed: {err}"))?;
    let surface_unknown: IUnknown = texture
        .cast()
        .map_err(|err| format!("cast texture to IUnknown failed: {err}"))?;
    let buffer = unsafe {
        MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &surface_unknown, 0, false)
            .map_err(|err| format!("MFCreateDXGISurfaceBuffer failed: {err}"))?
    };
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

fn cleanup_output_buffer(output: &mut MFT_OUTPUT_DATA_BUFFER) {
    let _ = unsafe { std::mem::ManuallyDrop::take(&mut output.pSample) };
    let _ = unsafe { std::mem::ManuallyDrop::take(&mut output.pEvents) };
}

fn create_dxgi_device_manager(device: &ID3D11Device) -> Result<IMFDXGIDeviceManager, String> {
    let mut reset_token = 0u32;
    let mut manager = None;
    unsafe {
        MFCreateDXGIDeviceManager(&mut reset_token, &mut manager)
            .map_err(|err| format!("MFCreateDXGIDeviceManager failed: {err}"))?;
    }
    let manager = manager.ok_or_else(|| "MFCreateDXGIDeviceManager returned null".to_string())?;
    let device_unknown: IUnknown = device
        .cast()
        .map_err(|err| format!("cast ID3D11Device to IUnknown failed: {err}"))?;
    unsafe {
        manager
            .ResetDevice(&device_unknown, reset_token)
            .map_err(|err| format!("IMFDXGIDeviceManager::ResetDevice failed: {err}"))?;
    }
    Ok(manager)
}

fn create_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    bind_flags: u32,
) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind_flags,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe {
        device
            .CreateTexture2D(&desc, None, Some(&mut texture))
            .map_err(|err| format!("CreateTexture2D({format:?}) failed: {err}"))?;
    }
    texture.ok_or_else(|| "CreateTexture2D returned null".to_string())
}

fn sample_texture(sample: &IMFSample) -> Result<ID3D11Texture2D, String> {
    let buffer = unsafe { sample.GetBufferByIndex(0) }
        .map_err(|err| format!("GetBufferByIndex failed: {err}"))?;
    let dxgi_buffer: IMFDXGIBuffer = buffer
        .cast()
        .map_err(|err| format!("cast IMFMediaBuffer to IMFDXGIBuffer failed: {err}"))?;
    let mut texture_ptr = std::ptr::null_mut();
    unsafe {
        dxgi_buffer
            .GetResource(&ID3D11Texture2D::IID, &mut texture_ptr)
            .map_err(|err| format!("IMFDXGIBuffer::GetResource failed: {err}"))?;
        ID3D11Texture2D::from_abi(texture_ptr as _)
            .map_err(|err| format!("ID3D11Texture2D::from_abi failed: {err}"))
    }
}

fn i420_to_bgra(i420: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let y_len = width as usize * height as usize;
    let uv_len = y_len / 4;
    if i420.len() != y_len + uv_len * 2 {
        return Err(format!(
            "i420 buffer length mismatch: expected {}, got {}",
            y_len + uv_len * 2,
            i420.len()
        ));
    }
    let (y_plane, rest) = i420.split_at(y_len);
    let (u_plane, v_plane) = rest.split_at(uv_len);
    let mut bgra = vec![0u8; width as usize * height as usize * 4];
    // Libyuv's ARGB byte layout is BGRA on little-endian Windows. Keep this
    // paired with the agent's ARGBToI420 conversion so red/blue channels do not swap.
    let ret = unsafe {
        yuv_sys::rs_I420ToARGB(
            y_plane.as_ptr(),
            width as c_int,
            u_plane.as_ptr(),
            (width / 2) as c_int,
            v_plane.as_ptr(),
            (width / 2) as c_int,
            bgra.as_mut_ptr(),
            (width * 4) as c_int,
            width as c_int,
            height as c_int,
        )
    };
    if ret != 0 {
        return Err(format!("libyuv I420ToARGB failed with code {ret}"));
    }
    Ok(bgra)
}
