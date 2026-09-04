#![cfg(windows)]

use std::{mem, slice};

use anyhow::Result;
use windows::{
    core::{Interface, Result as WindowsResult},
    Win32::{
        Foundation::{HMODULE, RECT},
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_9_1},
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
                D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
            },
            Dxgi::{
                Common::{
                    DXGI_MODE_ROTATION, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_ROTATE180,
                    DXGI_MODE_ROTATION_ROTATE270, DXGI_MODE_ROTATION_ROTATE90,
                    DXGI_MODE_ROTATION_UNSPECIFIED,
                },
                CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput,
                IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource, IDXGISurface1,
                DXGI_ERROR_ACCESS_DENIED, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_MORE_DATA,
                DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT, DXGI_MAPPED_RECT, DXGI_MAP_READ,
                DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_MOVE_RECT, DXGI_OUTPUT_DESC,
            },
        },
        System::Com::{CoInitializeEx, COINIT_MULTITHREADED},
    },
};

use crate::capture::{DirtyRect, Frame, FrameMetadata, MoveRect, PixelFormat};

const DEFAULT_TIMEOUT_MS: u32 = 100;

#[derive(Clone)]
pub struct DxgiTextureFrame {
    texture: ID3D11Texture2D,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    raw_width: u32,
    raw_height: u32,
    rotation: DXGI_MODE_ROTATION,
}

impl DxgiTextureFrame {
    pub(crate) fn is_identity_rotation(&self) -> bool {
        matches!(
            self.rotation,
            DXGI_MODE_ROTATION_IDENTITY | DXGI_MODE_ROTATION_UNSPECIFIED
        )
    }

    pub(crate) fn texture(&self) -> ID3D11Texture2D {
        self.texture.clone()
    }
}

pub enum TextureCaptureResult {
    Frame(DxgiTextureFrame),
    Timeout,
    AccessLost,
}

pub struct GpuDxgiBackend {
    factory: IDXGIFactory1,
    duplicated_output: Option<DuplicatedOutput>,
    capture_source_index: usize,
    timeout_ms: u32,
    owned_frame: Option<DeviceTexture>,
    full_readback: Option<StagingReadback>,
}

struct DuplicatedOutput {
    device: ID3D11Device,
    device_context: ID3D11DeviceContext,
    output: IDXGIOutput,
    output_duplication: IDXGIOutputDuplication,
}

struct StagingReadback {
    texture: ID3D11Texture2D,
    surface: IDXGISurface1,
    width: u32,
    height: u32,
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
}

struct DeviceTexture {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
}

#[derive(Debug)]
enum GpuCaptureError {
    AccessDenied,
    AccessLost,
    RefreshFailure,
    Timeout,
    Fail(anyhow::Error),
}

impl std::fmt::Display for GpuCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AccessDenied => write!(f, "access to output duplication was denied"),
            Self::AccessLost => write!(f, "access to duplicated output was lost"),
            Self::RefreshFailure => write!(f, "failed to refresh output duplication"),
            Self::Timeout => write!(f, "capture operation timed out"),
            Self::Fail(err) => write!(f, "capture failed: {err}"),
        }
    }
}

impl std::error::Error for GpuCaptureError {}

impl GpuDxgiBackend {
    pub fn new() -> Result<Self> {
        ensure_com_initialized();
        let factory = create_dxgi_factory_1().map_err(|e| anyhow::anyhow!("{}", e))?;
        let mut backend = Self {
            factory,
            duplicated_output: None,
            capture_source_index: 0,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            owned_frame: None,
            full_readback: None,
        };
        backend.acquire_output_duplication()?;
        Ok(backend)
    }

    pub fn set_capture_source_index(&mut self, index: usize) -> Result<()> {
        let previous = self.capture_source_index;
        self.capture_source_index = index;
        match self.acquire_output_duplication() {
            Ok(()) => Ok(()),
            Err(e) => {
                self.capture_source_index = previous;
                Err(anyhow::anyhow!("{}", e))
            }
        }
    }

    pub fn capture_source_index(&self) -> usize {
        self.capture_source_index
    }

    pub fn try_capture_texture_with_metadata(
        &mut self,
    ) -> Result<(TextureCaptureResult, FrameMetadata)> {
        match self.capture_texture_with_metadata() {
            Ok((frame, metadata)) => Ok((TextureCaptureResult::Frame(frame), metadata)),
            Err(GpuCaptureError::Timeout) => {
                Ok((TextureCaptureResult::Timeout, FrameMetadata::default()))
            }
            Err(GpuCaptureError::AccessLost) => {
                self.owned_frame = None;
                self.full_readback = None;
                Ok((TextureCaptureResult::AccessLost, FrameMetadata::default()))
            }
            Err(e) => Err(anyhow::anyhow!("{}", e)),
        }
    }

    pub fn readback_full_frame(&mut self, frame: &DxgiTextureFrame) -> Result<Frame> {
        let desc = texture_desc(&frame.texture);
        self.ensure_full_readback(desc.Width, desc.Height, desc.Format)?;
        let duplicated_output = self
            .duplicated_output
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("output duplication unavailable"))?;
        let staging = self
            .full_readback
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("full readback texture unavailable"))?;

        unsafe {
            duplicated_output
                .device_context
                .CopyResource(&staging.texture, &frame.texture);
        }

        let data = copy_surface_to_bgra(
            &staging.surface,
            frame.raw_width as usize,
            frame.raw_height as usize,
            frame.rotation,
        )?;
        Ok(Frame {
            width: frame.width,
            height: frame.height,
            stride: frame.stride,
            format: PixelFormat::Bgra8,
            data,
        })
    }

    pub(crate) fn device(&self) -> Result<ID3D11Device> {
        self.duplicated_output
            .as_ref()
            .map(|output| output.device.clone())
            .ok_or_else(|| anyhow::anyhow!("output duplication unavailable"))
    }

    pub(crate) fn device_context(&self) -> Result<ID3D11DeviceContext> {
        self.duplicated_output
            .as_ref()
            .map(|output| output.device_context.clone())
            .ok_or_else(|| anyhow::anyhow!("output duplication unavailable"))
    }

    fn acquire_output_duplication(&mut self) -> Result<()> {
        self.duplicated_output = None;
        let mut remaining_index = self.capture_source_index;

        for i in 0.. {
            let adapter = match unsafe { self.factory.EnumAdapters1(i) } {
                Ok(adapter) => adapter,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => return Err(anyhow::anyhow!("{}", e)),
            };

            let adapter_base: IDXGIAdapter =
                adapter.cast().map_err(|e| anyhow::anyhow!("{}", e))?;
            let (device, device_context) = match d3d11_create_device(Some(&adapter_base)) {
                Ok(device) => device,
                Err(_) => continue,
            };

            let outputs = get_adapter_outputs(&adapter).map_err(|e| anyhow::anyhow!("{}", e))?;
            if outputs.is_empty() {
                continue;
            }

            let output_duplications =
                duplicate_outputs(&device, outputs).map_err(|e| anyhow::anyhow!("{}", e))?;
            if let Some(local_index) =
                take_global_capture_output_index(output_duplications.len(), &mut remaining_index)
            {
                let (output_duplication, output) = output_duplications[local_index].clone();
                self.duplicated_output = Some(DuplicatedOutput {
                    device,
                    device_context,
                    output,
                    output_duplication,
                });
                self.owned_frame = None;
                self.full_readback = None;
                return Ok(());
            }
        }

        Err(anyhow::anyhow!("no suitable output display was found"))
    }

    fn capture_texture_with_metadata(
        &mut self,
    ) -> std::result::Result<(DxgiTextureFrame, FrameMetadata), GpuCaptureError> {
        if self.duplicated_output.is_none() && self.acquire_output_duplication().is_err() {
            return Err(GpuCaptureError::RefreshFailure);
        }

        let duplicated_output = self
            .duplicated_output
            .as_mut()
            .expect("duplication should exist");
        let mut resource: Option<IDXGIResource> = None;
        let mut frame_info: DXGI_OUTDUPL_FRAME_INFO = unsafe { mem::zeroed() };
        let output_desc = duplicated_output
            .get_desc()
            .map_err(|e| GpuCaptureError::Fail(anyhow::anyhow!("{}", e)))?;

        let result = unsafe {
            duplicated_output.output_duplication.AcquireNextFrame(
                self.timeout_ms,
                &mut frame_info,
                &mut resource,
            )
        };
        match result {
            Ok(()) => {}
            Err(e) => {
                let code = e.code();
                if code == DXGI_ERROR_ACCESS_LOST {
                    self.duplicated_output = None;
                    return Err(GpuCaptureError::AccessLost);
                }
                if code == DXGI_ERROR_WAIT_TIMEOUT {
                    return Err(GpuCaptureError::Timeout);
                }
                if code == DXGI_ERROR_ACCESS_DENIED {
                    self.duplicated_output = None;
                    return Err(GpuCaptureError::AccessDenied);
                }
                if code == DXGI_ERROR_MORE_DATA {
                    self.duplicated_output = None;
                    return Err(GpuCaptureError::Fail(anyhow::anyhow!("{}", e)));
                }
                self.duplicated_output = None;
                return Err(GpuCaptureError::Fail(anyhow::anyhow!("{}", e)));
            }
        }

        let output_duplication = duplicated_output.output_duplication.clone();
        let metadata = match duplicated_output.extract_frame_metadata(&frame_info) {
            Ok(metadata) => metadata,
            Err(e) => {
                let _ = unsafe { output_duplication.ReleaseFrame() };
                return Err(GpuCaptureError::Fail(anyhow::anyhow!("{}", e)));
            }
        };
        let acquired_texture: ID3D11Texture2D = match resource
            .ok_or_else(|| GpuCaptureError::Fail(anyhow::anyhow!("DXGI frame resource missing")))
            .and_then(|resource| {
                resource
                    .cast()
                    .map_err(|e| GpuCaptureError::Fail(anyhow::anyhow!("{}", e)))
            }) {
            Ok(texture) => texture,
            Err(err) => {
                let _ = unsafe { output_duplication.ReleaseFrame() };
                return Err(err);
            }
        };
        let desc = texture_desc(&acquired_texture);
        let device = duplicated_output.device.clone();
        let device_context = duplicated_output.device_context.clone();
        let logical_width =
            (output_desc.DesktopCoordinates.right - output_desc.DesktopCoordinates.left) as u32;
        let logical_height =
            (output_desc.DesktopCoordinates.bottom - output_desc.DesktopCoordinates.top) as u32;
        let (width, height) = match output_desc.Rotation {
            DXGI_MODE_ROTATION_ROTATE90 | DXGI_MODE_ROTATION_ROTATE270 => {
                (logical_height, logical_width)
            }
            _ => (logical_width, logical_height),
        };

        if let Err(e) =
            self.ensure_owned_frame_texture(&device, desc.Width, desc.Height, desc.Format)
        {
            let _ = unsafe { output_duplication.ReleaseFrame() };
            return Err(GpuCaptureError::Fail(e));
        }
        let owned_texture = self
            .owned_frame
            .as_ref()
            .expect("owned frame texture should exist")
            .texture
            .clone();

        unsafe {
            device_context.CopyResource(&owned_texture, &acquired_texture);
        }

        let release_result = unsafe { output_duplication.ReleaseFrame() };
        if let Err(e) = release_result {
            self.duplicated_output = None;
            self.owned_frame = None;
            return Err(GpuCaptureError::Fail(anyhow::anyhow!("{}", e)));
        }

        let frame = DxgiTextureFrame {
            texture: owned_texture,
            width,
            height,
            stride: width.saturating_mul(4),
            raw_width: desc.Width,
            raw_height: desc.Height,
            rotation: output_desc.Rotation,
        };

        Ok((frame, metadata))
    }

    fn ensure_owned_frame_texture(
        &mut self,
        device: &ID3D11Device,
        width: u32,
        height: u32,
        format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    ) -> std::result::Result<(), anyhow::Error> {
        ensure_device_texture(device, &mut self.owned_frame, width, height, format)
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    fn ensure_full_readback(
        &mut self,
        width: u32,
        height: u32,
        format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    ) -> Result<()> {
        let device = self
            .duplicated_output
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("output duplication unavailable"))?
            .device
            .clone();
        ensure_staging_readback(&device, &mut self.full_readback, width, height, format)
            .map_err(|e| anyhow::anyhow!("{}", e))
    }
}

impl DuplicatedOutput {
    fn get_desc(&self) -> WindowsResult<DXGI_OUTPUT_DESC> {
        unsafe { self.output.GetDesc() }
    }

    fn extract_frame_metadata(
        &self,
        frame_info: &DXGI_OUTDUPL_FRAME_INFO,
    ) -> WindowsResult<FrameMetadata> {
        let mut dirty_rects = Vec::new();
        let mut move_rects = Vec::new();

        if frame_info.TotalMetadataBufferSize > 0 {
            let total_metadata_size = frame_info.TotalMetadataBufferSize as usize;
            let move_rect_stride = mem::size_of::<DXGI_OUTDUPL_MOVE_RECT>();
            let move_rect_capacity = total_metadata_size.div_ceil(move_rect_stride).max(1);
            let mut move_rects_buffer: Vec<DXGI_OUTDUPL_MOVE_RECT> =
                vec![unsafe { mem::zeroed() }; move_rect_capacity];
            let mut move_rects_buffer_size = 0u32;
            unsafe {
                self.output_duplication.GetFrameMoveRects(
                    frame_info.TotalMetadataBufferSize,
                    move_rects_buffer.as_mut_ptr(),
                    &mut move_rects_buffer_size,
                )?;
            }
            let move_rect_bytes = move_rects_buffer_size as usize;
            if move_rect_bytes > total_metadata_size
                || !move_rect_bytes.is_multiple_of(move_rect_stride)
            {
                return Err(DXGI_ERROR_MORE_DATA.into());
            }
            let move_rect_count = move_rect_bytes / move_rect_stride;
            move_rects = move_rects_buffer
                .into_iter()
                .take(move_rect_count)
                .filter_map(|rect| {
                    normalize_move_rect(
                        rect.SourcePoint.x,
                        rect.SourcePoint.y,
                        rect.DestinationRect.left,
                        rect.DestinationRect.top,
                        rect.DestinationRect.right,
                        rect.DestinationRect.bottom,
                    )
                })
                .collect();

            let remaining_metadata_size = total_metadata_size - move_rect_bytes;
            let dirty_rect_stride = mem::size_of::<RECT>();
            let dirty_rect_capacity = remaining_metadata_size.div_ceil(dirty_rect_stride).max(1);
            let mut dirty_rects_buffer = vec![RECT::default(); dirty_rect_capacity];
            let mut dirty_rects_buffer_size = 0u32;
            unsafe {
                self.output_duplication.GetFrameDirtyRects(
                    remaining_metadata_size as u32,
                    dirty_rects_buffer.as_mut_ptr(),
                    &mut dirty_rects_buffer_size,
                )?;
            }
            let dirty_rect_bytes = dirty_rects_buffer_size as usize;
            if dirty_rect_bytes > remaining_metadata_size
                || !dirty_rect_bytes.is_multiple_of(dirty_rect_stride)
            {
                return Err(DXGI_ERROR_MORE_DATA.into());
            }
            let dirty_rect_count = dirty_rect_bytes / dirty_rect_stride;
            dirty_rects = dirty_rects_buffer
                .into_iter()
                .take(dirty_rect_count)
                .filter_map(|rect| {
                    normalize_dirty_rect(rect.left, rect.top, rect.right, rect.bottom)
                })
                .collect();
        }

        Ok(FrameMetadata {
            dirty_rects,
            move_rects,
            accumulated_frames: frame_info.AccumulatedFrames,
            rects_coalesced: frame_info.RectsCoalesced.as_bool(),
        })
    }
}

fn ensure_com_initialized() {
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

fn create_dxgi_factory_1() -> WindowsResult<IDXGIFactory1> {
    unsafe { CreateDXGIFactory1() }
}

fn d3d11_create_device(
    adapter: Option<&IDXGIAdapter>,
) -> WindowsResult<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device: Option<ID3D11Device> = None;
    let mut device_context: Option<ID3D11DeviceContext> = None;
    let feature_levels = [D3D_FEATURE_LEVEL_9_1];
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut device_context),
        )
    }?;
    Ok((
        device.expect("device should exist"),
        device_context.expect("context should exist"),
    ))
}

fn get_adapter_outputs(adapter: &IDXGIAdapter1) -> WindowsResult<Vec<IDXGIOutput>> {
    let mut outputs = Vec::new();
    for i in 0.. {
        match unsafe { adapter.EnumOutputs(i) } {
            Ok(output) => {
                let desc = unsafe { output.GetDesc()? };
                if desc.AttachedToDesktop.as_bool() {
                    outputs.push(output);
                }
            }
            Err(_) => break,
        }
    }
    Ok(outputs)
}

fn duplicate_outputs(
    device: &ID3D11Device,
    outputs: Vec<IDXGIOutput>,
) -> WindowsResult<Vec<(IDXGIOutputDuplication, IDXGIOutput)>> {
    let mut duplicated_outputs = Vec::new();
    for output in outputs {
        let output1: IDXGIOutput1 = output.cast()?;
        let duplicated_output = unsafe { output1.DuplicateOutput(device)? };
        duplicated_outputs.push((duplicated_output, output));
    }
    Ok(duplicated_outputs)
}

fn take_global_capture_output_index(
    output_count: usize,
    remaining_index: &mut usize,
) -> Option<usize> {
    if *remaining_index < output_count {
        Some(*remaining_index)
    } else {
        *remaining_index = (*remaining_index).saturating_sub(output_count);
        None
    }
}

fn ensure_staging_readback(
    device: &ID3D11Device,
    slot: &mut Option<StagingReadback>,
    width: u32,
    height: u32,
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
) -> WindowsResult<()> {
    let needs_recreate = slot
        .as_ref()
        .map(|current| {
            current.width != width || current.height != height || current.format != format
        })
        .unwrap_or(true);
    if !needs_recreate {
        return Ok(());
    }

    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut texture: Option<ID3D11Texture2D> = None;
    unsafe {
        device.CreateTexture2D(&desc, None, Some(&mut texture))?;
    }
    let texture = texture.expect("staging texture should exist");
    let surface: IDXGISurface1 = texture.cast()?;
    *slot = Some(StagingReadback {
        texture,
        surface,
        width,
        height,
        format,
    });
    Ok(())
}

fn ensure_device_texture(
    device: &ID3D11Device,
    slot: &mut Option<DeviceTexture>,
    width: u32,
    height: u32,
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
) -> WindowsResult<()> {
    let needs_recreate = slot
        .as_ref()
        .map(|current| {
            current.width != width || current.height != height || current.format != format
        })
        .unwrap_or(true);
    if !needs_recreate {
        return Ok(());
    }

    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: 0,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture: Option<ID3D11Texture2D> = None;
    unsafe {
        device.CreateTexture2D(&desc, None, Some(&mut texture))?;
    }
    *slot = Some(DeviceTexture {
        texture: texture.expect("device texture should exist"),
        width,
        height,
        format,
    });
    Ok(())
}

fn copy_surface_to_bgra(
    surface: &IDXGISurface1,
    width: usize,
    height: usize,
    rotation: DXGI_MODE_ROTATION,
) -> Result<Vec<u8>> {
    let mut mapped = DXGI_MAPPED_RECT::default();
    unsafe { surface.Map(&mut mapped, DXGI_MAP_READ)? };

    let result = {
        let pitch = mapped.Pitch as usize;
        let source_slice =
            unsafe { slice::from_raw_parts(mapped.pBits as *const u8, pitch * height) };
        let (rotated_width, rotated_height) = match rotation {
            DXGI_MODE_ROTATION_ROTATE90 | DXGI_MODE_ROTATION_ROTATE270 => (height, width),
            _ => (width, height),
        };
        let mut data = Vec::with_capacity(rotated_width * rotated_height * 4);
        match rotation {
            DXGI_MODE_ROTATION_IDENTITY | DXGI_MODE_ROTATION_UNSPECIFIED => {
                let row_bytes = width * 4;
                for row in 0..height {
                    let start = row * pitch;
                    let end = start + row_bytes;
                    data.extend_from_slice(&source_slice[start..end]);
                }
            }
            DXGI_MODE_ROTATION_ROTATE90 => {
                for x in 0..width {
                    for y in (0..height).rev() {
                        let index = y * pitch + x * 4;
                        data.extend_from_slice(&source_slice[index..index + 4]);
                    }
                }
            }
            DXGI_MODE_ROTATION_ROTATE180 => {
                for y in (0..height).rev() {
                    for x in (0..width).rev() {
                        let index = y * pitch + x * 4;
                        data.extend_from_slice(&source_slice[index..index + 4]);
                    }
                }
            }
            DXGI_MODE_ROTATION_ROTATE270 => {
                for x in (0..width).rev() {
                    for y in 0..height {
                        let index = y * pitch + x * 4;
                        data.extend_from_slice(&source_slice[index..index + 4]);
                    }
                }
            }
            _ => {}
        }
        Ok::<Vec<u8>, anyhow::Error>(data)
    };

    let _ = unsafe { surface.Unmap() };
    result
}

fn texture_desc(texture: &ID3D11Texture2D) -> D3D11_TEXTURE2D_DESC {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        texture.GetDesc(&mut desc);
    }
    desc
}

fn normalize_dirty_rect(left: i32, top: i32, right: i32, bottom: i32) -> Option<DirtyRect> {
    let left = left.max(0) as u32;
    let top = top.max(0) as u32;
    let right = right.max(0) as u32;
    let bottom = bottom.max(0) as u32;
    if right <= left || bottom <= top {
        return None;
    }
    Some(DirtyRect {
        left,
        top,
        right,
        bottom,
    })
}

fn normalize_move_rect(
    source_x: i32,
    source_y: i32,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) -> Option<MoveRect> {
    let source_x = source_x.max(0) as u32;
    let source_y = source_y.max(0) as u32;
    let left = left.max(0) as u32;
    let top = top.max(0) as u32;
    let right = right.max(0) as u32;
    let bottom = bottom.max(0) as u32;
    if right <= left || bottom <= top {
        return None;
    }
    Some(MoveRect {
        source_x,
        source_y,
        left,
        top,
        right,
        bottom,
    })
}

/// DXGI capture index matches `GpuDxgiBackend`: global index G selects the G-th attached desktop
/// output across adapters (in `EnumAdapters1` order), considering only adapters where D3D11
/// device creation succeeds.
#[derive(Clone, Debug)]
pub(crate) struct DxgiCaptureOutputInfo {
    pub index: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
}

fn output_device_name(output: &IDXGIOutput) -> WindowsResult<String> {
    let desc = unsafe { output.GetDesc()? };
    let end = desc
        .DeviceName
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(desc.DeviceName.len());
    Ok(String::from_utf16_lossy(&desc.DeviceName[..end]))
}

/// Desktop rectangle (`left`, `top`, `right`, `bottom`) for a global DXGI output
/// index, in virtual-screen coordinates. Matches the same adapter/output selection
/// logic as capture and `enumerate_dxgi_capture_outputs`.
pub fn dxgi_output_desktop_rect_for_global_index(
    global_index: usize,
) -> Option<(i32, i32, i32, i32)> {
    ensure_com_initialized();
    if let Ok(factory) = create_dxgi_factory_1() {
        let mut remaining_index = global_index;
        for i in 0u32.. {
            let adapter = match unsafe { factory.EnumAdapters1(i) } {
                Ok(adapter) => adapter,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(_) => break,
            };
            let Ok(adapter_base) = adapter.cast::<IDXGIAdapter>() else {
                continue;
            };
            if d3d11_create_device(Some(&adapter_base)).is_err() {
                continue;
            }
            let Ok(outputs) = get_adapter_outputs(&adapter) else {
                continue;
            };
            if outputs.is_empty() {
                continue;
            }
            if let Some(local_index) =
                take_global_capture_output_index(outputs.len(), &mut remaining_index)
            {
                let desc = unsafe { outputs[local_index].GetDesc() }.ok()?;
                let r = desc.DesktopCoordinates;
                return Some((r.left, r.top, r.right, r.bottom));
            }
        }
    }
    crate::monitor::enumerate_gdi_monitors()
        .get(global_index)
        .map(|monitor| (monitor.left, monitor.top, monitor.right, monitor.bottom))
}

pub(crate) fn enumerate_dxgi_capture_outputs() -> Vec<DxgiCaptureOutputInfo> {
    ensure_com_initialized();
    let primary_raw = crate::monitor::primary_gdi_display_device_name();
    let mut out = Vec::new();
    if let Ok(factory) = create_dxgi_factory_1() {
        for i in 0u32.. {
            let adapter = match unsafe { factory.EnumAdapters1(i) } {
                Ok(adapter) => adapter,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(_) => break,
            };
            let Ok(adapter_base) = adapter.cast::<IDXGIAdapter>() else {
                continue;
            };
            if d3d11_create_device(Some(&adapter_base)).is_err() {
                continue;
            }
            let Ok(outputs) = get_adapter_outputs(&adapter) else {
                continue;
            };
            for output in outputs {
                let Ok(raw_name) = output_device_name(&output) else {
                    continue;
                };
                let n = out.len() + 1;
                let is_primary = primary_raw
                    .as_ref()
                    .is_some_and(|primary| primary.eq_ignore_ascii_case(raw_name.trim()));
                let name = if is_primary {
                    format!("Monitor {n} (Primary)")
                } else {
                    format!("Monitor {n}")
                };
                let desc = match unsafe { output.GetDesc() } {
                    Ok(desc) => desc,
                    Err(_) => continue,
                };
                let width =
                    (desc.DesktopCoordinates.right - desc.DesktopCoordinates.left).max(0) as u32;
                let height =
                    (desc.DesktopCoordinates.bottom - desc.DesktopCoordinates.top).max(0) as u32;
                out.push(DxgiCaptureOutputInfo {
                    index: out.len() as u32,
                    name,
                    width,
                    height,
                });
            }
        }
    }
    if out.is_empty() {
        out.extend(
            crate::monitor::enumerate_gdi_monitors()
                .into_iter()
                .enumerate()
                .map(|(index, monitor)| {
                    let n = index + 1;
                    let name = if monitor.is_primary {
                        format!("Monitor {n} (Primary)")
                    } else {
                        format!("Monitor {n}")
                    };
                    DxgiCaptureOutputInfo {
                        index: index as u32,
                        name,
                        width: (monitor.right - monitor.left).max(0) as u32,
                        height: (monitor.bottom - monitor.top).max(0) as u32,
                    }
                }),
        );
    }
    out
}

#[cfg(all(test, windows))]
mod tests {
    use super::take_global_capture_output_index;

    #[test]
    fn take_global_capture_output_index_flattens_across_adapters() {
        let mut remaining_index = 1usize;
        assert_eq!(
            take_global_capture_output_index(1, &mut remaining_index),
            None
        );
        assert_eq!(remaining_index, 0);
        assert_eq!(
            take_global_capture_output_index(2, &mut remaining_index),
            Some(0)
        );
    }
}
