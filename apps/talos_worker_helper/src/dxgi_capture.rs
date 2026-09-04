#![cfg(target_os = "windows")]

use std::{mem, time::Duration, time::Instant};

use anyhow::{Context, Result};
use windows::{
    core::{Interface, Result as WindowsResult},
    Win32::{
        Foundation::{HMODULE, RECT},
        Graphics::{
            Direct3D::{
                D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
                D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_9_1, D3D_FEATURE_LEVEL_9_2,
                D3D_FEATURE_LEVEL_9_3,
            },
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
            },
            Dxgi::{
                Common::{
                    DXGI_FORMAT, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED,
                },
                CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput,
                IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_DENIED,
                DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_MORE_DATA, DXGI_ERROR_NOT_FOUND,
                DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_MOVE_RECT,
                DXGI_OUTPUT_DESC,
            },
        },
        System::Com::{CoInitializeEx, COINIT_MULTITHREADED},
    },
};

const DEFAULT_TIMEOUT_MS: u32 = 100;

#[derive(Clone, Copy, Debug)]
pub(crate) struct DirtyRect {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MoveRect {
    pub source_x: u32,
    pub source_y: u32,
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FrameMetadata {
    pub dirty_rects: Vec<DirtyRect>,
    pub move_rects: Vec<MoveRect>,
    pub accumulated_frames: u32,
    pub rects_coalesced: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CaptureTimings {
    pub acquire_wait: Duration,
}

pub(crate) enum CaptureEvent {
    Frame(CapturedFrame),
    Timeout,
    AccessLost,
}

pub(crate) struct CapturedFrame {
    pub texture: ID3D11Texture2D,
    pub metadata: FrameMetadata,
    pub timings: CaptureTimings,
    pub width: u32,
    pub height: u32,
    pub format: DXGI_FORMAT,
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    output_duplication: IDXGIOutputDuplication,
}

impl Drop for CapturedFrame {
    fn drop(&mut self) {
        let _ = unsafe { self.output_duplication.ReleaseFrame() };
    }
}

pub(crate) struct DxgiAtlasCapturer {
    factory: IDXGIFactory1,
    duplicated_output: Option<DuplicatedOutput>,
    capture_source_index: usize,
    timeout_ms: u32,
}

struct DuplicatedOutput {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput,
    output_duplication: IDXGIOutputDuplication,
}

impl DxgiAtlasCapturer {
    pub(crate) fn new(capture_source_index: usize) -> Result<Self> {
        ensure_com_initialized();
        let factory = unsafe { CreateDXGIFactory1() }.context("CreateDXGIFactory1 failed")?;
        let mut capturer = Self {
            factory,
            duplicated_output: None,
            capture_source_index,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        };
        capturer.acquire_output_duplication()?;
        Ok(capturer)
    }

    pub(crate) fn capture_next(&mut self) -> Result<CaptureEvent> {
        let capture_started = Instant::now();
        if self.duplicated_output.is_none() {
            self.acquire_output_duplication()?;
        }
        let duplicated_output = self
            .duplicated_output
            .as_ref()
            .context("output duplication unavailable")?;
        let output_desc = duplicated_output
            .get_desc()
            .context("DXGI output GetDesc failed")?;
        let mut resource: Option<IDXGIResource> = None;
        let mut frame_info: DXGI_OUTDUPL_FRAME_INFO = unsafe { mem::zeroed() };
        let acquire_started = Instant::now();
        match unsafe {
            duplicated_output.output_duplication.AcquireNextFrame(
                self.timeout_ms,
                &mut frame_info,
                &mut resource,
            )
        } {
            Ok(()) => {}
            Err(err) if err.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                return Ok(CaptureEvent::Timeout);
            }
            Err(err) if err.code() == DXGI_ERROR_ACCESS_LOST => {
                self.duplicated_output = None;
                return Ok(CaptureEvent::AccessLost);
            }
            Err(err) if err.code() == DXGI_ERROR_ACCESS_DENIED => {
                self.duplicated_output = None;
                return Err(anyhow::anyhow!("DXGI access denied: {err}"));
            }
            Err(err) => {
                self.duplicated_output = None;
                return Err(anyhow::anyhow!("AcquireNextFrame failed: {err}"));
            }
        }
        let acquire_wait = acquire_started.elapsed();

        let output_duplication = duplicated_output.output_duplication.clone();
        let result =
            self.build_captured_frame(duplicated_output, output_desc, frame_info, resource);
        match result {
            Ok(mut frame) => {
                frame.timings = CaptureTimings {
                    acquire_wait: acquire_wait.min(capture_started.elapsed()),
                };
                Ok(CaptureEvent::Frame(frame))
            }
            Err(err) => {
                let _ = unsafe { output_duplication.ReleaseFrame() };
                Err(err)
            }
        }
    }

    pub(crate) fn desktop_dimensions(&self) -> Result<(u32, u32)> {
        let duplicated_output = self
            .duplicated_output
            .as_ref()
            .context("output duplication unavailable")?;
        let output_desc = duplicated_output
            .get_desc()
            .context("DXGI output GetDesc failed")?;
        let width =
            (output_desc.DesktopCoordinates.right - output_desc.DesktopCoordinates.left) as u32;
        let height =
            (output_desc.DesktopCoordinates.bottom - output_desc.DesktopCoordinates.top) as u32;
        anyhow::ensure!(width > 0 && height > 0, "DXGI output has zero dimensions");
        Ok((width, height))
    }

    fn build_captured_frame(
        &self,
        duplicated_output: &DuplicatedOutput,
        output_desc: DXGI_OUTPUT_DESC,
        frame_info: DXGI_OUTDUPL_FRAME_INFO,
        resource: Option<IDXGIResource>,
    ) -> Result<CapturedFrame> {
        if !matches!(
            output_desc.Rotation,
            DXGI_MODE_ROTATION_IDENTITY | DXGI_MODE_ROTATION_UNSPECIFIED
        ) {
            anyhow::bail!(
                "rotated DXGI outputs are not supported for atlas dump V1: {:?}",
                output_desc.Rotation
            );
        }
        let metadata = duplicated_output
            .extract_frame_metadata(&frame_info)
            .context("extract DXGI frame metadata")?;
        let texture: ID3D11Texture2D = resource
            .context("DXGI frame resource missing")?
            .cast()
            .context("cast DXGI resource to ID3D11Texture2D")?;
        let desc = texture_desc(&texture);
        let width =
            (output_desc.DesktopCoordinates.right - output_desc.DesktopCoordinates.left) as u32;
        let height =
            (output_desc.DesktopCoordinates.bottom - output_desc.DesktopCoordinates.top) as u32;
        anyhow::ensure!(width > 0 && height > 0, "DXGI output has zero dimensions");
        Ok(CapturedFrame {
            texture,
            metadata,
            timings: CaptureTimings::default(),
            width,
            height,
            format: desc.Format,
            device: duplicated_output.device.clone(),
            context: duplicated_output.context.clone(),
            output_duplication: duplicated_output.output_duplication.clone(),
        })
    }

    fn acquire_output_duplication(&mut self) -> Result<()> {
        self.duplicated_output = None;
        let mut remaining_index = self.capture_source_index;

        for adapter_index in 0u32.. {
            let adapter = match unsafe { self.factory.EnumAdapters1(adapter_index) } {
                Ok(adapter) => adapter,
                Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(err) => return Err(anyhow::anyhow!("EnumAdapters1 failed: {err}")),
            };
            let adapter_base: IDXGIAdapter = adapter.cast().context("cast adapter")?;
            let (device, context) = match d3d11_create_device(Some(&adapter_base)) {
                Ok(pair) => pair,
                Err(_) => continue,
            };
            let outputs = get_adapter_outputs(&adapter).context("enumerate adapter outputs")?;
            if outputs.is_empty() {
                continue;
            }
            let Some(local_index) =
                take_global_capture_output_index(outputs.len(), &mut remaining_index)
            else {
                continue;
            };
            let output = outputs[local_index].clone();
            let output1: IDXGIOutput1 = output.cast().context("cast IDXGIOutput1")?;
            let output_duplication =
                unsafe { output1.DuplicateOutput(&device) }.context("DuplicateOutput failed")?;
            self.duplicated_output = Some(DuplicatedOutput {
                device,
                context,
                output,
                output_duplication,
            });
            return Ok(());
        }

        anyhow::bail!(
            "no suitable DXGI output was found for index {}",
            self.capture_source_index
        )
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

fn d3d11_create_device(
    adapter: Option<&IDXGIAdapter>,
) -> WindowsResult<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    let feature_levels = [
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
        D3D_FEATURE_LEVEL_9_3,
        D3D_FEATURE_LEVEL_9_2,
        D3D_FEATURE_LEVEL_9_1,
    ];
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
            Some(&mut context),
        )
    }?;
    Ok((
        device.expect("D3D11 device should exist"),
        context.expect("D3D11 context should exist"),
    ))
}

fn get_adapter_outputs(adapter: &IDXGIAdapter1) -> WindowsResult<Vec<IDXGIOutput>> {
    let mut outputs = Vec::new();
    for output_index in 0u32.. {
        match unsafe { adapter.EnumOutputs(output_index) } {
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

fn take_global_capture_output_index(
    output_count: usize,
    remaining_index: &mut usize,
) -> Option<usize> {
    if *remaining_index < output_count {
        Some(*remaining_index)
    } else {
        *remaining_index = remaining_index.saturating_sub(output_count);
        None
    }
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

pub(crate) fn texture_desc(texture: &ID3D11Texture2D) -> D3D11_TEXTURE2D_DESC {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        texture.GetDesc(&mut desc);
    }
    desc
}
