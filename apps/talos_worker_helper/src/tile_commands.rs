#![cfg(target_os = "windows")]

use std::{
    mem, slice,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use windows::{
    core::{s, PCSTR},
    Win32::Graphics::{
        Direct3D::{Fxc::D3DCompile, ID3DBlob, ID3DInclude, D3D11_SRV_DIMENSION_BUFFEREX},
        Direct3D11::{
            ID3D11Buffer, ID3D11ClassLinkage, ID3D11ComputeShader, ID3D11Device,
            ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
            ID3D11UnorderedAccessView, D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE,
            D3D11_BIND_UNORDERED_ACCESS, D3D11_BOX, D3D11_BUFFEREX_SRV,
            D3D11_BUFFEREX_SRV_FLAG_RAW, D3D11_BUFFER_DESC, D3D11_BUFFER_UAV,
            D3D11_BUFFER_UAV_FLAG_RAW, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE,
            D3D11_MAP_READ, D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS,
            D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC_0,
            D3D11_TEXTURE2D_DESC, D3D11_UAV_DIMENSION_BUFFER, D3D11_UNORDERED_ACCESS_VIEW_DESC,
            D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
        },
        Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R32_TYPELESS, DXGI_SAMPLE_DESC},
    },
};

pub(crate) const TILE_COMMAND_STREAM_MAGIC: u32 = 0x3258_5441; // "ATX2" little-endian.
pub(crate) const TILE_COMMAND_STREAM_VERSION: u32 = 4;
pub(crate) const TILE_COMMAND_STREAM_HEADER_BYTES: u32 = 32;
pub(crate) const TILE_COMMAND_HEADER_BYTES: u32 = 24;
pub(crate) const TILE_SIZE: u32 = 32;

const TILE_COMMAND_RAW_BGRA: u32 = 1;
const TILE_COMMAND_SOLID_COLOR: u32 = 2;
const TILE_COMMAND_XOR_RAW: u32 = 3;
const TILE_COMMAND_XOR_SPARSE: u32 = 4;
const TILE_COMMAND_MASKED_QUANT_DELTA: u32 = 5;
const TILE_COMMAND_LOSSY_UI_BLOCK: u32 = 6;
const TILE_COMMAND_SHARP_UI_BLOCK: u32 = 7;
const TILE_MAP_ENTRY_BYTES: u32 = 16;
const ANALYSIS_ENTRY_BYTES: u32 = 16;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TileCommandTile {
    pub atlas_xy: u32,
    pub desktop_xy: u32,
    pub wh: u32,
    pub flags: u32,
}

impl TileCommandTile {
    pub(crate) fn new(
        atlas_x: u32,
        atlas_y: u32,
        desktop_x: u32,
        desktop_y: u32,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        anyhow::ensure!(
            atlas_x <= u16::MAX as u32
                && atlas_y <= u16::MAX as u32
                && desktop_x <= u16::MAX as u32
                && desktop_y <= u16::MAX as u32
                && width <= u16::MAX as u32
                && height <= u16::MAX as u32,
            "tile command descriptor exceeds 16-bit field range"
        );
        anyhow::ensure!(width > 0 && height > 0, "tile command descriptor is empty");
        Ok(Self {
            atlas_xy: pack_xy(atlas_x, atlas_y),
            desktop_xy: pack_xy(desktop_x, desktop_y),
            wh: pack_xy(width, height),
            flags: 1,
        })
    }

    fn width(self) -> u32 {
        self.wh & 0xffff
    }

    fn height(self) -> u32 {
        self.wh >> 16
    }
}

const _: () = assert!(mem::size_of::<TileCommandTile>() == TILE_MAP_ENTRY_BYTES as usize);

#[derive(Clone, Debug)]
pub(crate) struct TileCommandStream {
    pub atlas_width: u32,
    pub atlas_height: u32,
    pub descriptor_count: u32,
    pub command_count: u32,
    pub byte_len: u32,
    pub raw_equivalent_bytes: u32,
    pub delta_bytes_saved_estimate: u32,
    pub copy_rects: Vec<TileCommandCopyRect>,
    pub bytes: Vec<u8>,
    pub command_counts: TileCommandCounts,
    pub timings: TileCommandEncodeTimings,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TileCommandCopyRect {
    pub atlas_x: u32,
    pub atlas_y: u32,
    pub desktop_x: u32,
    pub desktop_y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TileCommandEncodeTimings {
    pub setup: Duration,
    pub dispatch: Duration,
    pub staging_copy: Duration,
    pub map_wait: Duration,
    pub result_parse: Duration,
    pub total: Duration,
}

impl TileCommandStream {
    pub(crate) fn empty(atlas_width: u32, atlas_height: u32) -> Self {
        let bytes = build_empty_stream(atlas_width, atlas_height);
        let byte_len = bytes.len() as u32;
        Self {
            atlas_width,
            atlas_height,
            descriptor_count: 0,
            command_count: 0,
            byte_len,
            raw_equivalent_bytes: byte_len,
            delta_bytes_saved_estimate: 0,
            copy_rects: Vec::new(),
            bytes,
            command_counts: TileCommandCounts::default(),
            timings: TileCommandEncodeTimings::default(),
        }
    }

    pub(crate) fn descriptor_count(&self) -> u32 {
        self.descriptor_count
    }

    pub(crate) fn solid_color(atlas_width: u32, atlas_height: u32, bgra: [u8; 4]) -> Result<Self> {
        anyhow::ensure!(
            atlas_width > 0 && atlas_height > 0,
            "solid tile command stream requires non-zero atlas dimensions"
        );
        anyhow::ensure!(
            atlas_width <= u16::MAX as u32 && atlas_height <= u16::MAX as u32,
            "solid tile command stream dimensions exceed ATX2 16-bit fields"
        );
        let tiles_x = atlas_width.div_ceil(TILE_SIZE).max(1);
        let byte_len = TILE_COMMAND_STREAM_HEADER_BYTES + TILE_COMMAND_HEADER_BYTES + 4;
        let mut bytes = vec![0u8; byte_len as usize];
        write_stream_header(
            &mut bytes,
            atlas_width,
            atlas_height,
            tiles_x,
            1,
            byte_len,
            1,
            0,
        );
        let command_offset = TILE_COMMAND_STREAM_HEADER_BYTES as usize;
        write_command_header(
            &mut bytes,
            command_offset,
            TILE_COMMAND_SOLID_COLOR,
            pack_xy(0, 0),
            pack_xy(0, 0),
            pack_xy(atlas_width, atlas_height),
            4,
            atlas_width.saturating_mul(atlas_height),
        );
        bytes[command_offset + TILE_COMMAND_HEADER_BYTES as usize
            ..command_offset + TILE_COMMAND_HEADER_BYTES as usize + 4]
            .copy_from_slice(&bgra);
        Ok(Self {
            atlas_width,
            atlas_height,
            descriptor_count: 1,
            command_count: 1,
            byte_len,
            raw_equivalent_bytes: byte_len,
            delta_bytes_saved_estimate: 0,
            copy_rects: vec![TileCommandCopyRect {
                atlas_x: 0,
                atlas_y: 0,
                desktop_x: 0,
                desktop_y: 0,
                width: atlas_width,
                height: atlas_height,
            }],
            bytes,
            command_counts: TileCommandCounts {
                solid: 1,
                ..TileCommandCounts::default()
            },
            timings: TileCommandEncodeTimings::default(),
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TileCommandCounts {
    pub solid: usize,
    pub raw_key: usize,
    pub xor_raw: usize,
    pub xor_sparse: usize,
    pub masked_quant_delta: usize,
    pub lossy_ui_block: usize,
    pub sharp_ui_block: usize,
    pub skipped: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct TileCommandWireChunk {
    pub bytes: Vec<u8>,
    pub copy_rects: Vec<TileCommandCopyRect>,
}

#[derive(Clone)]
struct OwnedCommand {
    kind: u32,
    atlas_xy: u32,
    desktop_xy: u32,
    wh: u32,
    changed_count: u32,
    payload: Vec<u8>,
}

impl OwnedCommand {
    fn atlas_x(&self) -> u32 {
        self.atlas_xy & 0xffff
    }

    fn atlas_y(&self) -> u32 {
        self.atlas_xy >> 16
    }

    fn desktop_x(&self) -> u32 {
        self.desktop_xy & 0xffff
    }

    fn desktop_y(&self) -> u32 {
        self.desktop_xy >> 16
    }

    fn width(&self) -> u32 {
        self.wh & 0xffff
    }

    fn height(&self) -> u32 {
        self.wh >> 16
    }

    fn encoded_len(&self) -> usize {
        TILE_COMMAND_HEADER_BYTES as usize + self.payload.len()
    }

    fn copy_rect(&self) -> TileCommandCopyRect {
        TileCommandCopyRect {
            atlas_x: self.atlas_x(),
            atlas_y: self.atlas_y(),
            desktop_x: self.desktop_x(),
            desktop_y: self.desktop_y(),
            width: self.width(),
            height: self.height(),
        }
    }
}

#[derive(Clone, Copy)]
struct CommandRef {
    kind: u32,
    atlas_xy: u32,
    desktop_xy: u32,
    wh: u32,
    payload_len: usize,
    changed_count: u32,
    command_offset: usize,
    payload_offset: usize,
}

impl CommandRef {
    fn atlas_x(self) -> u32 {
        self.atlas_xy & 0xffff
    }

    fn atlas_y(self) -> u32 {
        self.atlas_xy >> 16
    }

    fn desktop_x(self) -> u32 {
        self.desktop_xy & 0xffff
    }

    fn desktop_y(self) -> u32 {
        self.desktop_xy >> 16
    }

    fn width(self) -> u32 {
        self.wh & 0xffff
    }

    fn height(self) -> u32 {
        self.wh >> 16
    }

    fn payload_end(self) -> usize {
        self.payload_offset + self.payload_len
    }
}

impl TileCommandStream {
    pub(crate) fn wire_chunks(&self, target_bytes: usize) -> Result<Vec<TileCommandWireChunk>> {
        let commands = parse_owned_commands(&self.bytes)?;
        if commands.is_empty() {
            return Ok(vec![TileCommandWireChunk {
                bytes: self.bytes.clone(),
                copy_rects: Vec::new(),
            }]);
        }
        let target_bytes = target_bytes.max(TILE_COMMAND_STREAM_HEADER_BYTES as usize + 1);
        let mut chunks = Vec::new();
        let mut current = Vec::new();
        let mut current_bytes = TILE_COMMAND_STREAM_HEADER_BYTES as usize;
        for command in commands {
            let command_len = command.encoded_len();
            if !current.is_empty() && current_bytes.saturating_add(command_len) > target_bytes {
                chunks.push(build_wire_chunk(
                    self.atlas_width,
                    self.atlas_height,
                    std::mem::take(&mut current),
                )?);
                current_bytes = TILE_COMMAND_STREAM_HEADER_BYTES as usize;
            }
            current_bytes = current_bytes.saturating_add(command_len);
            current.push(command);
        }
        if !current.is_empty() {
            chunks.push(build_wire_chunk(
                self.atlas_width,
                self.atlas_height,
                current,
            )?);
        }
        Ok(chunks)
    }
}

pub(crate) struct TileCommandEncoder {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    init_shader: ID3D11ComputeShader,
    classify_shader: ID3D11ComputeShader,
    emit_shader: ID3D11ComputeShader,
    params_buffer: ID3D11Buffer,
    source_srv: Option<ID3D11ShaderResourceView>,
    previous_texture: Option<ID3D11Texture2D>,
    previous_srv: Option<ID3D11ShaderResourceView>,
    previous_width: u32,
    previous_height: u32,
    previous_format: DXGI_FORMAT,
    reference_valid: bool,
    reference_exact: bool,
    exact_map_buffer: Option<ID3D11Buffer>,
    exact_map_srv: Option<ID3D11ShaderResourceView>,
    exact_map_capacity: u32,
    exact_map: Vec<u32>,
    exact_tiles_x: u32,
    exact_tiles_y: u32,
    exact_desktop_width: u32,
    exact_desktop_height: u32,
    tile_map_buffer: Option<ID3D11Buffer>,
    tile_map_srv: Option<ID3D11ShaderResourceView>,
    tile_map_capacity: u32,
    analysis_buffer: Option<ID3D11Buffer>,
    analysis_srv: Option<ID3D11ShaderResourceView>,
    analysis_uav: Option<ID3D11UnorderedAccessView>,
    analysis_capacity: u32,
    output_buffer: Option<ID3D11Buffer>,
    output_uav: Option<ID3D11UnorderedAccessView>,
    staging_buffer: Option<ID3D11Buffer>,
    output_capacity: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TileCommandParams {
    atlas_width: u32,
    atlas_height: u32,
    desktop_width: u32,
    desktop_height: u32,
    tile_count: u32,
    output_capacity: u32,
    reference_enabled: u32,
    delta_exact_enabled: u32,
    desktop_tiles_x: u32,
    desktop_tiles_y: u32,
    _pad0: u32,
    _pad1: u32,
}

const _: () = assert!(mem::size_of::<TileCommandParams>() % 16 == 0);

impl TileCommandEncoder {
    pub(crate) fn new(device: ID3D11Device, context: ID3D11DeviceContext) -> Result<Self> {
        let init_shader = create_compute_shader(&device, INIT_SHADER_SOURCE, "init")?;
        let classify_shader = create_compute_shader(&device, CLASSIFY_SHADER_SOURCE, "classify")?;
        let emit_shader = create_compute_shader(&device, EMIT_SHADER_SOURCE, "emit")?;
        let params_buffer = create_params_buffer(&device)?;
        Ok(Self {
            device,
            context,
            init_shader,
            classify_shader,
            emit_shader,
            params_buffer,
            source_srv: None,
            previous_texture: None,
            previous_srv: None,
            previous_width: 0,
            previous_height: 0,
            previous_format: DXGI_FORMAT(0),
            reference_valid: false,
            reference_exact: false,
            exact_map_buffer: None,
            exact_map_srv: None,
            exact_map_capacity: 0,
            exact_map: Vec::new(),
            exact_tiles_x: 0,
            exact_tiles_y: 0,
            exact_desktop_width: 0,
            exact_desktop_height: 0,
            tile_map_buffer: None,
            tile_map_srv: None,
            tile_map_capacity: 0,
            analysis_buffer: None,
            analysis_srv: None,
            analysis_uav: None,
            analysis_capacity: 0,
            output_buffer: None,
            output_uav: None,
            staging_buffer: None,
            output_capacity: 0,
        })
    }

    pub(crate) fn reset_tile_cache(&mut self) {
        self.reference_valid = false;
        self.reference_exact = false;
        self.clear_exact_map();
    }

    pub(crate) fn commit_reference(
        &mut self,
        desktop_texture: &ID3D11Texture2D,
        desktop_width: u32,
        desktop_height: u32,
        desktop_format: DXGI_FORMAT,
        emitted_lossy: bool,
        full_lossless_refresh: bool,
    ) -> Result<()> {
        let previous_reference_exact = self.reference_valid && self.reference_exact;
        self.ensure_reference_texture(desktop_width, desktop_height, desktop_format)?;
        let previous = self
            .previous_texture
            .as_ref()
            .context("delta reference texture unavailable")?;
        unsafe {
            self.context.CopyResource(previous, desktop_texture);
        }
        self.reference_valid = true;
        self.reference_exact =
            !emitted_lossy && (previous_reference_exact || full_lossless_refresh);
        Ok(())
    }

    pub(crate) fn encode_atlas(
        &mut self,
        atlas_texture: &ID3D11Texture2D,
        atlas_width: u32,
        atlas_height: u32,
        desktop_width: u32,
        desktop_height: u32,
        desktop_format: DXGI_FORMAT,
        tiles: &[TileCommandTile],
        allow_delta: bool,
    ) -> Result<TileCommandStream> {
        let total_started = Instant::now();
        if tiles.is_empty() {
            return Ok(TileCommandStream::empty(atlas_width, atlas_height));
        }

        let mut timings = TileCommandEncodeTimings::default();
        let descriptor_count = u32::try_from(tiles.len()).context("too many tile descriptors")?;
        let output_capacity = command_stream_capacity(tiles)?;
        let raw_equivalent_bytes = raw_equivalent_stream_bytes(tiles)?;

        let setup_started = Instant::now();
        self.ensure_resources(
            atlas_texture,
            desktop_width,
            desktop_height,
            desktop_format,
            descriptor_count,
            output_capacity,
        )?;
        self.upload_tile_map(tiles)?;
        let reference_enabled = allow_delta
            && self.reference_valid
            && self.previous_width == desktop_width
            && self.previous_height == desktop_height
            && self.previous_format == desktop_format;
        if !reference_enabled || !allow_delta {
            self.clear_exact_map();
        }
        self.upload_exact_map()?;
        self.update_params(
            atlas_width,
            atlas_height,
            desktop_width,
            desktop_height,
            descriptor_count,
            output_capacity,
            reference_enabled,
            false,
        );
        timings.setup = setup_started.elapsed();

        let dispatch_started = Instant::now();
        self.dispatch_init();
        self.dispatch_classify(descriptor_count);
        self.dispatch_emit(descriptor_count);
        timings.dispatch = dispatch_started.elapsed();

        let mut stream = self.read_stream(
            atlas_width,
            atlas_height,
            descriptor_count,
            raw_equivalent_bytes,
            &mut timings,
        )?;
        self.update_exact_map_from_stream(&stream)?;
        timings.total = total_started.elapsed();
        stream.timings = timings;
        Ok(stream)
    }

    fn ensure_resources(
        &mut self,
        atlas_texture: &ID3D11Texture2D,
        desktop_width: u32,
        desktop_height: u32,
        desktop_format: DXGI_FORMAT,
        descriptor_count: u32,
        output_capacity: u32,
    ) -> Result<()> {
        let mut srv = None;
        unsafe {
            self.device
                .CreateShaderResourceView(atlas_texture, None, Some(&mut srv))
                .map_err(|err| anyhow!("tile command atlas SRV creation failed: {err}"))?;
        }
        self.source_srv = Some(srv.context("tile command atlas SRV creation returned null")?);
        self.ensure_reference_texture(desktop_width, desktop_height, desktop_format)?;
        self.ensure_exact_map(desktop_width, desktop_height)?;

        let tile_map_bytes = descriptor_count
            .saturating_mul(TILE_MAP_ENTRY_BYTES)
            .max(TILE_MAP_ENTRY_BYTES);
        if self.tile_map_buffer.is_none() || tile_map_bytes > self.tile_map_capacity {
            let buffer = create_raw_buffer(
                &self.device,
                tile_map_bytes,
                D3D11_USAGE_DEFAULT,
                D3D11_BIND_SHADER_RESOURCE.0 as u32,
                0,
                D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS.0 as u32,
            )?;
            let srv = create_raw_buffer_srv(&self.device, &buffer, tile_map_bytes)?;
            self.tile_map_buffer = Some(buffer);
            self.tile_map_srv = Some(srv);
            self.tile_map_capacity = tile_map_bytes;
        }

        let analysis_bytes = descriptor_count
            .saturating_mul(ANALYSIS_ENTRY_BYTES)
            .max(ANALYSIS_ENTRY_BYTES);
        if self.analysis_buffer.is_none() || analysis_bytes > self.analysis_capacity {
            let buffer = create_raw_buffer(
                &self.device,
                analysis_bytes,
                D3D11_USAGE_DEFAULT,
                (D3D11_BIND_UNORDERED_ACCESS.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                0,
                D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS.0 as u32,
            )?;
            let srv = create_raw_buffer_srv(&self.device, &buffer, analysis_bytes)?;
            let uav = create_raw_buffer_uav(&self.device, &buffer, analysis_bytes)?;
            self.analysis_buffer = Some(buffer);
            self.analysis_srv = Some(srv);
            self.analysis_uav = Some(uav);
            self.analysis_capacity = analysis_bytes;
        }

        if self.output_buffer.is_none() || output_capacity > self.output_capacity {
            let output_buffer = create_raw_buffer(
                &self.device,
                output_capacity,
                D3D11_USAGE_DEFAULT,
                D3D11_BIND_UNORDERED_ACCESS.0 as u32,
                0,
                D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS.0 as u32,
            )?;
            let output_uav = create_raw_buffer_uav(&self.device, &output_buffer, output_capacity)?;
            let staging_buffer = create_raw_buffer(
                &self.device,
                output_capacity,
                D3D11_USAGE_STAGING,
                0,
                D3D11_CPU_ACCESS_READ.0 as u32,
                0,
            )?;
            self.output_buffer = Some(output_buffer);
            self.output_uav = Some(output_uav);
            self.staging_buffer = Some(staging_buffer);
            self.output_capacity = output_capacity;
        }
        Ok(())
    }

    fn ensure_reference_texture(
        &mut self,
        desktop_width: u32,
        desktop_height: u32,
        desktop_format: DXGI_FORMAT,
    ) -> Result<()> {
        if self.previous_texture.is_some()
            && self.previous_srv.is_some()
            && self.previous_width == desktop_width
            && self.previous_height == desktop_height
            && self.previous_format == desktop_format
        {
            return Ok(());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: desktop_width.max(1),
            Height: desktop_height.max(1),
            MipLevels: 1,
            ArraySize: 1,
            Format: desktop_format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .map_err(|err| anyhow!("delta reference texture creation failed: {err}"))?;
        }
        let texture = texture.context("delta reference texture creation returned null")?;
        let mut srv = None;
        unsafe {
            self.device
                .CreateShaderResourceView(&texture, None, Some(&mut srv))
                .map_err(|err| anyhow!("delta reference SRV creation failed: {err}"))?;
        }
        self.previous_texture = Some(texture);
        self.previous_srv = Some(srv.context("delta reference SRV creation returned null")?);
        self.previous_width = desktop_width;
        self.previous_height = desktop_height;
        self.previous_format = desktop_format;
        self.reference_valid = false;
        self.reference_exact = false;
        Ok(())
    }

    fn ensure_exact_map(&mut self, desktop_width: u32, desktop_height: u32) -> Result<()> {
        let tiles_x = desktop_width.div_ceil(TILE_SIZE).max(1);
        let tiles_y = desktop_height.div_ceil(TILE_SIZE).max(1);
        let tile_count = tiles_x
            .checked_mul(tiles_y)
            .context("exact tile map dimensions overflow")?;
        let byte_len = tile_count.saturating_mul(4).max(4);
        if self.exact_tiles_x != tiles_x
            || self.exact_tiles_y != tiles_y
            || self.exact_desktop_width != desktop_width
            || self.exact_desktop_height != desktop_height
        {
            self.exact_map = vec![0; tile_count as usize];
            self.exact_tiles_x = tiles_x;
            self.exact_tiles_y = tiles_y;
            self.exact_desktop_width = desktop_width;
            self.exact_desktop_height = desktop_height;
            self.exact_map_buffer = None;
            self.exact_map_srv = None;
            self.exact_map_capacity = 0;
        }
        if self.exact_map_buffer.is_none() || byte_len > self.exact_map_capacity {
            let buffer = create_raw_buffer(
                &self.device,
                byte_len,
                D3D11_USAGE_DEFAULT,
                D3D11_BIND_SHADER_RESOURCE.0 as u32,
                0,
                D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS.0 as u32,
            )?;
            let srv = create_raw_buffer_srv(&self.device, &buffer, byte_len)?;
            self.exact_map_buffer = Some(buffer);
            self.exact_map_srv = Some(srv);
            self.exact_map_capacity = byte_len;
        }
        Ok(())
    }

    fn clear_exact_map(&mut self) {
        for value in &mut self.exact_map {
            *value = 0;
        }
    }

    fn upload_exact_map(&self) -> Result<()> {
        let buffer = self
            .exact_map_buffer
            .as_ref()
            .context("exact tile map buffer unavailable")?;
        let byte_len = u32::try_from(self.exact_map.len().saturating_mul(4))
            .context("exact tile map byte length overflows u32")?
            .max(4);
        let update_box = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: byte_len,
            bottom: 1,
            back: 1,
        };
        unsafe {
            self.context.UpdateSubresource(
                buffer,
                0,
                Some(&update_box),
                self.exact_map.as_ptr() as *const _,
                0,
                0,
            );
        }
        Ok(())
    }

    fn update_exact_map_from_stream(&mut self, stream: &TileCommandStream) -> Result<()> {
        let commands = parse_command_refs(&stream.bytes)?;
        for command in commands {
            let exact = command_kind_is_exact(command.kind);
            self.mark_command_exactness(command, exact);
        }
        self.reference_exact = !self.exact_map.is_empty() && self.exact_map.iter().all(|v| *v != 0);
        Ok(())
    }

    fn mark_command_exactness(&mut self, command: CommandRef, exact: bool) {
        if self.exact_map.is_empty() || self.exact_tiles_x == 0 || self.exact_tiles_y == 0 {
            return;
        }
        let Some(right) = command.desktop_x().checked_add(command.width()) else {
            return;
        };
        let Some(bottom) = command.desktop_y().checked_add(command.height()) else {
            return;
        };
        if command.width() == 0 || command.height() == 0 {
            return;
        }
        let first_x = command.desktop_x() / TILE_SIZE;
        let last_x = right.saturating_sub(1) / TILE_SIZE;
        let first_y = command.desktop_y() / TILE_SIZE;
        let last_y = bottom.saturating_sub(1) / TILE_SIZE;
        for tile_y in first_y..=last_y.min(self.exact_tiles_y.saturating_sub(1)) {
            for tile_x in first_x..=last_x.min(self.exact_tiles_x.saturating_sub(1)) {
                let index = (tile_y as usize)
                    .saturating_mul(self.exact_tiles_x as usize)
                    .saturating_add(tile_x as usize);
                let Some(slot) = self.exact_map.get_mut(index) else {
                    continue;
                };
                if !exact {
                    *slot = 0;
                    continue;
                }
                let cell_left = tile_x.saturating_mul(TILE_SIZE);
                let cell_top = tile_y.saturating_mul(TILE_SIZE);
                let cell_right = cell_left
                    .saturating_add(TILE_SIZE)
                    .min(self.exact_desktop_width);
                let cell_bottom = cell_top
                    .saturating_add(TILE_SIZE)
                    .min(self.exact_desktop_height);
                if command.desktop_x() <= cell_left
                    && command.desktop_y() <= cell_top
                    && right >= cell_right
                    && bottom >= cell_bottom
                {
                    *slot = 1;
                }
            }
        }
    }

    fn upload_tile_map(&self, tiles: &[TileCommandTile]) -> Result<()> {
        let buffer = self
            .tile_map_buffer
            .as_ref()
            .context("tile map buffer unavailable")?;
        let byte_len = u32::try_from(
            tiles
                .len()
                .saturating_mul(mem::size_of::<TileCommandTile>()),
        )
        .context("tile map byte length overflows u32")?;
        let update_box = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: byte_len,
            bottom: 1,
            back: 1,
        };
        unsafe {
            self.context.UpdateSubresource(
                buffer,
                0,
                Some(&update_box),
                tiles.as_ptr() as *const _,
                0,
                0,
            );
        }
        Ok(())
    }

    fn update_params(
        &self,
        atlas_width: u32,
        atlas_height: u32,
        desktop_width: u32,
        desktop_height: u32,
        tile_count: u32,
        output_capacity: u32,
        reference_enabled: bool,
        delta_exact_enabled: bool,
    ) {
        let params = TileCommandParams {
            atlas_width,
            atlas_height,
            desktop_width,
            desktop_height,
            tile_count,
            output_capacity,
            reference_enabled: u32::from(reference_enabled),
            delta_exact_enabled: u32::from(delta_exact_enabled),
            desktop_tiles_x: self.exact_tiles_x,
            desktop_tiles_y: self.exact_tiles_y,
            _pad0: 0,
            _pad1: 0,
        };
        unsafe {
            self.context.UpdateSubresource(
                &self.params_buffer,
                0,
                None,
                &params as *const _ as *const _,
                0,
                0,
            );
        }
    }

    fn dispatch_init(&self) {
        let output_uav = self.output_uav.as_ref().expect("output UAV exists");
        unsafe {
            let uavs = [Some(output_uav.clone())];
            let initial_counts = [0u32];
            self.context.CSSetShader(&self.init_shader, None);
            self.context
                .CSSetConstantBuffers(0, Some(&[Some(self.params_buffer.clone())]));
            self.context.CSSetUnorderedAccessViews(
                0,
                1,
                Some(uavs.as_ptr()),
                Some(initial_counts.as_ptr()),
            );
            self.context.Dispatch(1, 1, 1);
            self.clear_compute_bindings();
        }
    }

    fn dispatch_classify(&self, descriptor_count: u32) {
        let source_srv = self.source_srv.as_ref().expect("source SRV exists");
        let previous_srv = self.previous_srv.as_ref().expect("previous SRV exists");
        let tile_map_srv = self.tile_map_srv.as_ref().expect("tile map SRV exists");
        let exact_map_srv = self.exact_map_srv.as_ref().expect("exact map SRV exists");
        let analysis_uav = self.analysis_uav.as_ref().expect("analysis UAV exists");
        unsafe {
            let srvs = [
                Some(source_srv.clone()),
                Some(previous_srv.clone()),
                Some(tile_map_srv.clone()),
                Some(exact_map_srv.clone()),
            ];
            let uavs = [Some(analysis_uav.clone())];
            let initial_counts = [0u32];
            self.context.CSSetShader(&self.classify_shader, None);
            self.context
                .CSSetConstantBuffers(0, Some(&[Some(self.params_buffer.clone())]));
            self.context.CSSetShaderResources(0, Some(&srvs));
            self.context.CSSetUnorderedAccessViews(
                0,
                1,
                Some(uavs.as_ptr()),
                Some(initial_counts.as_ptr()),
            );
            self.context.Dispatch(descriptor_count.max(1), 1, 1);
            self.clear_compute_bindings();
        }
    }

    fn dispatch_emit(&self, descriptor_count: u32) {
        let source_srv = self.source_srv.as_ref().expect("source SRV exists");
        let previous_srv = self.previous_srv.as_ref().expect("previous SRV exists");
        let tile_map_srv = self.tile_map_srv.as_ref().expect("tile map SRV exists");
        let analysis_srv = self.analysis_srv.as_ref().expect("analysis SRV exists");
        let output_uav = self.output_uav.as_ref().expect("output UAV exists");
        unsafe {
            let srvs = [
                Some(source_srv.clone()),
                Some(previous_srv.clone()),
                Some(tile_map_srv.clone()),
                Some(analysis_srv.clone()),
            ];
            let uavs = [Some(output_uav.clone())];
            let initial_counts = [0u32];
            self.context.CSSetShader(&self.emit_shader, None);
            self.context
                .CSSetConstantBuffers(0, Some(&[Some(self.params_buffer.clone())]));
            self.context.CSSetShaderResources(0, Some(&srvs));
            self.context.CSSetUnorderedAccessViews(
                0,
                1,
                Some(uavs.as_ptr()),
                Some(initial_counts.as_ptr()),
            );
            self.context.Dispatch(descriptor_count.max(1), 1, 1);
            self.clear_compute_bindings();
        }
    }

    fn clear_compute_bindings(&self) {
        unsafe {
            let null_srvs = [None, None, None, None];
            let null_uavs = [None];
            let null_cbuffers = [None];
            self.context.CSSetShaderResources(0, Some(&null_srvs));
            self.context
                .CSSetUnorderedAccessViews(0, 1, Some(null_uavs.as_ptr()), None);
            self.context.CSSetConstantBuffers(0, Some(&null_cbuffers));
            self.context.CSSetShader(None, None);
        }
    }

    fn read_stream(
        &self,
        atlas_width: u32,
        atlas_height: u32,
        descriptor_count: u32,
        raw_equivalent_bytes: u32,
        timings: &mut TileCommandEncodeTimings,
    ) -> Result<TileCommandStream> {
        let output_buffer = self
            .output_buffer
            .as_ref()
            .context("tile command output buffer unavailable")?;
        let staging_buffer = self
            .staging_buffer
            .as_ref()
            .context("tile command staging buffer unavailable")?;
        let stage_started = Instant::now();
        unsafe {
            self.context.CopyResource(staging_buffer, output_buffer);
        }
        timings.staging_copy = stage_started.elapsed();

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let stage_started = Instant::now();
        unsafe {
            self.context
                .Map(staging_buffer, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|err| anyhow!("tile command staging buffer map failed: {err}"))?;
        }
        timings.map_wait = stage_started.elapsed();
        let stage_started = Instant::now();
        let result = read_mapped_stream(
            &mapped,
            self.output_capacity,
            atlas_width,
            atlas_height,
            descriptor_count,
            raw_equivalent_bytes,
        );
        timings.result_parse = stage_started.elapsed();
        unsafe {
            self.context.Unmap(staging_buffer, 0);
        }
        result
    }
}

pub(crate) fn reconstruct_atlas_from_stream(
    stream: &TileCommandStream,
    previous_desktop: Option<&[u8]>,
    desktop_width: u32,
    desktop_height: u32,
) -> Result<Vec<u8>> {
    validate_stream_header(stream)?;
    let expected = stream.atlas_width as usize * stream.atlas_height as usize * 4;
    let mut reconstructed = vec![0u8; expected];
    let atlas_stride = stream.atlas_width as usize * 4;
    let mut offset = TILE_COMMAND_STREAM_HEADER_BYTES as usize;
    let end = stream.byte_len as usize;
    while offset < end {
        anyhow::ensure!(
            offset + TILE_COMMAND_HEADER_BYTES as usize <= end,
            "tile command stream has truncated command header"
        );
        let kind = read_u32(&stream.bytes, offset)?;
        let atlas_xy = read_u32(&stream.bytes, offset + 4)?;
        let desktop_xy = read_u32(&stream.bytes, offset + 8)?;
        let wh = read_u32(&stream.bytes, offset + 12)?;
        let payload_len = read_u32(&stream.bytes, offset + 16)? as usize;
        let changed_count = read_u32(&stream.bytes, offset + 20)? as usize;
        let atlas_x = atlas_xy & 0xffff;
        let atlas_y = atlas_xy >> 16;
        let desktop_x = desktop_xy & 0xffff;
        let desktop_y = desktop_xy >> 16;
        let width = wh & 0xffff;
        let height = wh >> 16;
        let payload_offset = offset + TILE_COMMAND_HEADER_BYTES as usize;
        let payload_end = payload_offset
            .checked_add(payload_len)
            .context("tile command payload offset overflow")?;
        anyhow::ensure!(
            payload_end <= end,
            "tile command stream has truncated payload"
        );
        validate_rect(
            atlas_x,
            atlas_y,
            width,
            height,
            stream.atlas_width,
            stream.atlas_height,
        )?;
        match kind {
            TILE_COMMAND_SOLID_COLOR => {
                let color = read_u32(&stream.bytes, payload_offset)?.to_le_bytes();
                fill_rect(
                    &mut reconstructed,
                    atlas_stride,
                    atlas_x,
                    atlas_y,
                    width,
                    height,
                    color,
                );
            }
            TILE_COMMAND_RAW_BGRA => decode_raw_payload(
                &mut reconstructed,
                atlas_stride,
                atlas_x,
                atlas_y,
                width,
                height,
                &stream.bytes[payload_offset..payload_end],
            )?,
            TILE_COMMAND_XOR_RAW => decode_xor_raw_payload(
                &mut reconstructed,
                atlas_stride,
                atlas_x,
                atlas_y,
                desktop_x,
                desktop_y,
                width,
                height,
                &stream.bytes[payload_offset..payload_end],
                previous_desktop,
                desktop_width,
                desktop_height,
            )?,
            TILE_COMMAND_XOR_SPARSE => decode_xor_sparse_payload(
                &mut reconstructed,
                atlas_stride,
                atlas_x,
                atlas_y,
                desktop_x,
                desktop_y,
                width,
                height,
                &stream.bytes[payload_offset..payload_end],
                previous_desktop,
                desktop_width,
                desktop_height,
            )?,
            TILE_COMMAND_MASKED_QUANT_DELTA => decode_masked_quant_delta_payload(
                &mut reconstructed,
                atlas_stride,
                atlas_x,
                atlas_y,
                desktop_x,
                desktop_y,
                width,
                height,
                &stream.bytes[payload_offset..payload_end],
                changed_count,
                previous_desktop,
                desktop_width,
                desktop_height,
            )?,
            TILE_COMMAND_LOSSY_UI_BLOCK => decode_lossy_ui_block_payload(
                &mut reconstructed,
                atlas_stride,
                atlas_x,
                atlas_y,
                width,
                height,
                &stream.bytes[payload_offset..payload_end],
            )?,
            TILE_COMMAND_SHARP_UI_BLOCK => decode_sharp_ui_block_payload(
                &mut reconstructed,
                atlas_stride,
                atlas_x,
                atlas_y,
                width,
                height,
                &stream.bytes[payload_offset..payload_end],
            )?,
            other => anyhow::bail!("unknown tile command kind {other}"),
        }
        offset = payload_end;
    }
    Ok(reconstructed)
}

fn validate_stream_header(stream: &TileCommandStream) -> Result<()> {
    anyhow::ensure!(
        stream.bytes.len() >= TILE_COMMAND_STREAM_HEADER_BYTES as usize,
        "tile command stream is shorter than header"
    );
    anyhow::ensure!(
        read_u32(&stream.bytes, 0)? == TILE_COMMAND_STREAM_MAGIC,
        "bad ATX2 tile command stream magic"
    );
    anyhow::ensure!(
        read_u32(&stream.bytes, 4)? == TILE_COMMAND_STREAM_VERSION,
        "unsupported ATX2 tile command stream version"
    );
    anyhow::ensure!(
        read_u32(&stream.bytes, 20)? == stream.byte_len,
        "tile command stream byte length header mismatch"
    );
    Ok(())
}

fn read_mapped_stream(
    mapped: &D3D11_MAPPED_SUBRESOURCE,
    output_capacity: u32,
    atlas_width: u32,
    atlas_height: u32,
    descriptor_count: u32,
    raw_equivalent_bytes: u32,
) -> Result<TileCommandStream> {
    let all_bytes =
        unsafe { slice::from_raw_parts(mapped.pData as *const u8, output_capacity as usize) };
    anyhow::ensure!(
        all_bytes.len() >= TILE_COMMAND_STREAM_HEADER_BYTES as usize,
        "tile command stream staging buffer is too small"
    );
    let byte_len = read_u32(all_bytes, 20)?;
    anyhow::ensure!(
        byte_len >= TILE_COMMAND_STREAM_HEADER_BYTES && byte_len <= output_capacity,
        "invalid tile command stream byte length {byte_len} for capacity {output_capacity}"
    );
    let original_command_count = read_u32(all_bytes, 16)?;
    let bytes = merge_solid_color_row_bands(
        &all_bytes[..byte_len as usize],
        atlas_width,
        atlas_height,
        descriptor_count,
    )?;
    let byte_len = u32::try_from(bytes.len()).context("merged tile command stream too large")?;
    let command_count = read_u32(&bytes, 16)?;
    let (mut command_counts, copy_rects) = parse_stream_commands(&bytes)?;
    command_counts.skipped = descriptor_count
        .saturating_sub(original_command_count)
        .try_into()
        .unwrap_or(usize::MAX);
    Ok(TileCommandStream {
        atlas_width,
        atlas_height,
        descriptor_count,
        command_count,
        byte_len,
        raw_equivalent_bytes,
        delta_bytes_saved_estimate: raw_equivalent_bytes.saturating_sub(byte_len),
        copy_rects,
        bytes,
        command_counts,
        timings: TileCommandEncodeTimings::default(),
    })
}

fn parse_owned_commands(bytes: &[u8]) -> Result<Vec<OwnedCommand>> {
    validate_basic_stream_bytes(bytes)?;
    let mut commands = Vec::new();
    let mut offset = TILE_COMMAND_STREAM_HEADER_BYTES as usize;
    while offset < bytes.len() {
        anyhow::ensure!(
            offset + TILE_COMMAND_HEADER_BYTES as usize <= bytes.len(),
            "tile command stream has truncated command header"
        );
        let kind = read_u32(bytes, offset)?;
        let atlas_xy = read_u32(bytes, offset + 4)?;
        let desktop_xy = read_u32(bytes, offset + 8)?;
        let wh = read_u32(bytes, offset + 12)?;
        let payload_len = read_u32(bytes, offset + 16)? as usize;
        let changed_count = read_u32(bytes, offset + 20)?;
        let payload_offset = offset + TILE_COMMAND_HEADER_BYTES as usize;
        let payload_end = payload_offset
            .checked_add(payload_len)
            .context("tile command payload offset overflow")?;
        anyhow::ensure!(
            payload_end <= bytes.len(),
            "tile command stream has truncated payload"
        );
        commands.push(OwnedCommand {
            kind,
            atlas_xy,
            desktop_xy,
            wh,
            changed_count,
            payload: bytes[payload_offset..payload_end].to_vec(),
        });
        offset = payload_end;
    }
    Ok(commands)
}

fn validate_basic_stream_bytes(bytes: &[u8]) -> Result<()> {
    anyhow::ensure!(
        bytes.len() >= TILE_COMMAND_STREAM_HEADER_BYTES as usize,
        "tile command stream is shorter than header"
    );
    anyhow::ensure!(
        bytes.len().is_multiple_of(4),
        "tile command stream is not 4-byte aligned"
    );
    anyhow::ensure!(
        read_u32(bytes, 0)? == TILE_COMMAND_STREAM_MAGIC,
        "bad ATX2 tile command stream magic"
    );
    anyhow::ensure!(
        read_u32(bytes, 4)? == TILE_COMMAND_STREAM_VERSION,
        "unsupported ATX2 tile command stream version"
    );
    anyhow::ensure!(
        read_u32(bytes, 20)? as usize == bytes.len(),
        "tile command stream byte length header mismatch"
    );
    Ok(())
}

fn parse_command_refs(bytes: &[u8]) -> Result<Vec<CommandRef>> {
    validate_basic_stream_bytes(bytes)?;
    let mut commands = Vec::new();
    let mut offset = TILE_COMMAND_STREAM_HEADER_BYTES as usize;
    while offset < bytes.len() {
        anyhow::ensure!(
            offset + TILE_COMMAND_HEADER_BYTES as usize <= bytes.len(),
            "tile command stream has truncated command header"
        );
        let kind = read_u32(bytes, offset)?;
        let atlas_xy = read_u32(bytes, offset + 4)?;
        let desktop_xy = read_u32(bytes, offset + 8)?;
        let wh = read_u32(bytes, offset + 12)?;
        let payload_len = read_u32(bytes, offset + 16)? as usize;
        let changed_count = read_u32(bytes, offset + 20)?;
        let payload_offset = offset + TILE_COMMAND_HEADER_BYTES as usize;
        let payload_end = payload_offset
            .checked_add(payload_len)
            .context("tile command payload offset overflow")?;
        anyhow::ensure!(
            payload_end <= bytes.len(),
            "tile command stream has truncated payload"
        );
        commands.push(CommandRef {
            kind,
            atlas_xy,
            desktop_xy,
            wh,
            payload_len,
            changed_count,
            command_offset: offset,
            payload_offset,
        });
        offset = payload_end;
    }
    Ok(commands)
}

fn solid_color_key(bytes: &[u8], command: CommandRef) -> u32 {
    if command.kind == TILE_COMMAND_SOLID_COLOR && command.payload_len == 4 {
        u32::from_le_bytes(
            bytes[command.payload_offset..command.payload_offset + 4]
                .try_into()
                .expect("solid color payload has already been validated"),
        )
    } else {
        0
    }
}

fn merge_solid_color_row_bands(
    bytes: &[u8],
    atlas_width: u32,
    atlas_height: u32,
    descriptor_count: u32,
) -> Result<Vec<u8>> {
    let mut commands = parse_command_refs(bytes)?;
    if commands.len() < 2 {
        return Ok(bytes.to_vec());
    }
    let solid_count = commands
        .iter()
        .filter(|command| command.kind == TILE_COMMAND_SOLID_COLOR && command.payload_len == 4)
        .count();
    if solid_count < 2 {
        return Ok(bytes.to_vec());
    }

    commands.sort_by_key(|command| {
        (
            command.atlas_y(),
            command.desktop_y(),
            command.height(),
            command.kind,
            solid_color_key(bytes, *command),
            command.atlas_x(),
            command.desktop_x(),
        )
    });

    let mut output = vec![0u8; TILE_COMMAND_STREAM_HEADER_BYTES as usize];
    let mut command_count = 0u32;
    let mut index = 0usize;
    while index < commands.len() {
        let command = commands[index];
        if command.kind != TILE_COMMAND_SOLID_COLOR || command.payload_len != 4 {
            output.extend_from_slice(&bytes[command.command_offset..command.payload_end()]);
            command_count = command_count.saturating_add(1);
            index += 1;
            continue;
        }

        let color = solid_color_key(bytes, command);
        let mut width = command.width();
        let mut changed_count = command.changed_count;
        index += 1;
        while index < commands.len() {
            let next = commands[index];
            if next.kind == TILE_COMMAND_SOLID_COLOR
                && next.payload_len == 4
                && solid_color_key(bytes, next) == color
                && next.atlas_y() == command.atlas_y()
                && next.desktop_y() == command.desktop_y()
                && next.height() == command.height()
                && next.atlas_x() == command.atlas_x().saturating_add(width)
                && next.desktop_x() == command.desktop_x().saturating_add(width)
                && width
                    .checked_add(next.width())
                    .is_some_and(|merged_width| merged_width <= u16::MAX as u32)
            {
                width += next.width();
                changed_count = changed_count.saturating_add(next.changed_count);
                index += 1;
            } else {
                break;
            }
        }

        let command_offset = output.len();
        output.resize(command_offset + TILE_COMMAND_HEADER_BYTES as usize + 4, 0);
        write_command_header(
            &mut output,
            command_offset,
            TILE_COMMAND_SOLID_COLOR,
            command.atlas_xy,
            command.desktop_xy,
            pack_xy(width, command.height()),
            4,
            changed_count,
        );
        write_u32(
            &mut output,
            command_offset + TILE_COMMAND_HEADER_BYTES as usize,
            color,
        );
        command_count = command_count.saturating_add(1);
    }

    anyhow::ensure!(
        output.len() <= u32::MAX as usize,
        "merged tile command stream too large"
    );
    let byte_len = output.len() as u32;
    write_stream_header(
        &mut output,
        atlas_width,
        atlas_height,
        atlas_width.div_ceil(TILE_SIZE).max(1),
        command_count,
        byte_len,
        descriptor_count,
        0,
    );
    Ok(output)
}

fn build_command_stream_from_commands(
    atlas_width: u32,
    atlas_height: u32,
    descriptor_count: u32,
    commands: &[OwnedCommand],
) -> Result<Vec<u8>> {
    let byte_len =
        commands
            .iter()
            .try_fold(TILE_COMMAND_STREAM_HEADER_BYTES as usize, |sum, command| {
                sum.checked_add(command.encoded_len())
                    .context("tile command stream byte length overflow")
            })?;
    anyhow::ensure!(
        byte_len <= u32::MAX as usize,
        "tile command stream too large"
    );
    let mut bytes = vec![0u8; byte_len];
    write_stream_header(
        &mut bytes,
        atlas_width,
        atlas_height,
        atlas_width.div_ceil(TILE_SIZE).max(1),
        commands.len() as u32,
        byte_len as u32,
        descriptor_count,
        0,
    );
    let mut offset = TILE_COMMAND_STREAM_HEADER_BYTES as usize;
    for command in commands {
        write_command_header(
            &mut bytes,
            offset,
            command.kind,
            command.atlas_xy,
            command.desktop_xy,
            command.wh,
            command.payload.len() as u32,
            command.changed_count,
        );
        let payload_offset = offset + TILE_COMMAND_HEADER_BYTES as usize;
        bytes[payload_offset..payload_offset + command.payload.len()]
            .copy_from_slice(&command.payload);
        offset = payload_offset + command.payload.len();
    }
    Ok(bytes)
}

fn build_wire_chunk(
    atlas_width: u32,
    atlas_height: u32,
    commands: Vec<OwnedCommand>,
) -> Result<TileCommandWireChunk> {
    let command_count = commands.len() as u32;
    let copy_rects = commands.iter().map(OwnedCommand::copy_rect).collect();
    let bytes =
        build_command_stream_from_commands(atlas_width, atlas_height, command_count, &commands)?;
    Ok(TileCommandWireChunk { bytes, copy_rects })
}

fn parse_stream_commands(bytes: &[u8]) -> Result<(TileCommandCounts, Vec<TileCommandCopyRect>)> {
    let mut counts = TileCommandCounts::default();
    let mut copy_rects = Vec::new();
    let mut offset = TILE_COMMAND_STREAM_HEADER_BYTES as usize;
    while offset < bytes.len() {
        anyhow::ensure!(
            offset + TILE_COMMAND_HEADER_BYTES as usize <= bytes.len(),
            "tile command stream has truncated command header while counting"
        );
        let kind = read_u32(bytes, offset)?;
        let atlas_xy = read_u32(bytes, offset + 4)?;
        let desktop_xy = read_u32(bytes, offset + 8)?;
        let wh = read_u32(bytes, offset + 12)?;
        let payload_len = read_u32(bytes, offset + 16)? as usize;
        match kind {
            TILE_COMMAND_SOLID_COLOR => counts.solid += 1,
            TILE_COMMAND_RAW_BGRA => counts.raw_key += 1,
            TILE_COMMAND_XOR_RAW => counts.xor_raw += 1,
            TILE_COMMAND_XOR_SPARSE => counts.xor_sparse += 1,
            TILE_COMMAND_MASKED_QUANT_DELTA => counts.masked_quant_delta += 1,
            TILE_COMMAND_LOSSY_UI_BLOCK => counts.lossy_ui_block += 1,
            TILE_COMMAND_SHARP_UI_BLOCK => counts.sharp_ui_block += 1,
            other => anyhow::bail!("unknown tile command kind {other} while counting"),
        }
        copy_rects.push(TileCommandCopyRect {
            atlas_x: atlas_xy & 0xffff,
            atlas_y: atlas_xy >> 16,
            desktop_x: desktop_xy & 0xffff,
            desktop_y: desktop_xy >> 16,
            width: wh & 0xffff,
            height: wh >> 16,
        });
        offset = offset
            .checked_add(TILE_COMMAND_HEADER_BYTES as usize)
            .and_then(|offset| offset.checked_add(payload_len))
            .context("tile command stream count offset overflow")?;
    }
    anyhow::ensure!(
        offset == bytes.len(),
        "tile command stream count ended past byte length"
    );
    Ok((counts, copy_rects))
}

fn command_kind_is_exact(kind: u32) -> bool {
    matches!(
        kind,
        TILE_COMMAND_SOLID_COLOR
            | TILE_COMMAND_RAW_BGRA
            | TILE_COMMAND_XOR_RAW
            | TILE_COMMAND_XOR_SPARSE
            | TILE_COMMAND_MASKED_QUANT_DELTA
    )
}

fn decode_raw_payload(
    atlas: &mut [u8],
    atlas_stride: usize,
    atlas_x: u32,
    atlas_y: u32,
    width: u32,
    height: u32,
    payload: &[u8],
) -> Result<()> {
    let row_bytes = width as usize * 4;
    let expected = row_bytes * height as usize;
    anyhow::ensure!(
        payload.len() == expected,
        "raw payload length mismatch: expected {expected}, got {}",
        payload.len()
    );
    for row in 0..height as usize {
        let src = row * row_bytes;
        let dst = ((atlas_y as usize + row) * atlas_stride) + atlas_x as usize * 4;
        atlas[dst..dst + row_bytes].copy_from_slice(&payload[src..src + row_bytes]);
    }
    Ok(())
}

fn decode_xor_raw_payload(
    atlas: &mut [u8],
    atlas_stride: usize,
    atlas_x: u32,
    atlas_y: u32,
    desktop_x: u32,
    desktop_y: u32,
    width: u32,
    height: u32,
    payload: &[u8],
    previous_desktop: Option<&[u8]>,
    desktop_width: u32,
    desktop_height: u32,
) -> Result<()> {
    let previous = previous_desktop.context("XOR raw command requires previous desktop")?;
    validate_rect(
        desktop_x,
        desktop_y,
        width,
        height,
        desktop_width,
        desktop_height,
    )?;
    let pixel_count = width as usize * height as usize;
    anyhow::ensure!(
        payload.len() == pixel_count * 4,
        "XOR raw payload length mismatch"
    );
    for pixel in 0..pixel_count {
        let previous_pixel =
            read_previous_pixel(previous, desktop_width, desktop_x, desktop_y, width, pixel)?;
        let xor_pixel = read_u32(payload, pixel * 4)?;
        let color = (previous_pixel ^ xor_pixel).to_le_bytes();
        write_pixel(atlas, atlas_stride, atlas_x, atlas_y, width, pixel, color);
    }
    Ok(())
}

fn decode_xor_sparse_payload(
    atlas: &mut [u8],
    atlas_stride: usize,
    atlas_x: u32,
    atlas_y: u32,
    desktop_x: u32,
    desktop_y: u32,
    width: u32,
    height: u32,
    payload: &[u8],
    previous_desktop: Option<&[u8]>,
    desktop_width: u32,
    desktop_height: u32,
) -> Result<()> {
    let previous = previous_desktop.context("XOR sparse command requires previous desktop")?;
    validate_rect(
        desktop_x,
        desktop_y,
        width,
        height,
        desktop_width,
        desktop_height,
    )?;
    anyhow::ensure!(
        payload.len().is_multiple_of(8),
        "XOR sparse payload length is not 8-byte aligned"
    );
    let pixel_count = width as usize * height as usize;
    for pixel in 0..pixel_count {
        let color =
            read_previous_pixel(previous, desktop_width, desktop_x, desktop_y, width, pixel)?
                .to_le_bytes();
        write_pixel(atlas, atlas_stride, atlas_x, atlas_y, width, pixel, color);
    }
    for entry_offset in (0..payload.len()).step_by(8) {
        let pixel = read_u32(payload, entry_offset)? as usize;
        anyhow::ensure!(pixel < pixel_count, "XOR sparse pixel index exceeds tile");
        let previous_pixel =
            read_previous_pixel(previous, desktop_width, desktop_x, desktop_y, width, pixel)?;
        let xor_pixel = read_u32(payload, entry_offset + 4)?;
        let color = (previous_pixel ^ xor_pixel).to_le_bytes();
        write_pixel(atlas, atlas_stride, atlas_x, atlas_y, width, pixel, color);
    }
    Ok(())
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
) -> Result<()> {
    let previous =
        previous_desktop.context("masked quant delta command requires previous desktop")?;
    validate_rect(
        desktop_x,
        desktop_y,
        width,
        height,
        desktop_width,
        desktop_height,
    )?;
    let pixel_count = width as usize * height as usize;
    anyhow::ensure!(
        changed_count <= pixel_count,
        "masked quant delta changed count exceeds tile"
    );
    let mask_words = pixel_count.div_ceil(32);
    let mask_bytes = mask_words
        .checked_mul(4)
        .context("masked quant delta mask byte length overflow")?;
    let residual_bytes = pixel_count
        .checked_mul(2)
        .context("masked quant delta residual byte length overflow")?;
    let expected = align_usize_to_4(
        4usize
            .checked_add(mask_bytes)
            .and_then(|value| value.checked_add(residual_bytes))
            .context("masked quant delta payload length overflow")?,
    );
    anyhow::ensure!(
        payload.len() == expected,
        "masked quant delta payload length mismatch: expected {expected}, got {}",
        payload.len()
    );
    let quant_shift = read_u32(payload, 0)? & 0xff;
    anyhow::ensure!(quant_shift <= 4, "masked quant delta shift is invalid");
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
    anyhow::ensure!(
        seen_changed == changed_count,
        "masked quant delta changed mask count mismatch"
    );
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
) -> Result<()> {
    let pixel_count = width as usize * height as usize;
    let chroma_width = (width as usize).div_ceil(4);
    let chroma_height = (height as usize).div_ceil(4);
    let chroma_count = chroma_width
        .checked_mul(chroma_height)
        .context("lossy UI block chroma count overflow")?;
    let y_bytes = align_usize_to_4(pixel_count.div_ceil(2));
    let chroma_bytes = align_usize_to_4(chroma_count);
    let expected = 4usize
        .checked_add(y_bytes)
        .and_then(|value| value.checked_add(chroma_bytes))
        .context("lossy UI block payload length overflow")?;
    anyhow::ensure!(
        payload.len() == expected,
        "lossy UI block payload length mismatch: expected {expected}, got {}",
        payload.len()
    );
    let header = read_u32(payload, 0)?;
    anyhow::ensure!(
        (header & 0xffff) as usize == chroma_width && (header >> 16) as usize == chroma_height,
        "lossy UI block chroma dimensions mismatch"
    );
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
        let color = ycocg_to_bgra(y, co, cg);
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
) -> Result<()> {
    let pixel_count = width as usize * height as usize;
    let expected = align_usize_to_4(
        pixel_count
            .checked_mul(2)
            .context("sharp UI block payload length overflow")?,
    );
    anyhow::ensure!(
        payload.len() == expected,
        "sharp UI block payload length mismatch: expected {expected}, got {}",
        payload.len()
    );
    for pixel in 0..pixel_count {
        let word = read_u32(payload, (pixel / 2) * 4)?;
        let packed = if pixel & 1 == 0 {
            word & 0xffff
        } else {
            word >> 16
        };
        let color = rgb565_to_bgra(packed);
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
    let b = clamp_u8(previous_b + delta_b) as u32;
    let g = clamp_u8(previous_g + delta_g) as u32;
    let r = clamp_u8(previous_r + delta_r) as u32;
    b | (g << 8) | (r << 16) | alpha
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
    u32::from(clamp_u8(b))
        | (u32::from(clamp_u8(g)) << 8)
        | (u32::from(clamp_u8(r)) << 16)
        | 0xff00_0000
}

fn sign_extend(value: u32, bits: u32) -> i32 {
    let sign_bit = 1u32 << (bits - 1);
    if value & sign_bit == 0 {
        value as i32
    } else {
        value as i32 - (1i32 << bits)
    }
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

fn read_previous_pixel(
    previous: &[u8],
    desktop_width: u32,
    desktop_x: u32,
    desktop_y: u32,
    tile_width: u32,
    pixel: usize,
) -> Result<u32> {
    let row = pixel / tile_width as usize;
    let col = pixel % tile_width as usize;
    let offset = ((desktop_y as usize + row) * desktop_width as usize + desktop_x as usize + col)
        .checked_mul(4)
        .context("previous desktop pixel offset overflow")?;
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
) -> Result<()> {
    anyhow::ensure!(width > 0 && height > 0, "tile command has empty rect");
    anyhow::ensure!(
        x.checked_add(width)
            .is_some_and(|right| right <= limit_width)
            && y.checked_add(height)
                .is_some_and(|bottom| bottom <= limit_height),
        "tile command rect exceeds bounds"
    );
    Ok(())
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset.checked_add(4).context("u32 read offset overflow")?;
    let value = bytes
        .get(offset..end)
        .context("u32 read exceeds byte stream")?;
    Ok(u32::from_le_bytes(
        value.try_into().expect("slice has length 4"),
    ))
}

fn align_usize_to_4(value: usize) -> usize {
    (value + 3) & !3
}

fn command_stream_capacity(tiles: &[TileCommandTile]) -> Result<u32> {
    raw_equivalent_stream_bytes(tiles)
}

fn raw_equivalent_stream_bytes(tiles: &[TileCommandTile]) -> Result<u32> {
    let mut capacity = u64::from(TILE_COMMAND_STREAM_HEADER_BYTES);
    for tile in tiles {
        let raw_payload = u64::from(tile.width()) * u64::from(tile.height()) * 4;
        capacity = capacity
            .checked_add(u64::from(TILE_COMMAND_HEADER_BYTES))
            .and_then(|value| value.checked_add(raw_payload))
            .context("tile command stream capacity overflow")?;
    }
    let aligned = (capacity + 3) & !3;
    anyhow::ensure!(
        aligned <= u64::from(u32::MAX),
        "tile command stream capacity exceeds u32"
    );
    Ok(aligned as u32)
}

fn build_empty_stream(atlas_width: u32, atlas_height: u32) -> Vec<u8> {
    let tiles_x = atlas_width.div_ceil(TILE_SIZE).max(1);
    let mut bytes = vec![0u8; TILE_COMMAND_STREAM_HEADER_BYTES as usize];
    write_stream_header(
        &mut bytes,
        atlas_width,
        atlas_height,
        tiles_x,
        0,
        TILE_COMMAND_STREAM_HEADER_BYTES,
        0,
        0,
    );
    bytes
}

fn write_stream_header(
    bytes: &mut [u8],
    atlas_width: u32,
    atlas_height: u32,
    tiles_x: u32,
    command_count: u32,
    byte_len: u32,
    descriptor_count: u32,
    overflow: u32,
) {
    write_u32(bytes, 0, TILE_COMMAND_STREAM_MAGIC);
    write_u32(bytes, 4, TILE_COMMAND_STREAM_VERSION);
    write_u32(bytes, 8, pack_xy(atlas_width, atlas_height));
    write_u32(bytes, 12, pack_xy(TILE_SIZE, tiles_x));
    write_u32(bytes, 16, command_count);
    write_u32(bytes, 20, byte_len);
    write_u32(bytes, 24, descriptor_count);
    write_u32(bytes, 28, overflow);
}

fn write_command_header(
    bytes: &mut [u8],
    offset: usize,
    kind: u32,
    atlas_xy: u32,
    desktop_xy: u32,
    wh: u32,
    payload_len: u32,
    changed_count: u32,
) {
    write_u32(bytes, offset, kind);
    write_u32(bytes, offset + 4, atlas_xy);
    write_u32(bytes, offset + 8, desktop_xy);
    write_u32(bytes, offset + 12, wh);
    write_u32(bytes, offset + 16, payload_len);
    write_u32(bytes, offset + 20, changed_count);
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn pack_xy(x: u32, y: u32) -> u32 {
    (x & 0xffff) | ((y & 0xffff) << 16)
}

fn create_raw_buffer(
    device: &ID3D11Device,
    byte_width: u32,
    usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE,
    bind_flags: u32,
    cpu_access_flags: u32,
    misc_flags: u32,
) -> Result<ID3D11Buffer> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: byte_width.max(4),
        Usage: usage,
        BindFlags: bind_flags,
        CPUAccessFlags: cpu_access_flags,
        MiscFlags: misc_flags,
        StructureByteStride: 0,
    };
    let mut buffer = None;
    unsafe {
        device
            .CreateBuffer(&desc, None, Some(&mut buffer))
            .map_err(|err| anyhow!("raw tile command buffer creation failed: {err}"))?;
    }
    buffer.context("raw tile command buffer creation returned null")
}

fn create_raw_buffer_srv(
    device: &ID3D11Device,
    buffer: &ID3D11Buffer,
    byte_width: u32,
) -> Result<ID3D11ShaderResourceView> {
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
            .map_err(|err| anyhow!("raw tile command SRV creation failed: {err}"))?;
    }
    view.context("raw tile command SRV creation returned null")
}

fn create_raw_buffer_uav(
    device: &ID3D11Device,
    buffer: &ID3D11Buffer,
    byte_width: u32,
) -> Result<ID3D11UnorderedAccessView> {
    let desc = D3D11_UNORDERED_ACCESS_VIEW_DESC {
        Format: DXGI_FORMAT_R32_TYPELESS,
        ViewDimension: D3D11_UAV_DIMENSION_BUFFER,
        Anonymous: windows::Win32::Graphics::Direct3D11::D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
            Buffer: D3D11_BUFFER_UAV {
                FirstElement: 0,
                NumElements: byte_width.div_ceil(4).max(1),
                Flags: D3D11_BUFFER_UAV_FLAG_RAW.0 as u32,
            },
        },
    };
    let mut uav = None;
    unsafe {
        device
            .CreateUnorderedAccessView(buffer, Some(&desc), Some(&mut uav))
            .map_err(|err| anyhow!("raw tile command UAV creation failed: {err}"))?;
    }
    uav.context("raw tile command UAV creation returned null")
}

fn create_params_buffer(device: &ID3D11Device) -> Result<ID3D11Buffer> {
    let byte_width = mem::size_of::<TileCommandParams>() as u32;
    let byte_width = (byte_width + 15) & !15;
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: byte_width,
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
            .map_err(|err| anyhow!("tile command params buffer creation failed: {err}"))?;
    }
    buffer.context("tile command params buffer creation returned null")
}

fn create_compute_shader(
    device: &ID3D11Device,
    source: &str,
    label: &str,
) -> Result<ID3D11ComputeShader> {
    let full_source = format!("{SHADER_COMMON}{source}");
    let blob = compile_shader(&full_source, label)?;
    let bytes = blob_bytes(&blob);
    let mut compute_shader = None;
    unsafe {
        device
            .CreateComputeShader(
                bytes,
                None::<&ID3D11ClassLinkage>,
                Some(&mut compute_shader),
            )
            .map_err(|err| anyhow!("tile command {label} shader creation failed: {err}"))?;
    }
    compute_shader.context("tile command shader creation returned null")
}

fn compile_shader(source: &str, label: &str) -> Result<ID3DBlob> {
    let mut code = None;
    let mut errors = None;
    let result = unsafe {
        D3DCompile(
            source.as_ptr() as *const _,
            source.len(),
            PCSTR::null(),
            None,
            None::<&ID3DInclude>,
            s!("cs_main"),
            s!("cs_5_0"),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    match result {
        Ok(()) => code.context("tile command shader compiler returned no bytecode"),
        Err(err) => {
            let compiler_output = errors
                .as_ref()
                .map(blob_to_string)
                .unwrap_or_else(|| "no compiler output".to_string());
            Err(anyhow!(
                "tile command {label} shader compile failed: {err}; {compiler_output}"
            ))
        }
    }
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe { slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) }
}

fn blob_to_string(blob: &ID3DBlob) -> String {
    let bytes = blob_bytes(blob);
    String::from_utf8_lossy(bytes).to_string()
}

const SHADER_COMMON: &str = r#"
cbuffer TileCommandParams : register(b0) {
    uint atlas_width;
    uint atlas_height;
    uint desktop_width;
    uint desktop_height;
    uint tile_count;
    uint output_capacity;
    uint reference_enabled;
    uint delta_exact_enabled;
    uint desktop_tiles_x;
    uint desktop_tiles_y;
    uint _pad0;
    uint _pad1;
};

static const uint TILE_SIZE = 32;
static const uint TILE_PIXELS = TILE_SIZE * TILE_SIZE;
static const uint STREAM_MAGIC = 0x32585441;
static const uint STREAM_VERSION = 4;
static const uint STREAM_HEADER_BYTES = 32;
static const uint COMMAND_HEADER_BYTES = 24;
static const uint COMMAND_SKIP = 0;
static const uint COMMAND_RAW_BGRA = 1;
static const uint COMMAND_SOLID_COLOR = 2;
static const uint COMMAND_XOR_RAW = 3;
static const uint COMMAND_XOR_SPARSE = 4;
static const uint COMMAND_MASKED_QUANT_DELTA = 5;
static const uint COMMAND_LOSSY_UI_BLOCK = 6;
static const uint COMMAND_SHARP_UI_BLOCK = 7;
static const uint TILE_MAP_ENTRY_BYTES = 16;
static const uint ANALYSIS_ENTRY_BYTES = 16;
static const uint MAX_SPARSE_CHANGED_PIXELS = 128;
static const uint LOSSY_UI_BLOCK_MIN_PIXELS = 64;
static const uint SHARP_UI_BLOCK_MIN_PIXELS = 64;
static const uint SHARP_UI_MIN_EDGES = 8;
static const uint SHARP_UI_MIN_LUMA_RANGE = 48;

uint bgra_from_float4(float4 pixel) {
    uint b = (uint)round(saturate(pixel.b) * 255.0);
    uint g = (uint)round(saturate(pixel.g) * 255.0);
    uint r = (uint)round(saturate(pixel.r) * 255.0);
    uint a = (uint)round(saturate(pixel.a) * 255.0);
    return b | (g << 8) | (r << 16) | (a << 24);
}

uint channel_delta(uint current, uint previous, uint shift) {
    uint c = (current >> shift) & 0xff;
    uint p = (previous >> shift) & 0xff;
    return c > p ? c - p : p - c;
}

uint max_color_delta(uint current, uint previous) {
    return max(channel_delta(current, previous, 0),
        max(channel_delta(current, previous, 8), channel_delta(current, previous, 16)));
}

uint align4(uint value) {
    return (value + 3) & ~3;
}

uint quant_shift_from_max_delta(uint max_delta) {
    uint shift = 0;
    uint positive_limit = 15;
    while (shift < 4 && max_delta > positive_limit) {
        shift += 1;
        positive_limit <<= 1;
    }
    return shift;
}

int quantize_delta_channel(uint current, uint previous, uint shift, int min_value, int max_value) {
    bool negative = current < previous;
    uint magnitude = negative ? previous - current : current - previous;
    uint half_step = shift == 0 ? 0 : (1u << (shift - 1));
    int quantized = int((magnitude + half_step) >> shift);
    if (negative) {
        quantized = -quantized;
    }
    return clamp(quantized, min_value, max_value);
}

uint pack_quant_delta(uint current, uint previous, uint quant_shift) {
    int qb = quantize_delta_channel(current & 0xff, previous & 0xff, quant_shift, -16, 15);
    int qg = quantize_delta_channel((current >> 8) & 0xff, (previous >> 8) & 0xff, quant_shift, -32, 31);
    int qr = quantize_delta_channel((current >> 16) & 0xff, (previous >> 16) & 0xff, quant_shift, -16, 15);
    return (uint(qb) & 0x1f) | ((uint(qg) & 0x3f) << 5) | ((uint(qr) & 0x1f) << 11);
}

uint lossy_ui_payload_len(uint width, uint height) {
    uint pixel_count = width * height;
    uint chroma_width = (width + 3) / 4;
    uint chroma_height = (height + 3) / 4;
    uint chroma_samples = chroma_width * chroma_height;
    uint y_bytes = align4((pixel_count + 1) / 2);
    uint chroma_bytes = align4(chroma_samples);
    return 4 + y_bytes + chroma_bytes;
}

uint sharp_ui_payload_len(uint width, uint height) {
    return align4(width * height * 2);
}

int ycocg_co(uint color) {
    int b = int(color & 0xff);
    int r = int((color >> 16) & 0xff);
    return r - b;
}

int ycocg_cg(uint color) {
    int b = int(color & 0xff);
    int g = int((color >> 8) & 0xff);
    int r = int((color >> 16) & 0xff);
    return g - ((r + b) >> 1);
}

uint ycocg_y(uint color) {
    uint b = color & 0xff;
    uint g = (color >> 8) & 0xff;
    uint r = (color >> 16) & 0xff;
    return (b + r + g * 2 + 2) >> 2;
}

uint pack_rgb565(uint color) {
    uint b = (color >> 3) & 0x1f;
    uint g = ((color >> 8) >> 2) & 0x3f;
    uint r = ((color >> 16) >> 3) & 0x1f;
    return b | (g << 5) | (r << 11);
}

uint quantize_lossy_luma(uint y) {
    return min(15, (y + 8) >> 4);
}

int quantize_lossy_chroma(int value) {
    return clamp(value >= 0 ? ((value + 16) >> 5) : -(((-value) + 16) >> 5), -8, 7);
}

uint pack_lossy_chroma_pair(int co, int cg) {
    int co_q = quantize_lossy_chroma(co);
    int cg_q = quantize_lossy_chroma(cg);
    return (uint(co_q) & 0x0f) | ((uint(cg_q) & 0x0f) << 4);
}

int average_lossy_chroma(int sum, uint count) {
    if (count == 0) {
        return 0;
    }
    if (count == 1) {
        return sum;
    }
    return sum / int(count);
}

"#;

const INIT_SHADER_SOURCE: &str = r#"
RWByteAddressBuffer outBuf : register(u0);

[numthreads(1, 1, 1)]
void cs_main(uint3 dispatch_id : SV_DispatchThreadID) {
    if (dispatch_id.x != 0 || dispatch_id.y != 0) {
        return;
    }
    uint tiles_x = max(1, (atlas_width + TILE_SIZE - 1) / TILE_SIZE);
    outBuf.Store(0, STREAM_MAGIC);
    outBuf.Store(4, STREAM_VERSION);
    outBuf.Store(8, (atlas_width & 0xffff) | ((atlas_height & 0xffff) << 16));
    outBuf.Store(12, (TILE_SIZE & 0xffff) | ((tiles_x & 0xffff) << 16));
    outBuf.Store(16, 0);
    outBuf.Store(20, STREAM_HEADER_BYTES);
    outBuf.Store(24, tile_count);
    outBuf.Store(28, 0);
}
"#;

const CLASSIFY_SHADER_SOURCE: &str = r#"
Texture2D<float4> sourceTex : register(t0);
Texture2D<float4> previousTex : register(t1);
ByteAddressBuffer tileMap : register(t2);
ByteAddressBuffer exactMap : register(t3);
RWByteAddressBuffer analysisBuf : register(u0);

groupshared uint g_pixels[TILE_PIXELS];
groupshared uint g_non_solid[TILE_PIXELS];
groupshared uint g_changed[TILE_PIXELS];
groupshared uint g_max_delta[TILE_PIXELS];
groupshared uint g_edge[TILE_PIXELS];
groupshared uint g_min_luma[TILE_PIXELS];
groupshared uint g_max_luma[TILE_PIXELS];
groupshared uint g_tile_exact;

bool previous_tile_is_exact(uint desktop_x, uint desktop_y, uint width, uint height) {
    if (reference_enabled == 0 || desktop_tiles_x == 0 || desktop_tiles_y == 0) {
        return false;
    }
    uint right = desktop_x + width - 1;
    uint bottom = desktop_y + height - 1;
    uint first_x = desktop_x / TILE_SIZE;
    uint last_x = min(right / TILE_SIZE, desktop_tiles_x - 1);
    uint first_y = desktop_y / TILE_SIZE;
    uint last_y = min(bottom / TILE_SIZE, desktop_tiles_y - 1);
    for (uint y = first_y; y <= last_y; y += 1) {
        for (uint x = first_x; x <= last_x; x += 1) {
            if (exactMap.Load((y * desktop_tiles_x + x) * 4) == 0) {
                return false;
            }
        }
    }
    return true;
}

[numthreads(32, 32, 1)]
void cs_main(uint3 group_id : SV_GroupID, uint3 group_thread_id : SV_GroupThreadID) {
    uint tile_index = group_id.x;
    if (tile_index >= tile_count) {
        return;
    }

    uint map_offset = tile_index * TILE_MAP_ENTRY_BYTES;
    uint atlas_xy = tileMap.Load(map_offset);
    uint desktop_xy = tileMap.Load(map_offset + 4);
    uint wh = tileMap.Load(map_offset + 8);
    uint flags = tileMap.Load(map_offset + 12);
    uint atlas_x = atlas_xy & 0xffff;
    uint atlas_y = atlas_xy >> 16;
    uint desktop_x = desktop_xy & 0xffff;
    uint desktop_y = desktop_xy >> 16;
    uint width = wh & 0xffff;
    uint height = wh >> 16;
    uint local_x = group_thread_id.x;
    uint local_y = group_thread_id.y;
    uint local_index = local_y * TILE_SIZE + local_x;
    bool active = (flags & 1) != 0 && local_x < width && local_y < height;
    if (local_index == 0) {
        g_tile_exact = previous_tile_is_exact(desktop_x, desktop_y, width, height) ? 1 : 0;
    }
    GroupMemoryBarrierWithGroupSync();

    uint pixel = 0;
    uint previous = 0;
    if (active) {
        pixel = bgra_from_float4(sourceTex.Load(int3(atlas_x + local_x, atlas_y + local_y, 0)));
        if (reference_enabled != 0) {
            previous = bgra_from_float4(previousTex.Load(int3(desktop_x + local_x, desktop_y + local_y, 0)));
        }
    }
    uint xor_pixel = pixel ^ previous;
    g_pixels[local_index] = pixel;
    GroupMemoryBarrierWithGroupSync();

    uint first = g_pixels[0];
    uint luma = active ? ycocg_y(pixel) : 0;
    uint edge = 0;
    if (active) {
        if (local_x > 0) {
            uint left = g_pixels[local_index - 1];
            uint left_luma = ycocg_y(left);
            uint delta = luma > left_luma ? luma - left_luma : left_luma - luma;
            edge = max(edge, delta >= 32 ? 1 : 0);
        }
        if (local_y > 0) {
            uint up = g_pixels[local_index - TILE_SIZE];
            uint up_luma = ycocg_y(up);
            uint delta = luma > up_luma ? luma - up_luma : up_luma - luma;
            edge = max(edge, delta >= 32 ? 1 : 0);
        }
    }
    g_non_solid[local_index] = (active && pixel != first) ? 1 : 0;
    g_changed[local_index] = active ? ((reference_enabled != 0) ? (xor_pixel != 0 ? 1 : 0) : 1) : 0;
    g_max_delta[local_index] = (active && g_tile_exact != 0 && xor_pixel != 0)
        ? max_color_delta(pixel, previous)
        : 0;
    g_edge[local_index] = edge;
    g_min_luma[local_index] = active ? luma : 255;
    g_max_luma[local_index] = active ? luma : 0;
    GroupMemoryBarrierWithGroupSync();

    for (uint stride = TILE_PIXELS / 2; stride > 0; stride >>= 1) {
        if (local_index < stride) {
            g_non_solid[local_index] += g_non_solid[local_index + stride];
            g_changed[local_index] += g_changed[local_index + stride];
            g_max_delta[local_index] = max(g_max_delta[local_index], g_max_delta[local_index + stride]);
            g_edge[local_index] += g_edge[local_index + stride];
            g_min_luma[local_index] = min(g_min_luma[local_index], g_min_luma[local_index + stride]);
            g_max_luma[local_index] = max(g_max_luma[local_index], g_max_luma[local_index + stride]);
        }
        GroupMemoryBarrierWithGroupSync();
    }

    if (local_index != 0) {
        return;
    }

    uint kind = COMMAND_SKIP;
    uint payload_len = 0;
    uint pixel_count = width * height;
    uint changed_count = g_changed[0];
    bool tile_exact = g_tile_exact != 0;
    uint luma_range = g_max_luma[0] > g_min_luma[0] ? g_max_luma[0] - g_min_luma[0] : 0;
    bool sharp_ui = pixel_count >= SHARP_UI_BLOCK_MIN_PIXELS
        && g_edge[0] >= max(SHARP_UI_MIN_EDGES, pixel_count / 64)
        && g_edge[0] <= (pixel_count * 3) / 4
        && luma_range >= SHARP_UI_MIN_LUMA_RANGE;
    if ((flags & 1) != 0 && pixel_count > 0 && changed_count > 0) {
        if (g_non_solid[0] == 0) {
            kind = COMMAND_SOLID_COLOR;
            payload_len = 4;
        } else if (tile_exact) {
            uint raw_len = pixel_count * 4;
            uint sparse_len = changed_count * 8;
            uint mask_words = (pixel_count + 31) / 32;
            uint quant_len = align4(4 + mask_words * 4 + pixel_count * 2);
            uint lossy_len = lossy_ui_payload_len(width, height);
            uint sharp_len = sharp_ui_payload_len(width, height);
            if (changed_count <= MAX_SPARSE_CHANGED_PIXELS && sparse_len < raw_len) {
                kind = COMMAND_XOR_SPARSE;
                payload_len = sparse_len;
            } else if (g_max_delta[0] <= 15 && quant_len < raw_len) {
                kind = COMMAND_MASKED_QUANT_DELTA;
                payload_len = quant_len;
            } else if (sharp_ui && sharp_len < raw_len) {
                kind = COMMAND_SHARP_UI_BLOCK;
                payload_len = sharp_len;
            } else if (pixel_count >= LOSSY_UI_BLOCK_MIN_PIXELS && lossy_len < raw_len) {
                kind = COMMAND_LOSSY_UI_BLOCK;
                payload_len = lossy_len;
            } else {
                kind = COMMAND_XOR_RAW;
                payload_len = raw_len;
            }
        } else if (reference_enabled != 0) {
            uint raw_len = pixel_count * 4;
            uint lossy_len = lossy_ui_payload_len(width, height);
            uint sharp_len = sharp_ui_payload_len(width, height);
            if (sharp_ui && sharp_len < raw_len) {
                kind = COMMAND_SHARP_UI_BLOCK;
                payload_len = sharp_len;
            } else if (pixel_count >= LOSSY_UI_BLOCK_MIN_PIXELS && lossy_len < raw_len) {
                kind = COMMAND_LOSSY_UI_BLOCK;
                payload_len = lossy_len;
            } else {
                kind = COMMAND_RAW_BGRA;
                payload_len = raw_len;
            }
        } else {
            uint raw_len = pixel_count * 4;
            uint lossy_len = lossy_ui_payload_len(width, height);
            if (sharp_ui) {
                kind = COMMAND_RAW_BGRA;
                payload_len = raw_len;
            } else if (pixel_count >= LOSSY_UI_BLOCK_MIN_PIXELS && lossy_len < raw_len) {
                kind = COMMAND_LOSSY_UI_BLOCK;
                payload_len = lossy_len;
            } else {
                kind = COMMAND_RAW_BGRA;
                payload_len = raw_len;
            }
        }
    }

    uint analysis_offset = tile_index * ANALYSIS_ENTRY_BYTES;
    analysisBuf.Store(analysis_offset, kind);
    analysisBuf.Store(analysis_offset + 4, payload_len);
    analysisBuf.Store(analysis_offset + 8, changed_count);
    analysisBuf.Store(analysis_offset + 12, quant_shift_from_max_delta(g_max_delta[0]));
}
"#;

const EMIT_SHADER_SOURCE: &str = r#"
Texture2D<float4> sourceTex : register(t0);
Texture2D<float4> previousTex : register(t1);
ByteAddressBuffer tileMap : register(t2);
ByteAddressBuffer analysisBuf : register(t3);
RWByteAddressBuffer outBuf : register(u0);

groupshared uint g_command_offset;
groupshared uint g_payload_offset;
groupshared uint g_kind;
groupshared uint g_width;
groupshared uint g_height;
groupshared uint g_atlas_xy;
groupshared uint g_desktop_xy;
groupshared uint g_atlas_x;
groupshared uint g_atlas_y;
groupshared uint g_desktop_x;
groupshared uint g_desktop_y;
groupshared uint g_active;
groupshared uint g_sparse_count;
groupshared uint g_payload_len;
groupshared uint g_quant_shift;

[numthreads(32, 32, 1)]
void cs_main(uint3 group_id : SV_GroupID, uint3 group_thread_id : SV_GroupThreadID) {
    uint tile_index = group_id.x;
    uint local_x = group_thread_id.x;
    uint local_y = group_thread_id.y;
    uint local_index = local_y * TILE_SIZE + local_x;
    if (tile_index >= tile_count) {
        return;
    }

    if (local_index == 0) {
        uint map_offset = tile_index * TILE_MAP_ENTRY_BYTES;
        g_atlas_xy = tileMap.Load(map_offset);
        g_desktop_xy = tileMap.Load(map_offset + 4);
        uint wh = tileMap.Load(map_offset + 8);
        uint analysis_offset = tile_index * ANALYSIS_ENTRY_BYTES;
        g_kind = analysisBuf.Load(analysis_offset);
        g_width = wh & 0xffff;
        g_height = wh >> 16;
        g_atlas_x = g_atlas_xy & 0xffff;
        g_atlas_y = g_atlas_xy >> 16;
        g_desktop_x = g_desktop_xy & 0xffff;
        g_desktop_y = g_desktop_xy >> 16;
        g_active = 0;
        g_sparse_count = 0;

        if (g_kind != COMMAND_SKIP && g_width > 0 && g_height > 0) {
            uint payload_len = analysisBuf.Load(analysis_offset + 4);
            uint changed_count = analysisBuf.Load(analysis_offset + 8);
            g_payload_len = payload_len;
            g_quant_shift = analysisBuf.Load(analysis_offset + 12);
            uint total_len = COMMAND_HEADER_BYTES + payload_len;
            uint command_offset;
            outBuf.InterlockedAdd(20, total_len, command_offset);
            g_command_offset = command_offset;
            if (g_command_offset + total_len <= output_capacity) {
                uint ignored;
                outBuf.InterlockedAdd(16, 1, ignored);
                outBuf.Store(g_command_offset, g_kind);
                outBuf.Store(g_command_offset + 4, g_atlas_xy);
                outBuf.Store(g_command_offset + 8, g_desktop_xy);
                outBuf.Store(g_command_offset + 12, (g_width & 0xffff) | ((g_height & 0xffff) << 16));
                outBuf.Store(g_command_offset + 16, payload_len);
                outBuf.Store(g_command_offset + 20, changed_count);
                g_payload_offset = g_command_offset + COMMAND_HEADER_BYTES;
                g_active = 1;
            } else {
                outBuf.Store(28, 1);
            }
        }
    }
    GroupMemoryBarrierWithGroupSync();

    if (g_active == 0) {
        return;
    }

    if (g_kind == COMMAND_SOLID_COLOR) {
        if (local_index == 0) {
            uint pixel = bgra_from_float4(sourceTex.Load(int3(g_atlas_x, g_atlas_y, 0)));
            outBuf.Store(g_payload_offset, pixel);
        }
        return;
    }

    if (g_kind == COMMAND_MASKED_QUANT_DELTA
        || g_kind == COMMAND_LOSSY_UI_BLOCK) {
        for (uint clear_word = local_index; clear_word < g_payload_len / 4; clear_word += TILE_PIXELS) {
            outBuf.Store(g_payload_offset + clear_word * 4, 0);
        }
        GroupMemoryBarrierWithGroupSync();
        if (local_index == 0 && g_kind == COMMAND_MASKED_QUANT_DELTA) {
            outBuf.Store(g_payload_offset, g_quant_shift);
        } else if (local_index == 0 && g_kind == COMMAND_LOSSY_UI_BLOCK) {
            uint chroma_width = (g_width + 3) / 4;
            uint chroma_height = (g_height + 3) / 4;
            outBuf.Store(g_payload_offset, (chroma_width & 0xffff) | ((chroma_height & 0xffff) << 16));
        }
        GroupMemoryBarrierWithGroupSync();
    }

    if (local_x >= g_width || local_y >= g_height) {
        return;
    }

    uint pixel_index = local_y * g_width + local_x;
    uint pixel = bgra_from_float4(sourceTex.Load(int3(g_atlas_x + local_x, g_atlas_y + local_y, 0)));

    if (g_kind == COMMAND_RAW_BGRA) {
        outBuf.Store(g_payload_offset + pixel_index * 4, pixel);
        return;
    }

    if (g_kind == COMMAND_SHARP_UI_BLOCK) {
        if ((pixel_index & 1) == 0) {
            uint next_pixel = 0;
            if (pixel_index + 1 < g_width * g_height) {
                uint next_x = (pixel_index + 1) % g_width;
                uint next_y = (pixel_index + 1) / g_width;
                next_pixel = bgra_from_float4(sourceTex.Load(int3(g_atlas_x + next_x, g_atlas_y + next_y, 0)));
            }
            uint packed = pack_rgb565(pixel) | (pack_rgb565(next_pixel) << 16);
            outBuf.Store(g_payload_offset + (pixel_index / 2) * 4, packed);
        }
        return;
    }

    if (g_kind == COMMAND_LOSSY_UI_BLOCK) {
        uint y_plane_offset = g_payload_offset + 4;
        uint y_word_offset = y_plane_offset + (pixel_index / 8) * 4;
        uint y_shift = (pixel_index & 7) * 4;
        uint ignored;
        outBuf.InterlockedOr(y_word_offset, quantize_lossy_luma(ycocg_y(pixel)) << y_shift, ignored);

        if ((local_x & 3) == 0 && (local_y & 3) == 0) {
            int co_sum = 0;
            int cg_sum = 0;
            uint sample_count = 0;
            for (uint oy = 0; oy < 4; oy += 1) {
                for (uint ox = 0; ox < 4; ox += 1) {
                    uint sx = local_x + ox;
                    uint sy = local_y + oy;
                    if (sx < g_width && sy < g_height) {
                        uint sample = bgra_from_float4(sourceTex.Load(int3(g_atlas_x + sx, g_atlas_y + sy, 0)));
                        co_sum += ycocg_co(sample);
                        cg_sum += ycocg_cg(sample);
                        sample_count += 1;
                    }
                }
            }
            int avg_co = average_lossy_chroma(co_sum, sample_count);
            int avg_cg = average_lossy_chroma(cg_sum, sample_count);
            uint chroma_width = (g_width + 3) / 4;
            uint chroma_index = (local_y / 4) * chroma_width + (local_x / 4);
            uint chroma_base = y_plane_offset + align4((g_width * g_height + 1) / 2);
            uint chroma_word_offset = chroma_base + (chroma_index / 4) * 4;
            uint chroma_shift = (chroma_index & 3) * 8;
            outBuf.InterlockedOr(chroma_word_offset, pack_lossy_chroma_pair(avg_co, avg_cg) << chroma_shift, ignored);
        }
        return;
    }

    uint previous = bgra_from_float4(previousTex.Load(int3(g_desktop_x + local_x, g_desktop_y + local_y, 0)));
    uint xor_pixel = pixel ^ previous;

    if (g_kind == COMMAND_XOR_RAW) {
        outBuf.Store(g_payload_offset + pixel_index * 4, xor_pixel);
        return;
    }

    if (g_kind == COMMAND_XOR_SPARSE && xor_pixel != 0) {
        uint sparse_index;
        InterlockedAdd(g_sparse_count, 1, sparse_index);
        uint entry_offset = g_payload_offset + sparse_index * 8;
        outBuf.Store(entry_offset, pixel_index);
        outBuf.Store(entry_offset + 4, xor_pixel);
        return;
    }

    if (g_kind == COMMAND_MASKED_QUANT_DELTA) {
        uint pixel_count = g_width * g_height;
        uint mask_words = (pixel_count + 31) / 32;
        if (xor_pixel != 0) {
            uint mask_offset = g_payload_offset + 4 + (pixel_index / 32) * 4;
            uint ignored;
            outBuf.InterlockedOr(mask_offset, 1u << (pixel_index & 31), ignored);
            uint packed_delta = pack_quant_delta(pixel, previous, g_quant_shift);
            uint residual_base = g_payload_offset + 4 + mask_words * 4;
            uint residual_word_offset = residual_base + (pixel_index / 2) * 4;
            uint residual_shift = (pixel_index & 1) * 16;
            outBuf.InterlockedOr(residual_word_offset, packed_delta << residual_shift, ignored);
        }
    }
}
"#;
