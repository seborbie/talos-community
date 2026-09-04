use std::{mem, slice, time::Duration};

use tracing::debug;
use windows::{
    core::{s, PCSTR},
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::{
                Fxc::D3DCompile, ID3DBlob, ID3DInclude, D3D11_SRV_DIMENSION_BUFFEREX,
                D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP,
                D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP,
            },
            Direct3D11::{
                D3D11CreateDeviceAndSwapChain, ID3D11Buffer, ID3D11ComputeShader, ID3D11Device,
                ID3D11DeviceContext, ID3D11PixelShader, ID3D11RasterizerState,
                ID3D11RenderTargetView, ID3D11SamplerState, ID3D11ShaderResourceView,
                ID3D11Texture2D, ID3D11UnorderedAccessView, ID3D11VertexShader,
                D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE,
                D3D11_BIND_UNORDERED_ACCESS, D3D11_BOX, D3D11_BUFFEREX_SRV,
                D3D11_BUFFEREX_SRV_FLAG_RAW, D3D11_BUFFER_DESC, D3D11_COMPARISON_NEVER,
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CULL_NONE,
                D3D11_FILL_SOLID, D3D11_FILTER_MIN_MAG_MIP_POINT, D3D11_FLOAT32_MAX,
                D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_RASTERIZER_DESC,
                D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS, D3D11_SAMPLER_DESC, D3D11_SDK_VERSION,
                D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC_0,
                D3D11_TEXTURE2D_DESC, D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_USAGE_DEFAULT,
                D3D11_USAGE_STAGING, D3D11_VIEWPORT,
            },
            Dxgi::{
                Common::{
                    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R32_TYPELESS, DXGI_FORMAT_R32_UINT,
                    DXGI_FORMAT_UNKNOWN, DXGI_MODE_DESC, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
                },
                IDXGIAdapter, IDXGISwapChain, DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC,
                DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
            },
        },
    },
};
use windows_sys::Win32::Foundation::HWND as RawHwnd;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ExperimentalMoveRect {
    pub(crate) src_x: u32,
    pub(crate) src_y: u32,
    pub(crate) dst_x: u32,
    pub(crate) dst_y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ExperimentalCompositeStats {
    pub(crate) validate: Duration,
    pub(crate) upload: Duration,
    pub(crate) decode: Duration,
    pub(crate) moves: Duration,
    pub(crate) dirty: Duration,
    pub(crate) command_count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DesktopSourceKind {
    Bgra,
    ExperimentalUint,
}

#[derive(Debug)]
struct TileCommandStreamInfo {
    atlas_width: u32,
    atlas_height: u32,
    command_count: u32,
    byte_len: u32,
    max_pixels_per_command: u32,
    requires_previous: bool,
    command_offsets: Vec<u32>,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ExperimentalDecodeParams {
    desktop_width: u32,
    desktop_height: u32,
    atlas_width: u32,
    atlas_height: u32,
    stream_byte_len: u32,
    command_count: u32,
    max_pixels_per_command: u32,
    _pad0: u32,
}

const TILE_COMMAND_STREAM_MAGIC: u32 = 0x3258_5441;
const TILE_COMMAND_STREAM_VERSION: u32 = 4;
const TILE_COMMAND_STREAM_HEADER_BYTES: usize = 32;
const TILE_COMMAND_HEADER_BYTES: usize = 24;
const TILE_SIZE: u32 = 32;
const TILE_COMMAND_RAW_BGRA: u32 = 1;
const TILE_COMMAND_SOLID_COLOR: u32 = 2;
const TILE_COMMAND_XOR_RAW: u32 = 3;
const TILE_COMMAND_XOR_SPARSE: u32 = 4;
const TILE_COMMAND_MASKED_QUANT_DELTA: u32 = 5;
const TILE_COMMAND_LOSSY_UI_BLOCK: u32 = 6;
const TILE_COMMAND_SHARP_UI_BLOCK: u32 = 7;

pub(crate) struct D3d11Viewport {
    swap_chain: IDXGISwapChain,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    driver_label: &'static str,
    render_target_view: Option<ID3D11RenderTargetView>,
    desktop_texture: Option<ID3D11Texture2D>,
    desktop_bgra_srv: Option<ID3D11ShaderResourceView>,
    experimental_desktop_texture: Option<ID3D11Texture2D>,
    experimental_desktop_srv: Option<ID3D11ShaderResourceView>,
    experimental_previous_texture: Option<ID3D11Texture2D>,
    has_experimental_desktop_frame: bool,
    experimental_atlas_texture: Option<ID3D11Texture2D>,
    experimental_atlas_uav: Option<ID3D11UnorderedAccessView>,
    experimental_command_buffer: Option<ID3D11Buffer>,
    experimental_command_srv: Option<ID3D11ShaderResourceView>,
    experimental_command_capacity: u32,
    experimental_command_offsets_buffer: Option<ID3D11Buffer>,
    experimental_command_offsets_srv: Option<ID3D11ShaderResourceView>,
    experimental_command_offsets_capacity: u32,
    vertex_shader: ID3D11VertexShader,
    bgra_pixel_shader: ID3D11PixelShader,
    uint_pixel_shader: ID3D11PixelShader,
    experimental_decode_shader: ID3D11ComputeShader,
    experimental_decode_params: ID3D11Buffer,
    sampler_state: ID3D11SamplerState,
    rasterizer_state: ID3D11RasterizerState,
    active_desktop_source: DesktopSourceKind,
    swap_width: u32,
    swap_height: u32,
    desktop_width: u32,
    desktop_height: u32,
    has_desktop_frame: bool,
}

impl D3d11Viewport {
    pub(crate) fn new(hwnd: RawHwnd) -> Result<Self, String> {
        let (swap_chain, device, context, driver_label) = create_device_and_swap_chain(hwnd)?;
        let (vertex_shader, bgra_pixel_shader, uint_pixel_shader) = create_shaders(&device)?;
        let experimental_decode_shader = create_compute_shader(&device)?;
        let experimental_decode_params = create_experimental_decode_params(&device)?;
        let sampler_state = create_sampler_state(&device)?;
        let rasterizer_state = create_rasterizer_state(&device)?;
        let mut viewport = Self {
            swap_chain,
            device,
            context,
            driver_label,
            render_target_view: None,
            desktop_texture: None,
            desktop_bgra_srv: None,
            experimental_desktop_texture: None,
            experimental_desktop_srv: None,
            experimental_previous_texture: None,
            has_experimental_desktop_frame: false,
            experimental_atlas_texture: None,
            experimental_atlas_uav: None,
            experimental_command_buffer: None,
            experimental_command_srv: None,
            experimental_command_capacity: 0,
            experimental_command_offsets_buffer: None,
            experimental_command_offsets_srv: None,
            experimental_command_offsets_capacity: 0,
            vertex_shader,
            bgra_pixel_shader,
            uint_pixel_shader,
            experimental_decode_shader,
            experimental_decode_params,
            sampler_state,
            rasterizer_state,
            active_desktop_source: DesktopSourceKind::Bgra,
            swap_width: 0,
            swap_height: 0,
            desktop_width: 0,
            desktop_height: 0,
            has_desktop_frame: false,
        };
        viewport.ensure_render_target(1, 1)?;
        Ok(viewport)
    }

    pub(crate) fn driver_label(&self) -> &'static str {
        self.driver_label
    }

    pub(crate) fn has_desktop_frame(&self) -> bool {
        if !self.has_desktop_frame {
            return false;
        }
        match self.active_desktop_source {
            DesktopSourceKind::Bgra => {
                self.desktop_texture.is_some() && self.desktop_bgra_srv.is_some()
            }
            DesktopSourceKind::ExperimentalUint => {
                self.experimental_desktop_texture.is_some()
                    && self.experimental_desktop_srv.is_some()
            }
        }
    }

    pub(crate) fn has_experimental_desktop_frame(&self, width: u32, height: u32) -> bool {
        self.has_experimental_desktop_frame
            && self.experimental_desktop_texture.is_some()
            && self.experimental_desktop_srv.is_some()
            && self.desktop_width == width
            && self.desktop_height == height
    }

    pub(crate) fn read_experimental_desktop_argb(
        &self,
    ) -> Result<Option<(u32, u32, Vec<u32>)>, String> {
        if !self.has_experimental_desktop_frame
            || self.desktop_width == 0
            || self.desktop_height == 0
        {
            return Ok(None);
        }
        let Some(source) = self.experimental_desktop_texture.as_ref() else {
            return Ok(None);
        };

        let width = self.desktop_width;
        let height = self.desktop_height;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R32_UINT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .map_err(|err| {
                    format!("CreateTexture2D(experimental snapshot staging) failed: {err}")
                })?;
            let staging = staging.ok_or_else(|| {
                "CreateTexture2D(experimental snapshot staging) returned null".to_string()
            })?;
            self.context.CopyResource(&staging, source);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|err| format!("experimental desktop snapshot map failed: {err}"))?;
            let result = copy_mapped_u32_texture(&mapped, width, height);
            self.context.Unmap(&staging, 0);
            Ok(Some((width, height, result?)))
        }
    }

    pub(crate) fn clear_desktop_frame(&mut self) {
        self.has_desktop_frame = false;
        self.has_experimental_desktop_frame = false;
    }

    pub(crate) fn upload_full_bgra(
        &mut self,
        width: u32,
        height: u32,
        bgra: &[u8],
    ) -> Result<(), String> {
        debug!(
            width,
            height,
            bytes = bgra.len(),
            driver = self.driver_label,
            "viewer D3D11 compositor upload BGRA start"
        );
        validate_bgra_len(width, height, bgra.len(), "full frame")?;
        self.ensure_bgra_desktop_texture(width, height)?;
        let Some(texture) = self.desktop_texture.as_ref() else {
            return Err("desktop texture unavailable".to_string());
        };
        unsafe {
            self.context.UpdateSubresource(
                texture,
                0,
                None,
                bgra.as_ptr() as *const _,
                width.saturating_mul(4),
                bgra.len() as u32,
            );
        }
        self.active_desktop_source = DesktopSourceKind::Bgra;
        self.has_desktop_frame = true;
        self.has_experimental_desktop_frame = false;
        debug!(
            width,
            height,
            driver = self.driver_label,
            "viewer D3D11 compositor upload BGRA completed"
        );
        Ok(())
    }

    pub(crate) fn upload_full_argb_words(
        &mut self,
        width: u32,
        height: u32,
        argb: &[u32],
    ) -> Result<(), String> {
        let expected = width as usize * height as usize;
        if argb.len() != expected {
            return Err(format!(
                "decoded frame length mismatch: expected {expected} pixels, got {}",
                argb.len()
            ));
        }
        let bgra = unsafe { slice::from_raw_parts(argb.as_ptr() as *const u8, argb.len() * 4) };
        self.upload_full_bgra(width, height, bgra)
    }

    pub(crate) fn upload_full_argb_words_as_experimental(
        &mut self,
        width: u32,
        height: u32,
        argb: &[u32],
    ) -> Result<(), String> {
        let expected = width as usize * height as usize;
        if argb.len() != expected {
            return Err(format!(
                "decoded frame length mismatch: expected {expected} pixels, got {}",
                argb.len()
            ));
        }
        let bgra = unsafe { slice::from_raw_parts(argb.as_ptr() as *const u8, argb.len() * 4) };
        self.upload_full_bgra_as_experimental(width, height, bgra)
    }

    pub(crate) fn present(
        &mut self,
        viewport_width: u32,
        viewport_height: u32,
    ) -> Result<(), String> {
        if viewport_width == 0 || viewport_height == 0 {
            debug!(
                viewport_width,
                viewport_height, "viewer D3D11 compositor present skipped; invalid viewport size"
            );
            return Ok(());
        }
        if !self.has_desktop_frame() {
            debug!(
                viewport_width,
                viewport_height,
                desktop_width = self.desktop_width,
                desktop_height = self.desktop_height,
                has_desktop_frame = self.has_desktop_frame,
                has_texture = self.desktop_texture.is_some(),
                has_srv = self.desktop_bgra_srv.is_some(),
                "viewer D3D11 compositor present skipped; desktop frame missing"
            );
            return Ok(());
        }
        self.ensure_render_target(viewport_width, viewport_height)?;
        let Some(render_target_view) = self.render_target_view.as_ref() else {
            return Err("render target view unavailable".to_string());
        };
        let transform = compute_letterbox_transform(
            self.desktop_width,
            self.desktop_height,
            viewport_width,
            viewport_height,
        );
        let clear = [0.0, 0.0, 0.0, 1.0];
        let viewport = D3D11_VIEWPORT {
            TopLeftX: transform.offset_x as f32,
            TopLeftY: transform.offset_y as f32,
            Width: transform.scaled_w as f32,
            Height: transform.scaled_h as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        let render_targets = [Some(render_target_view.clone())];
        let samplers = [Some(self.sampler_state.clone())];
        let null_shader_resources: [Option<ID3D11ShaderResourceView>; 2] = [None, None];
        let (desktop_srv, pixel_shader, source_label) = match self.active_desktop_source {
            DesktopSourceKind::Bgra => {
                let Some(desktop_bgra_srv) = self.desktop_bgra_srv.as_ref() else {
                    return Err("desktop shader resource unavailable".to_string());
                };
                (desktop_bgra_srv, &self.bgra_pixel_shader, "bgra")
            }
            DesktopSourceKind::ExperimentalUint => {
                let Some(experimental_desktop_srv) = self.experimental_desktop_srv.as_ref() else {
                    return Err("experimental desktop shader resource unavailable".to_string());
                };
                (
                    experimental_desktop_srv,
                    &self.uint_pixel_shader,
                    "experimental_uint",
                )
            }
        };
        let shader_resources = [Some(desktop_srv.clone()), None];
        debug!(
            viewport_width,
            viewport_height,
            desktop_width = self.desktop_width,
            desktop_height = self.desktop_height,
            scaled_w = transform.scaled_w,
            scaled_h = transform.scaled_h,
            offset_x = transform.offset_x,
            offset_y = transform.offset_y,
            source = source_label,
            driver = self.driver_label,
            "viewer D3D11 compositor present start"
        );
        unsafe {
            self.context
                .ClearRenderTargetView(render_target_view, &clear);
            self.context.OMSetRenderTargets(
                Some(&render_targets),
                None::<&windows::Win32::Graphics::Direct3D11::ID3D11DepthStencilView>,
            );
            self.context.RSSetState(&self.rasterizer_state);
            self.context.RSSetViewports(Some(&[viewport]));
            self.context
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            self.context.VSSetShader(&self.vertex_shader, None);
            self.context.PSSetShader(pixel_shader, None);
            self.context
                .PSSetShaderResources(0, Some(&shader_resources));
            self.context.PSSetSamplers(0, Some(&samplers));
            self.context.Draw(4, 0);
            self.context
                .PSSetShaderResources(0, Some(&null_shader_resources));
        }
        unsafe {
            self.swap_chain
                .Present(0, DXGI_PRESENT(0))
                .ok()
                .map_err(|err| format!("swap chain present failed: {err}"))?;
        }
        debug!(
            viewport_width,
            viewport_height,
            source = source_label,
            driver = self.driver_label,
            "viewer D3D11 compositor present completed"
        );
        Ok(())
    }

    pub(crate) fn composite_experimental_atlas_commands(
        &mut self,
        desktop_width: u32,
        desktop_height: u32,
        atlas_width: u32,
        atlas_height: u32,
        rects: &[talos_protocol::DisplayAtlasRect],
        moves: &[ExperimentalMoveRect],
        tile_commands: &[u8],
    ) -> Result<ExperimentalCompositeStats, String> {
        let mut stats = ExperimentalCompositeStats::default();

        let started = std::time::Instant::now();
        validate_experimental_dimensions(desktop_width, desktop_height, "desktop")?;
        validate_experimental_dimensions(atlas_width, atlas_height, "atlas")?;
        for rect in rects {
            validate_rect_bounds(
                rect.dst_x,
                rect.dst_y,
                rect.width,
                rect.height,
                desktop_width,
                desktop_height,
                "desktop dirty rect",
            )?;
            validate_rect_bounds(
                rect.atlas_x,
                rect.atlas_y,
                rect.width,
                rect.height,
                atlas_width,
                atlas_height,
                "atlas dirty rect",
            )?;
        }
        for rect in moves {
            validate_rect_bounds(
                rect.src_x,
                rect.src_y,
                rect.width,
                rect.height,
                desktop_width,
                desktop_height,
                "move source rect",
            )?;
            validate_rect_bounds(
                rect.dst_x,
                rect.dst_y,
                rect.width,
                rect.height,
                desktop_width,
                desktop_height,
                "move destination rect",
            )?;
        }
        let stream = validate_tile_command_stream(
            tile_commands,
            atlas_width,
            atlas_height,
            desktop_width,
            desktop_height,
        )?;
        stats.command_count = stream.command_count;
        stats.validate = started.elapsed();

        let had_previous_experimental_frame = self.has_experimental_desktop_frame
            && self.desktop_width == desktop_width
            && self.desktop_height == desktop_height;
        let has_full_dirty_rect = rects.iter().any(|rect| {
            rect.dst_x == 0
                && rect.dst_y == 0
                && rect.width == desktop_width
                && rect.height == desktop_height
        });

        self.ensure_experimental_desktop_texture(desktop_width, desktop_height)?;
        self.ensure_experimental_atlas_texture(atlas_width, atlas_height)?;
        self.ensure_experimental_command_buffer(stream.byte_len)?;
        self.ensure_experimental_command_offsets_buffer(stream.command_count)?;

        if !moves.is_empty() && !had_previous_experimental_frame && !has_full_dirty_rect {
            return Err("experimental move rect received before initial desktop frame".to_string());
        }
        if stream.requires_previous && !had_previous_experimental_frame {
            return Err(
                "experimental ATX2 delta command received before previous desktop frame"
                    .to_string(),
            );
        }

        let started = std::time::Instant::now();
        self.upload_experimental_command_stream(tile_commands, &stream)?;
        stats.upload = started.elapsed();

        let started = std::time::Instant::now();
        if !moves.is_empty() && had_previous_experimental_frame {
            let Some(desktop) = self.experimental_desktop_texture.as_ref() else {
                return Err("experimental desktop texture unavailable".to_string());
            };
            let Some(previous) = self.experimental_previous_texture.as_ref() else {
                return Err("experimental previous desktop texture unavailable".to_string());
            };
            unsafe {
                self.context.CopyResource(previous, desktop);
            }
            for rect in moves {
                let src_box = D3D11_BOX {
                    left: rect.src_x,
                    top: rect.src_y,
                    front: 0,
                    right: rect.src_x + rect.width,
                    bottom: rect.src_y + rect.height,
                    back: 1,
                };
                unsafe {
                    self.context.CopySubresourceRegion(
                        desktop,
                        0,
                        rect.dst_x,
                        rect.dst_y,
                        0,
                        previous,
                        0,
                        Some(&src_box),
                    );
                }
            }
        } else if !moves.is_empty() {
            debug!(
                move_rects = moves.len(),
                "viewer experimental move rects skipped because full dirty rect initializes desktop"
            );
        }
        stats.moves = started.elapsed();

        let started = std::time::Instant::now();
        if stream.command_count > 0 {
            self.dispatch_experimental_decode(&stream);
        }
        stats.decode = started.elapsed();

        let started = std::time::Instant::now();
        if !rects.is_empty() {
            let Some(desktop) = self.experimental_desktop_texture.as_ref() else {
                return Err("experimental desktop texture unavailable".to_string());
            };
            let Some(atlas) = self.experimental_atlas_texture.as_ref() else {
                return Err("experimental atlas texture unavailable".to_string());
            };
            for rect in rects {
                let src_box = D3D11_BOX {
                    left: rect.atlas_x,
                    top: rect.atlas_y,
                    front: 0,
                    right: rect.atlas_x + rect.width,
                    bottom: rect.atlas_y + rect.height,
                    back: 1,
                };
                unsafe {
                    self.context.CopySubresourceRegion(
                        desktop,
                        0,
                        rect.dst_x,
                        rect.dst_y,
                        0,
                        atlas,
                        0,
                        Some(&src_box),
                    );
                }
            }
        }
        stats.dirty = started.elapsed();

        self.desktop_width = desktop_width;
        self.desktop_height = desktop_height;
        self.active_desktop_source = DesktopSourceKind::ExperimentalUint;
        self.has_desktop_frame = true;
        self.has_experimental_desktop_frame = true;
        Ok(stats)
    }

    fn upload_full_bgra_as_experimental(
        &mut self,
        width: u32,
        height: u32,
        bgra: &[u8],
    ) -> Result<(), String> {
        debug!(
            width,
            height,
            bytes = bgra.len(),
            driver = self.driver_label,
            "viewer D3D11 compositor upload experimental desktop start"
        );
        validate_bgra_len(width, height, bgra.len(), "full experimental frame")?;
        self.ensure_experimental_desktop_texture(width, height)?;
        let Some(texture) = self.experimental_desktop_texture.as_ref() else {
            return Err("experimental desktop texture unavailable".to_string());
        };
        unsafe {
            self.context.UpdateSubresource(
                texture,
                0,
                None,
                bgra.as_ptr() as *const _,
                width.saturating_mul(4),
                bgra.len() as u32,
            );
        }
        self.desktop_width = width;
        self.desktop_height = height;
        self.active_desktop_source = DesktopSourceKind::ExperimentalUint;
        self.has_desktop_frame = true;
        self.has_experimental_desktop_frame = true;
        debug!(
            width,
            height,
            driver = self.driver_label,
            "viewer D3D11 compositor upload experimental desktop completed"
        );
        Ok(())
    }

    fn ensure_render_target(&mut self, width: u32, height: u32) -> Result<(), String> {
        let width = width.max(1);
        let height = height.max(1);
        if self.render_target_view.is_some()
            && self.swap_width == width
            && self.swap_height == height
        {
            return Ok(());
        }
        self.render_target_view = None;
        unsafe {
            self.swap_chain
                .ResizeBuffers(
                    0,
                    width,
                    height,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
                .map_err(|err| format!("swap chain resize failed: {err}"))?;
        }
        let back_buffer: ID3D11Texture2D = unsafe {
            self.swap_chain
                .GetBuffer(0)
                .map_err(|err| format!("swap chain back buffer failed: {err}"))?
        };
        let mut render_target_view = None;
        unsafe {
            self.device
                .CreateRenderTargetView(&back_buffer, None, Some(&mut render_target_view))
                .map_err(|err| format!("render target view creation failed: {err}"))?;
        }
        self.render_target_view = render_target_view;
        self.swap_width = width;
        self.swap_height = height;
        debug!(
            width,
            height,
            driver = self.driver_label,
            "viewer D3D11 compositor render target ready"
        );
        Ok(())
    }

    fn ensure_bgra_desktop_texture(&mut self, width: u32, height: u32) -> Result<(), String> {
        if self.desktop_texture.is_some()
            && self.desktop_bgra_srv.is_some()
            && self.desktop_width == width
            && self.desktop_height == height
        {
            return Ok(());
        }
        let desktop_texture = create_texture(
            &self.device,
            width,
            height,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            D3D11_BIND_SHADER_RESOURCE.0 as u32,
        )?;
        let desktop_bgra_srv = create_bgra_shader_resource_view(&self.device, &desktop_texture)?;
        self.desktop_texture = Some(desktop_texture);
        self.desktop_bgra_srv = Some(desktop_bgra_srv);
        self.desktop_width = width;
        self.desktop_height = height;
        self.has_desktop_frame = false;
        self.has_experimental_desktop_frame = false;
        debug!(
            width,
            height, "viewer GPU viewport prepared BGRA desktop texture"
        );
        Ok(())
    }

    fn ensure_experimental_desktop_texture(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        if self.experimental_desktop_texture.is_some()
            && self.experimental_desktop_srv.is_some()
            && self.experimental_previous_texture.is_some()
            && self.desktop_width == width
            && self.desktop_height == height
        {
            return Ok(());
        }

        let desktop_texture = create_texture(
            &self.device,
            width,
            height,
            DXGI_FORMAT_R32_UINT,
            D3D11_BIND_SHADER_RESOURCE.0 as u32,
        )?;
        let desktop_srv = create_shader_resource_view(&self.device, &desktop_texture)?;
        let previous_texture =
            create_texture(&self.device, width, height, DXGI_FORMAT_R32_UINT, 0)?;
        self.experimental_desktop_texture = Some(desktop_texture);
        self.experimental_desktop_srv = Some(desktop_srv);
        self.experimental_previous_texture = Some(previous_texture);
        self.desktop_width = width;
        self.desktop_height = height;
        self.has_experimental_desktop_frame = false;
        if self.active_desktop_source == DesktopSourceKind::ExperimentalUint {
            self.has_desktop_frame = false;
        }
        debug!(
            width,
            height, "viewer GPU viewport prepared experimental desktop texture"
        );
        Ok(())
    }

    fn ensure_experimental_atlas_texture(&mut self, width: u32, height: u32) -> Result<(), String> {
        let recreate = match self.experimental_atlas_texture.as_ref() {
            Some(texture) => {
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                unsafe {
                    texture.GetDesc(&mut desc);
                }
                desc.Width != width || desc.Height != height
            }
            None => true,
        };
        if !recreate && self.experimental_atlas_uav.is_some() {
            return Ok(());
        }
        let atlas_texture = create_texture(
            &self.device,
            width,
            height,
            DXGI_FORMAT_R32_UINT,
            D3D11_BIND_UNORDERED_ACCESS.0 as u32,
        )?;
        let atlas_uav = create_unordered_access_view(&self.device, &atlas_texture)?;
        self.experimental_atlas_texture = Some(atlas_texture);
        self.experimental_atlas_uav = Some(atlas_uav);
        debug!(
            width,
            height, "viewer GPU viewport prepared experimental atlas texture"
        );
        Ok(())
    }

    fn ensure_experimental_command_buffer(&mut self, byte_len: u32) -> Result<(), String> {
        if self.experimental_command_buffer.is_some()
            && self.experimental_command_srv.is_some()
            && self.experimental_command_capacity >= byte_len
        {
            return Ok(());
        }
        let requested = byte_len.max(4);
        let capacity = requested.checked_next_power_of_two().unwrap_or(requested);
        let command_buffer = create_raw_shader_resource_buffer(&self.device, capacity)?;
        let command_srv = create_raw_shader_resource_view(&self.device, &command_buffer, capacity)?;
        self.experimental_command_buffer = Some(command_buffer);
        self.experimental_command_srv = Some(command_srv);
        self.experimental_command_capacity = capacity;
        debug!(
            capacity,
            "viewer GPU viewport prepared experimental command buffer"
        );
        Ok(())
    }

    fn ensure_experimental_command_offsets_buffer(
        &mut self,
        command_count: u32,
    ) -> Result<(), String> {
        let byte_len = command_count.saturating_mul(4).max(4);
        if self.experimental_command_offsets_buffer.is_some()
            && self.experimental_command_offsets_srv.is_some()
            && self.experimental_command_offsets_capacity >= byte_len
        {
            return Ok(());
        }
        let capacity = byte_len.checked_next_power_of_two().unwrap_or(byte_len);
        let offsets_buffer = create_raw_shader_resource_buffer(&self.device, capacity)?;
        let offsets_srv = create_raw_shader_resource_view(&self.device, &offsets_buffer, capacity)?;
        self.experimental_command_offsets_buffer = Some(offsets_buffer);
        self.experimental_command_offsets_srv = Some(offsets_srv);
        self.experimental_command_offsets_capacity = capacity;
        debug!(
            capacity,
            "viewer GPU viewport prepared experimental command offsets buffer"
        );
        Ok(())
    }

    fn upload_experimental_command_stream(
        &self,
        tile_commands: &[u8],
        stream: &TileCommandStreamInfo,
    ) -> Result<(), String> {
        let Some(command_buffer) = self.experimental_command_buffer.as_ref() else {
            return Err("experimental command buffer unavailable".to_string());
        };
        let Some(offsets_buffer) = self.experimental_command_offsets_buffer.as_ref() else {
            return Err("experimental command offsets buffer unavailable".to_string());
        };
        let params = ExperimentalDecodeParams {
            desktop_width: self.desktop_width,
            desktop_height: self.desktop_height,
            atlas_width: stream.atlas_width,
            atlas_height: stream.atlas_height,
            stream_byte_len: stream.byte_len,
            command_count: stream.command_count,
            max_pixels_per_command: stream.max_pixels_per_command,
            _pad0: 0,
        };
        let command_box = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: stream.byte_len,
            bottom: 1,
            back: 1,
        };
        unsafe {
            self.context.UpdateSubresource(
                command_buffer,
                0,
                Some(&command_box),
                tile_commands.as_ptr() as *const _,
                0,
                0,
            );
            if !stream.command_offsets.is_empty() {
                let offset_box = D3D11_BOX {
                    left: 0,
                    top: 0,
                    front: 0,
                    right: stream.command_count.saturating_mul(4),
                    bottom: 1,
                    back: 1,
                };
                self.context.UpdateSubresource(
                    offsets_buffer,
                    0,
                    Some(&offset_box),
                    stream.command_offsets.as_ptr() as *const _,
                    0,
                    0,
                );
            }
            self.context.UpdateSubresource(
                &self.experimental_decode_params,
                0,
                None,
                &params as *const _ as *const _,
                0,
                0,
            );
        }
        Ok(())
    }

    fn dispatch_experimental_decode(&self, stream: &TileCommandStreamInfo) {
        let command_srv = self
            .experimental_command_srv
            .as_ref()
            .expect("experimental command SRV exists");
        let offsets_srv = self
            .experimental_command_offsets_srv
            .as_ref()
            .expect("experimental command offsets SRV exists");
        let previous_srv = self
            .experimental_desktop_srv
            .as_ref()
            .expect("experimental desktop SRV exists");
        let atlas_uav = self
            .experimental_atlas_uav
            .as_ref()
            .expect("experimental atlas UAV exists");
        unsafe {
            let srvs = [
                Some(command_srv.clone()),
                Some(offsets_srv.clone()),
                Some(previous_srv.clone()),
            ];
            let uavs = [Some(atlas_uav.clone())];
            let initial_counts = [0u32];
            self.context
                .CSSetShader(&self.experimental_decode_shader, None);
            self.context
                .CSSetConstantBuffers(0, Some(&[Some(self.experimental_decode_params.clone())]));
            self.context.CSSetShaderResources(0, Some(&srvs));
            self.context.CSSetUnorderedAccessViews(
                0,
                1,
                Some(uavs.as_ptr()),
                Some(initial_counts.as_ptr()),
            );
            self.context.Dispatch(
                stream.max_pixels_per_command.div_ceil(64).max(1),
                stream.command_count,
                1,
            );
            let null_srvs = [None, None, None];
            let null_uavs = [None];
            let null_cbuffers = [None];
            self.context.CSSetShaderResources(0, Some(&null_srvs));
            self.context
                .CSSetUnorderedAccessViews(0, 1, Some(null_uavs.as_ptr()), None);
            self.context.CSSetConstantBuffers(0, Some(&null_cbuffers));
            self.context.CSSetShader(None, None);
        }
    }
}

fn create_device_and_swap_chain(
    hwnd: RawHwnd,
) -> Result<
    (
        IDXGISwapChain,
        ID3D11Device,
        ID3D11DeviceContext,
        &'static str,
    ),
    String,
> {
    let swap_chain_desc = DXGI_SWAP_CHAIN_DESC {
        BufferDesc: DXGI_MODE_DESC {
            Width: 1,
            Height: 1,
            RefreshRate: DXGI_RATIONAL {
                Numerator: 0,
                Denominator: 1,
            },
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            ..Default::default()
        },
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        OutputWindow: windows::Win32::Foundation::HWND(hwnd as _),
        Windowed: true.into(),
        SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
        Flags: 0,
    };

    let try_create = |driver_type,
                      driver_label|
     -> Result<
        (
            IDXGISwapChain,
            ID3D11Device,
            ID3D11DeviceContext,
            &'static str,
        ),
        String,
    > {
        let mut swap_chain = None;
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDeviceAndSwapChain(
                None::<&IDXGIAdapter>,
                driver_type,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&swap_chain_desc as *const _),
                Some(&mut swap_chain),
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|err| format!("D3D11CreateDeviceAndSwapChain failed: {err}"))?;
        }
        let swap_chain =
            swap_chain.ok_or_else(|| "swap chain creation returned null".to_string())?;
        let device = device.ok_or_else(|| "D3D11 device creation returned null".to_string())?;
        let context =
            context.ok_or_else(|| "D3D11 device context creation returned null".to_string())?;
        Ok((swap_chain, device, context, driver_label))
    };

    try_create(D3D_DRIVER_TYPE_HARDWARE, "hardware").or_else(|hardware_err| {
        try_create(D3D_DRIVER_TYPE_WARP, "warp")
            .map_err(|warp_err| format!("{hardware_err}; WARP fallback also failed: {warp_err}"))
    })
}

fn create_shaders(
    device: &ID3D11Device,
) -> Result<(ID3D11VertexShader, ID3D11PixelShader, ID3D11PixelShader), String> {
    const SHADER_SOURCE: &str = r#"
Texture2D desktopTex : register(t0);
SamplerState desktopSampler : register(s0);

struct VsOut {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

VsOut vs_main(uint vertex_id : SV_VertexID) {
    float2 positions[4] = {
        float2(-1.0,  1.0),
        float2( 1.0,  1.0),
        float2(-1.0, -1.0),
        float2( 1.0, -1.0)
    };
    float2 uvs[4] = {
        float2(0.0, 0.0),
        float2(1.0, 0.0),
        float2(0.0, 1.0),
        float2(1.0, 1.0)
    };
    VsOut out_value;
    out_value.position = float4(positions[vertex_id], 0.0, 1.0);
    out_value.uv = uvs[vertex_id];
    return out_value;
}

float4 ps_bgra(VsOut input_value) : SV_Target {
    return desktopTex.Sample(desktopSampler, input_value.uv);
}
"#;

    const UINT_PIXEL_SHADER_SOURCE: &str = r#"
Texture2D<uint> desktopUintTex : register(t0);

struct VsOut {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

float4 unpack_bgra(uint pixel) {
    float b = (float)(pixel & 0xff) / 255.0;
    float g = (float)((pixel >> 8) & 0xff) / 255.0;
    float r = (float)((pixel >> 16) & 0xff) / 255.0;
    float a = (float)((pixel >> 24) & 0xff) / 255.0;
    return float4(r, g, b, a);
}

float4 load_bgra(int2 xy, uint width, uint height) {
    xy = clamp(xy, int2(0, 0), int2((int)width - 1, (int)height - 1));
    return unpack_bgra(desktopUintTex.Load(int3(xy, 0)));
}

float4 sample_bilinear(float2 source_pos, uint width, uint height) {
    float2 base_f = floor(source_pos);
    int2 base = int2(base_f);
    float2 frac_value = saturate(source_pos - base_f);
    float4 c00 = load_bgra(base, width, height);
    float4 c10 = load_bgra(base + int2(1, 0), width, height);
    float4 c01 = load_bgra(base + int2(0, 1), width, height);
    float4 c11 = load_bgra(base + int2(1, 1), width, height);
    return lerp(lerp(c00, c10, frac_value.x), lerp(c01, c11, frac_value.x), frac_value.y);
}

float4 ps_uint(VsOut input_value) : SV_Target {
    uint width;
    uint height;
    desktopUintTex.GetDimensions(width, height);
    float2 source_size = float2((float)width, (float)height);
    float2 source_pos = input_value.uv * source_size - 0.5;
    float2 footprint = max(abs(ddx(input_value.uv)) * source_size, abs(ddy(input_value.uv)) * source_size);
    float max_footprint = max(footprint.x, footprint.y);
    if (max_footprint > 1.15) {
        float2 half_span = max((footprint - 1.0) * 0.5, float2(0.0, 0.0));
        return 0.25 * (
            sample_bilinear(source_pos + float2(-half_span.x, -half_span.y), width, height) +
            sample_bilinear(source_pos + float2( half_span.x, -half_span.y), width, height) +
            sample_bilinear(source_pos + float2(-half_span.x,  half_span.y), width, height) +
            sample_bilinear(source_pos + float2( half_span.x,  half_span.y), width, height));
    }
    return sample_bilinear(source_pos, width, height);
}
"#;

    let vertex_blob = compile_shader(SHADER_SOURCE, "vs_main", "vs_4_0")?;
    let bgra_pixel_blob = compile_shader(SHADER_SOURCE, "ps_bgra", "ps_4_0")?;
    let uint_pixel_blob = compile_shader(UINT_PIXEL_SHADER_SOURCE, "ps_uint", "ps_4_0")?;
    let vertex_bytes = blob_bytes(&vertex_blob);
    let bgra_pixel_bytes = blob_bytes(&bgra_pixel_blob);
    let uint_pixel_bytes = blob_bytes(&uint_pixel_blob);
    let mut vertex_shader = None;
    let mut bgra_pixel_shader = None;
    let mut uint_pixel_shader = None;
    unsafe {
        device
            .CreateVertexShader(
                vertex_bytes,
                None::<&windows::Win32::Graphics::Direct3D11::ID3D11ClassLinkage>,
                Some(&mut vertex_shader),
            )
            .map_err(|err| format!("vertex shader creation failed: {err}"))?;
        device
            .CreatePixelShader(
                bgra_pixel_bytes,
                None::<&windows::Win32::Graphics::Direct3D11::ID3D11ClassLinkage>,
                Some(&mut bgra_pixel_shader),
            )
            .map_err(|err| format!("BGRA pixel shader creation failed: {err}"))?;
        device
            .CreatePixelShader(
                uint_pixel_bytes,
                None::<&windows::Win32::Graphics::Direct3D11::ID3D11ClassLinkage>,
                Some(&mut uint_pixel_shader),
            )
            .map_err(|err| format!("UINT pixel shader creation failed: {err}"))?;
    }
    let vertex_shader =
        vertex_shader.ok_or_else(|| "vertex shader creation returned null".to_string())?;
    let bgra_pixel_shader =
        bgra_pixel_shader.ok_or_else(|| "BGRA pixel shader creation returned null".to_string())?;
    let uint_pixel_shader =
        uint_pixel_shader.ok_or_else(|| "UINT pixel shader creation returned null".to_string())?;
    Ok((vertex_shader, bgra_pixel_shader, uint_pixel_shader))
}

fn create_compute_shader(device: &ID3D11Device) -> Result<ID3D11ComputeShader, String> {
    let blob = compile_shader(EXPERIMENTAL_ATX2_DECODE_SHADER, "cs_decode_atx2", "cs_5_0")?;
    let bytes = blob_bytes(&blob);
    let mut shader = None;
    unsafe {
        device
            .CreateComputeShader(
                bytes,
                None::<&windows::Win32::Graphics::Direct3D11::ID3D11ClassLinkage>,
                Some(&mut shader),
            )
            .map_err(|err| format!("experimental ATX2 compute shader creation failed: {err}"))?;
    }
    shader.ok_or_else(|| "experimental ATX2 compute shader creation returned null".to_string())
}

fn compile_shader(source: &str, entry: &str, target: &str) -> Result<ID3DBlob, String> {
    let mut code = None;
    let mut errors = None;
    let entry_name = entry;
    let target_name = target;
    let entry = match entry_name {
        "vs_main" => s!("vs_main"),
        "ps_bgra" => s!("ps_bgra"),
        "ps_uint" => s!("ps_uint"),
        "cs_decode_atx2" => s!("cs_decode_atx2"),
        other => return Err(format!("unsupported shader entry point: {other}")),
    };
    let target = match target_name {
        "vs_4_0" => s!("vs_4_0"),
        "ps_4_0" => s!("ps_4_0"),
        "cs_5_0" => s!("cs_5_0"),
        other => return Err(format!("unsupported shader target: {other}")),
    };
    let result = unsafe {
        D3DCompile(
            source.as_ptr() as *const _,
            source.len(),
            PCSTR::null(),
            None,
            None::<&ID3DInclude>,
            entry,
            target,
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    match result {
        Ok(()) => {
            code.ok_or_else(|| format!("shader compiler returned no bytecode for {entry_name}"))
        }
        Err(err) => {
            let compiler_output = errors
                .as_ref()
                .map(blob_to_string)
                .unwrap_or_else(|| "no compiler output".to_string());
            Err(format!(
                "shader compile failed for {entry_name}/{target_name}: {err}; {compiler_output}"
            ))
        }
    }
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe { slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) }
}

fn blob_to_string(blob: &ID3DBlob) -> String {
    String::from_utf8_lossy(blob_bytes(blob)).trim().to_string()
}

fn create_sampler_state(device: &ID3D11Device) -> Result<ID3D11SamplerState, String> {
    let desc = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        BorderColor: [0.0, 0.0, 0.0, 0.0],
        MinLOD: 0.0,
        MaxLOD: D3D11_FLOAT32_MAX,
    };
    let mut sampler = None;
    unsafe {
        device
            .CreateSamplerState(&desc, Some(&mut sampler))
            .map_err(|err| format!("sampler state creation failed: {err}"))?;
    }
    sampler.ok_or_else(|| "sampler state creation returned null".to_string())
}

fn create_rasterizer_state(device: &ID3D11Device) -> Result<ID3D11RasterizerState, String> {
    let desc = D3D11_RASTERIZER_DESC {
        FillMode: D3D11_FILL_SOLID,
        CullMode: D3D11_CULL_NONE,
        FrontCounterClockwise: false.into(),
        DepthBias: 0,
        DepthBiasClamp: 0.0,
        SlopeScaledDepthBias: 0.0,
        DepthClipEnable: true.into(),
        ScissorEnable: false.into(),
        MultisampleEnable: false.into(),
        AntialiasedLineEnable: false.into(),
    };
    let mut rasterizer = None;
    unsafe {
        device
            .CreateRasterizerState(&desc, Some(&mut rasterizer))
            .map_err(|err| format!("rasterizer state creation failed: {err}"))?;
    }
    rasterizer.ok_or_else(|| "rasterizer state creation returned null".to_string())
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

fn validate_bgra_len(
    width: u32,
    height: u32,
    actual_len: usize,
    label: &str,
) -> Result<(), String> {
    let expected = width as usize * height as usize * 4;
    if actual_len != expected {
        return Err(format!(
            "{label} payload length mismatch: expected {expected}, got {actual_len}"
        ));
    }
    Ok(())
}

fn copy_mapped_u32_texture(
    mapped: &D3D11_MAPPED_SUBRESOURCE,
    width: u32,
    height: u32,
) -> Result<Vec<u32>, String> {
    let row_bytes = width as usize * std::mem::size_of::<u32>();
    let pitch = mapped.RowPitch as usize;
    if mapped.pData.is_null() {
        return Err("mapped texture pointer is null".to_string());
    }
    if pitch < row_bytes {
        return Err(format!(
            "mapped texture pitch {pitch} is smaller than row bytes {row_bytes}"
        ));
    }

    let mut out = vec![0u32; width as usize * height as usize];
    for row in 0..height as usize {
        let src = unsafe { (mapped.pData as *const u8).add(row * pitch) as *const u32 };
        let src = unsafe { slice::from_raw_parts(src, width as usize) };
        let dst = &mut out[row * width as usize..(row + 1) * width as usize];
        dst.copy_from_slice(src);
    }
    Ok(out)
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

fn create_bgra_shader_resource_view(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
) -> Result<ID3D11ShaderResourceView, String> {
    let mut view = None;
    unsafe {
        device
            .CreateShaderResourceView(texture, None, Some(&mut view))
            .map_err(|err| format!("CreateShaderResourceView(BGRA) failed: {err}"))?;
    }
    view.ok_or_else(|| "CreateShaderResourceView(BGRA) returned null".to_string())
}

fn create_shader_resource_view(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
) -> Result<ID3D11ShaderResourceView, String> {
    let mut view = None;
    unsafe {
        device
            .CreateShaderResourceView(texture, None, Some(&mut view))
            .map_err(|err| format!("CreateShaderResourceView failed: {err}"))?;
    }
    view.ok_or_else(|| "CreateShaderResourceView returned null".to_string())
}

fn create_unordered_access_view(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
) -> Result<ID3D11UnorderedAccessView, String> {
    let mut view = None;
    unsafe {
        device
            .CreateUnorderedAccessView(texture, None, Some(&mut view))
            .map_err(|err| format!("CreateUnorderedAccessView failed: {err}"))?;
    }
    view.ok_or_else(|| "CreateUnorderedAccessView returned null".to_string())
}

fn create_raw_shader_resource_buffer(
    device: &ID3D11Device,
    byte_width: u32,
) -> Result<ID3D11Buffer, String> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: byte_width.max(4),
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS.0 as u32,
        StructureByteStride: 0,
    };
    let mut buffer = None;
    unsafe {
        device
            .CreateBuffer(&desc, None, Some(&mut buffer))
            .map_err(|err| format!("experimental raw command buffer creation failed: {err}"))?;
    }
    buffer.ok_or_else(|| "experimental raw command buffer creation returned null".to_string())
}

fn create_raw_shader_resource_view(
    device: &ID3D11Device,
    buffer: &ID3D11Buffer,
    byte_width: u32,
) -> Result<ID3D11ShaderResourceView, String> {
    let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_R32_TYPELESS,
        ViewDimension: D3D11_SRV_DIMENSION_BUFFEREX,
        Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
            BufferEx: D3D11_BUFFEREX_SRV {
                FirstElement: 0,
                NumElements: byte_width.div_ceil(4).max(1),
                Flags: D3D11_BUFFEREX_SRV_FLAG_RAW.0 as u32,
            },
        },
    };
    let mut view = None;
    unsafe {
        device
            .CreateShaderResourceView(buffer, Some(&desc), Some(&mut view))
            .map_err(|err| format!("experimental raw command SRV creation failed: {err}"))?;
    }
    view.ok_or_else(|| "experimental raw command SRV creation returned null".to_string())
}

fn create_experimental_decode_params(device: &ID3D11Device) -> Result<ID3D11Buffer, String> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: mem::size_of::<ExperimentalDecodeParams>() as u32,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut buffer = None;
    unsafe {
        device
            .CreateBuffer(&desc, None, Some(&mut buffer))
            .map_err(|err| format!("experimental decode params buffer creation failed: {err}"))?;
    }
    buffer.ok_or_else(|| "experimental decode params buffer creation returned null".to_string())
}

fn validate_experimental_dimensions(width: u32, height: u32, label: &str) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err(format!("experimental {label} dimensions are zero"));
    }
    if width > u16::MAX as u32 || height > u16::MAX as u32 {
        return Err(format!(
            "experimental {label} dimensions exceed ATX2 16-bit field range: {width}x{height}"
        ));
    }
    Ok(())
}

fn validate_rect_bounds(
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    limit_width: u32,
    limit_height: u32,
    label: &str,
) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err(format!("experimental {label} has zero dimensions"));
    }
    let right = x
        .checked_add(width)
        .ok_or_else(|| format!("experimental {label} x range overflows"))?;
    let bottom = y
        .checked_add(height)
        .ok_or_else(|| format!("experimental {label} y range overflows"))?;
    if right > limit_width || bottom > limit_height {
        return Err(format!(
            "experimental {label} exceeds bounds: rect=({},{} {}x{}) bounds={}x{}",
            x, y, width, height, limit_width, limit_height
        ));
    }
    Ok(())
}

fn validate_tile_command_stream(
    bytes: &[u8],
    atlas_width: u32,
    atlas_height: u32,
    desktop_width: u32,
    desktop_height: u32,
) -> Result<TileCommandStreamInfo, String> {
    if bytes.len() < TILE_COMMAND_STREAM_HEADER_BYTES {
        return Err("experimental ATX2 stream is shorter than header".to_string());
    }
    if bytes.len() % 4 != 0 {
        return Err("experimental ATX2 stream byte length is not 4-byte aligned".to_string());
    }
    let magic = read_u32(bytes, 0)?;
    let version = read_u32(bytes, 4)?;
    if magic != TILE_COMMAND_STREAM_MAGIC {
        return Err("experimental ATX2 stream has invalid magic".to_string());
    }
    if version != TILE_COMMAND_STREAM_VERSION {
        return Err(format!(
            "experimental ATX2 stream version {version} is unsupported"
        ));
    }
    let packed_dimensions = read_u32(bytes, 8)?;
    let stream_atlas_width = packed_dimensions & 0xffff;
    let stream_atlas_height = packed_dimensions >> 16;
    if stream_atlas_width != atlas_width || stream_atlas_height != atlas_height {
        return Err(format!(
            "experimental ATX2 atlas dimensions mismatch: stream={}x{} record={}x{}",
            stream_atlas_width, stream_atlas_height, atlas_width, atlas_height
        ));
    }
    let packed_tiles = read_u32(bytes, 12)?;
    let tile_size = packed_tiles & 0xffff;
    let tiles_x = packed_tiles >> 16;
    let expected_tiles_x = atlas_width.div_ceil(TILE_SIZE).max(1);
    let expected_tiles_y = atlas_height.div_ceil(TILE_SIZE).max(1);
    if tile_size != TILE_SIZE || tiles_x != expected_tiles_x {
        return Err(format!(
            "experimental ATX2 tile header mismatch: tile_size={tile_size} tiles_x={tiles_x}"
        ));
    }
    let command_count = read_u32(bytes, 16)?;
    let byte_len = read_u32(bytes, 20)?;
    let descriptor_count = read_u32(bytes, 24)?;
    let overflow = read_u32(bytes, 28)?;
    if byte_len as usize != bytes.len() {
        return Err(format!(
            "experimental ATX2 byte length mismatch: header={byte_len} actual={}",
            bytes.len()
        ));
    }
    if descriptor_count > expected_tiles_x.saturating_mul(expected_tiles_y) {
        return Err(format!(
            "experimental ATX2 descriptor count exceeds atlas tile count: header={descriptor_count} max={}",
            expected_tiles_x.saturating_mul(expected_tiles_y)
        ));
    }
    if overflow != 0 {
        return Err("experimental ATX2 stream reports encoder overflow".to_string());
    }
    if command_count > descriptor_count {
        return Err("experimental ATX2 command count exceeds descriptor count".to_string());
    }

    let mut offset = TILE_COMMAND_STREAM_HEADER_BYTES;
    let mut command_offsets = Vec::with_capacity(command_count as usize);
    let mut max_pixels_per_command = 0u32;
    let mut requires_previous = false;
    for _ in 0..command_count {
        if offset + TILE_COMMAND_HEADER_BYTES > bytes.len() {
            return Err("experimental ATX2 command header is truncated".to_string());
        }
        command_offsets.push(offset as u32);
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
        validate_rect_bounds(
            atlas_x,
            atlas_y,
            width,
            height,
            atlas_width,
            atlas_height,
            "ATX2 command atlas rect",
        )?;
        validate_rect_bounds(
            desktop_x,
            desktop_y,
            width,
            height,
            desktop_width,
            desktop_height,
            "ATX2 command desktop rect",
        )?;
        max_pixels_per_command = max_pixels_per_command.max(width.saturating_mul(height));
        let payload_offset = offset + TILE_COMMAND_HEADER_BYTES;
        let payload_end = payload_offset
            .checked_add(payload_len)
            .ok_or_else(|| "experimental ATX2 payload offset overflows".to_string())?;
        if payload_end > bytes.len() {
            return Err("experimental ATX2 command payload is truncated".to_string());
        }
        if matches!(
            kind,
            TILE_COMMAND_XOR_RAW | TILE_COMMAND_XOR_SPARSE | TILE_COMMAND_MASKED_QUANT_DELTA
        ) {
            requires_previous = true;
        }
        validate_tile_payload(
            bytes,
            payload_offset,
            payload_len,
            kind,
            width,
            height,
            changed_count,
        )?;
        offset = payload_end;
    }
    if offset != bytes.len() {
        return Err("experimental ATX2 stream has trailing bytes after commands".to_string());
    }
    Ok(TileCommandStreamInfo {
        atlas_width,
        atlas_height,
        command_count,
        byte_len,
        max_pixels_per_command,
        requires_previous,
        command_offsets,
    })
}

fn validate_tile_payload(
    bytes: &[u8],
    payload_offset: usize,
    payload_len: usize,
    kind: u32,
    width: u32,
    height: u32,
    changed_count: usize,
) -> Result<(), String> {
    let pixel_count = width as usize * height as usize;
    match kind {
        TILE_COMMAND_SOLID_COLOR => {
            if payload_len != 4 {
                return Err("experimental ATX2 solid payload length is invalid".to_string());
            }
        }
        TILE_COMMAND_RAW_BGRA => {
            let expected = pixel_count
                .checked_mul(4)
                .ok_or_else(|| "experimental ATX2 raw payload length overflows".to_string())?;
            if payload_len != expected {
                return Err("experimental ATX2 raw payload length is invalid".to_string());
            }
        }
        TILE_COMMAND_XOR_RAW => {
            let expected = pixel_count
                .checked_mul(4)
                .ok_or_else(|| "experimental ATX2 XOR raw payload length overflows".to_string())?;
            if payload_len != expected {
                return Err("experimental ATX2 XOR raw payload length is invalid".to_string());
            }
        }
        TILE_COMMAND_XOR_SPARSE => {
            if payload_len % 8 != 0 {
                return Err("experimental ATX2 XOR sparse payload length is invalid".to_string());
            }
            if changed_count > pixel_count || payload_len != changed_count.saturating_mul(8) {
                return Err("experimental ATX2 XOR sparse changed count is invalid".to_string());
            }
            for entry_offset in (payload_offset..payload_offset + payload_len).step_by(8) {
                let pixel_index = read_u32(bytes, entry_offset)? as usize;
                if pixel_index >= pixel_count {
                    return Err("experimental ATX2 XOR sparse pixel index exceeds tile".to_string());
                }
            }
        }
        TILE_COMMAND_MASKED_QUANT_DELTA => {
            let mask_words = pixel_count.div_ceil(32);
            let expected = 4usize
                .checked_add(mask_words.saturating_mul(4))
                .and_then(|value| value.checked_add(pixel_count.saturating_mul(2)))
                .map(align_usize_to_4)
                .ok_or_else(|| {
                    "experimental ATX2 masked quant delta payload length overflows".to_string()
                })?;
            if changed_count > pixel_count || payload_len != expected {
                return Err(
                    "experimental ATX2 masked quant delta payload length is invalid".to_string(),
                );
            }
            let quant_shift = read_u32(bytes, payload_offset)? & 0xff;
            if quant_shift > 4 {
                return Err("experimental ATX2 masked quant delta shift is invalid".to_string());
            }
            let mask_offset = payload_offset + 4;
            let mut mask_count = 0usize;
            for word_index in 0..mask_words {
                let word = read_u32(bytes, mask_offset + word_index * 4)?;
                mask_count = mask_count.saturating_add(word.count_ones() as usize);
            }
            if mask_count != changed_count {
                return Err(
                    "experimental ATX2 masked quant delta mask count is invalid".to_string()
                );
            }
        }
        TILE_COMMAND_LOSSY_UI_BLOCK => {
            let chroma_width = (width as usize).div_ceil(4);
            let chroma_height = (height as usize).div_ceil(4);
            let chroma_count = chroma_width.saturating_mul(chroma_height);
            let expected = 4usize
                .checked_add(align_usize_to_4(pixel_count.div_ceil(2)))
                .and_then(|value| value.checked_add(align_usize_to_4(chroma_count)))
                .ok_or_else(|| {
                    "experimental ATX2 lossy UI block payload length overflows".to_string()
                })?;
            if payload_len != expected {
                return Err(
                    "experimental ATX2 lossy UI block payload length is invalid".to_string()
                );
            }
            let header = read_u32(bytes, payload_offset)?;
            if (header & 0xffff) as usize != chroma_width
                || (header >> 16) as usize != chroma_height
            {
                return Err(
                    "experimental ATX2 lossy UI block chroma dimensions are invalid".to_string(),
                );
            }
        }
        TILE_COMMAND_SHARP_UI_BLOCK => {
            let expected = pixel_count
                .checked_mul(2)
                .map(align_usize_to_4)
                .ok_or_else(|| {
                    "experimental ATX2 sharp UI block payload length overflows".to_string()
                })?;
            if payload_len != expected {
                return Err(
                    "experimental ATX2 sharp UI block payload length is invalid".to_string()
                );
            }
        }
        other => {
            return Err(format!("experimental ATX2 command kind {other} is unknown"));
        }
    }
    Ok(())
}

fn align_usize_to_4(value: usize) -> usize {
    (value + 3) & !3
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| "u32 offset overflow".to_string())?;
    let value = bytes
        .get(offset..end)
        .ok_or_else(|| "u32 read is out of bounds".to_string())?;
    Ok(u32::from_le_bytes(value.try_into().unwrap()))
}

const EXPERIMENTAL_ATX2_DECODE_SHADER: &str = r#"
cbuffer ExperimentalDecodeParams : register(b0) {
    uint desktop_width;
    uint desktop_height;
    uint atlas_width;
    uint atlas_height;
    uint stream_byte_len;
    uint command_count;
    uint max_pixels_per_command;
    uint _pad0;
};

ByteAddressBuffer commandBuf : register(t0);
ByteAddressBuffer commandOffsets : register(t1);
Texture2D<uint> previousDesktop : register(t2);
RWTexture2D<uint> atlasTex : register(u0);

static const uint STREAM_HEADER_BYTES = 32;
static const uint COMMAND_HEADER_BYTES = 24;
static const uint COMMAND_RAW_BGRA = 1;
static const uint COMMAND_SOLID_COLOR = 2;
static const uint COMMAND_XOR_RAW = 3;
static const uint COMMAND_XOR_SPARSE = 4;
static const uint COMMAND_MASKED_QUANT_DELTA = 5;
static const uint COMMAND_LOSSY_UI_BLOCK = 6;
static const uint COMMAND_SHARP_UI_BLOCK = 7;

void store_pixel(uint atlas_x, uint atlas_y, uint width, uint pixel_index, uint color) {
    uint x = atlas_x + (pixel_index % width);
    uint y = atlas_y + (pixel_index / width);
    if (x < atlas_width && y < atlas_height) {
        atlasTex[uint2(x, y)] = color;
    }
}

uint previous_pixel(uint desktop_x, uint desktop_y, uint width, uint pixel_index) {
    uint x = desktop_x + (pixel_index % width);
    uint y = desktop_y + (pixel_index / width);
    if (x >= desktop_width || y >= desktop_height) {
        return 0;
    }
    return previousDesktop[uint2(x, y)];
}

uint apply_quant_component(uint previous_channel, uint raw_delta, uint bits, uint quant_shift) {
    uint sign_bit = 1u << (bits - 1);
    uint mask = (1u << bits) - 1;
    bool negative = (raw_delta & sign_bit) != 0;
    uint magnitude = negative ? (((~raw_delta) + 1u) & mask) : raw_delta;
    uint delta = magnitude << quant_shift;
    if (negative) {
        return previous_channel > delta ? previous_channel - delta : 0;
    }
    uint value = previous_channel + delta;
    return value > 255 ? 255 : value;
}

uint apply_masked_quant_delta(uint previous, uint packed_delta, uint quant_shift) {
    uint b = apply_quant_component(previous & 0xff, packed_delta & 0x1f, 5, quant_shift);
    uint g = apply_quant_component((previous >> 8) & 0xff, (packed_delta >> 5) & 0x3f, 6, quant_shift);
    uint r = apply_quant_component((previous >> 16) & 0xff, (packed_delta >> 11) & 0x1f, 5, quant_shift);
    return b | (g << 8) | (r << 16) | (previous & 0xff000000);
}

uint clamp_byte_int(int value) {
    if (value < 0) {
        return 0;
    }
    if (value > 255) {
        return 255;
    }
    return uint(value);
}

int sign_extend4(uint value) {
    int signed_value = int(value & 0x0f);
    if (signed_value >= 8) {
        signed_value -= 16;
    }
    return signed_value;
}

uint ycocg4_to_bgra(uint y4, uint chroma_byte) {
    uint y_byte = (y4 << 4) | y4;
    int y = int(y_byte);
    int co = sign_extend4(chroma_byte & 0x0f) * 32;
    int cg = sign_extend4((chroma_byte >> 4) & 0x0f) * 32;
    int tmp = y - (cg >> 1);
    int g = cg + tmp;
    int b = tmp - (co >> 1);
    int r = b + co;
    return clamp_byte_int(b) | (clamp_byte_int(g) << 8) | (clamp_byte_int(r) << 16) | 0xff000000;
}

uint rgb565_to_bgra(uint value) {
    uint b5 = value & 0x1f;
    uint g6 = (value >> 5) & 0x3f;
    uint r5 = (value >> 11) & 0x1f;
    uint b = (b5 << 3) | (b5 >> 2);
    uint g = (g6 << 2) | (g6 >> 4);
    uint r = (r5 << 3) | (r5 >> 2);
    return b | (g << 8) | (r << 16) | 0xff000000;
}

[numthreads(64, 1, 1)]
void cs_decode_atx2(uint3 dispatch_id : SV_DispatchThreadID) {
    uint pixel = dispatch_id.x;
    uint command_index = dispatch_id.y;
    if (command_index >= command_count || pixel >= max_pixels_per_command) {
        return;
    }

    uint offset = commandOffsets.Load(command_index * 4);
    if (offset + COMMAND_HEADER_BYTES > stream_byte_len) {
        return;
    }

    uint kind = commandBuf.Load(offset);
    uint atlas_xy = commandBuf.Load(offset + 4);
    uint desktop_xy = commandBuf.Load(offset + 8);
    uint wh = commandBuf.Load(offset + 12);
    uint payload_len = commandBuf.Load(offset + 16);
    uint changed_count = commandBuf.Load(offset + 20);
    uint atlas_x = atlas_xy & 0xffff;
    uint atlas_y = atlas_xy >> 16;
    uint desktop_x = desktop_xy & 0xffff;
    uint desktop_y = desktop_xy >> 16;
    uint width = wh & 0xffff;
    uint height = wh >> 16;
    uint payload_offset = offset + COMMAND_HEADER_BYTES;
    uint pixel_count = width * height;
    if (width == 0 || height == 0 || pixel >= pixel_count || payload_offset + payload_len > stream_byte_len) {
        return;
    }

    if (kind == COMMAND_SOLID_COLOR) {
        uint color = commandBuf.Load(payload_offset);
        store_pixel(atlas_x, atlas_y, width, pixel, color);
    } else if (kind == COMMAND_RAW_BGRA) {
        uint color = commandBuf.Load(payload_offset + pixel * 4);
        store_pixel(atlas_x, atlas_y, width, pixel, color);
    } else if (kind == COMMAND_XOR_RAW) {
        uint color = previous_pixel(desktop_x, desktop_y, width, pixel)
            ^ commandBuf.Load(payload_offset + pixel * 4);
        store_pixel(atlas_x, atlas_y, width, pixel, color);
    } else if (kind == COMMAND_XOR_SPARSE) {
        uint color = previous_pixel(desktop_x, desktop_y, width, pixel);
        for (uint entry = 0; entry < changed_count; entry += 1) {
            uint entry_offset = payload_offset + entry * 8;
            uint sparse_pixel = commandBuf.Load(entry_offset);
            if (sparse_pixel == pixel) {
                color ^= commandBuf.Load(entry_offset + 4);
                break;
            }
        }
        store_pixel(atlas_x, atlas_y, width, pixel, color);
    } else if (kind == COMMAND_MASKED_QUANT_DELTA) {
        uint previous = previous_pixel(desktop_x, desktop_y, width, pixel);
        uint pixel_count = width * height;
        uint mask_words = (pixel_count + 31) / 32;
        uint quant_shift = commandBuf.Load(payload_offset) & 0xff;
        uint mask_offset = payload_offset + 4;
        uint residual_offset = mask_offset + mask_words * 4;
        uint mask_word = commandBuf.Load(mask_offset + (pixel / 32) * 4);
        uint color = previous;
        if ((mask_word & (1u << (pixel & 31))) != 0) {
            uint residual_word = commandBuf.Load(residual_offset + (pixel / 2) * 4);
            uint packed_delta = ((pixel & 1) == 0) ? (residual_word & 0xffff) : (residual_word >> 16);
            color = apply_masked_quant_delta(previous, packed_delta, quant_shift);
        }
        store_pixel(atlas_x, atlas_y, width, pixel, color);
    } else if (kind == COMMAND_LOSSY_UI_BLOCK) {
        uint pixel_x = pixel % width;
        uint pixel_y = pixel / width;
        uint chroma_width = (width + 3) / 4;
        uint chroma_index = (pixel_y / 4) * chroma_width + (pixel_x / 4);
        uint y_offset = payload_offset + 4;
        uint chroma_offset = y_offset + (((pixel_count + 1) / 2 + 3) & ~3);
        uint y_word = commandBuf.Load(y_offset + (pixel / 8) * 4);
        uint y4 = (y_word >> ((pixel & 7) * 4)) & 0x0f;
        uint chroma_word = commandBuf.Load(chroma_offset + (chroma_index / 4) * 4);
        uint chroma_byte = (chroma_word >> ((chroma_index & 3) * 8)) & 0xff;
        store_pixel(atlas_x, atlas_y, width, pixel, ycocg4_to_bgra(y4, chroma_byte));
    } else if (kind == COMMAND_SHARP_UI_BLOCK) {
        uint word = commandBuf.Load(payload_offset + (pixel / 2) * 4);
        uint packed = ((pixel & 1) == 0) ? (word & 0xffff) : (word >> 16);
        store_pixel(atlas_x, atlas_y, width, pixel, rgb565_to_bgra(packed));
    }
}
"#;
