#![cfg(target_os = "windows")]

use std::{
    slice,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use windows::{
    core::Interface,
    Win32::Graphics::{
        Direct3D::{
            D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_9_1,
            D3D_FEATURE_LEVEL_9_2, D3D_FEATURE_LEVEL_9_3,
        },
        Direct3D11::{
            ID3D11RenderTargetView, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
            D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
        },
        Dxgi::{
            Common::{
                DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
                DXGI_SAMPLE_DESC,
            },
            IDXGISurface1, DXGI_MAPPED_RECT, DXGI_MAP_READ,
        },
    },
};

use crate::{
    dump::{DumpDirtyRect, RectU32},
    dxgi_capture::{CapturedFrame, DirtyRect},
    tile_commands::{TileCommandEncoder, TileCommandStream, TileCommandTile, TILE_SIZE},
};

const CONSERVATIVE_MAX_TEXTURE_SIZE: u32 = 16_384;
const DIRTY_RECT_MERGE_MAX_AREA_GROWTH_NUMERATOR: u64 = 5;
const DIRTY_RECT_MERGE_MAX_AREA_GROWTH_DENOMINATOR: u64 = 4;

pub(crate) struct AtlasReadback {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    pub dirty_rects: Vec<DumpDirtyRect>,
    pub tile_commands: TileCommandStream,
    pub timings: AtlasTimings,
    pub fallback_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AtlasTimings {
    pub rect_prepare: Duration,
    pub pack: Duration,
    pub ensure_textures: Duration,
    pub clear: Duration,
    pub gpu_copy_dirty: Duration,
    pub readback: Duration,
    pub classify_commands: Duration,
    pub command_setup: Duration,
    pub command_dispatch: Duration,
    pub command_staging_copy: Duration,
    pub command_map_wait: Duration,
    pub command_result_parse: Duration,
}

impl AtlasTimings {
    pub(crate) fn total(self) -> Duration {
        self.rect_prepare
            + self.pack
            + self.ensure_textures
            + self.clear
            + self.gpu_copy_dirty
            + self.readback
            + self.classify_commands
    }
}

#[derive(Default)]
pub(crate) struct AtlasGpuResources {
    atlas_texture: Option<TextureResource>,
    staging_texture: Option<TextureResource>,
    tile_encoder: Option<TileCommandEncoder>,
}

struct TextureResource {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
}

impl AtlasGpuResources {
    pub(crate) fn reset(&mut self) {
        self.atlas_texture = None;
        self.staging_texture = None;
        self.tile_encoder = None;
    }

    pub(crate) fn reset_tile_cache(&mut self) {
        if let Some(tile_encoder) = self.tile_encoder.as_mut() {
            tile_encoder.reset_tile_cache();
        }
    }

    pub(crate) fn commit_delta_reference(
        &mut self,
        frame: &CapturedFrame,
        emitted_lossy: bool,
        full_lossless_refresh: bool,
    ) -> Result<()> {
        if let Some(tile_encoder) = self.tile_encoder.as_mut() {
            tile_encoder
                .commit_reference(
                    &frame.texture,
                    frame.width,
                    frame.height,
                    frame.format,
                    emitted_lossy,
                    full_lossless_refresh,
                )
                .context("commit tile command delta reference")?;
        }
        Ok(())
    }

    fn ensure_atlas_texture(
        &mut self,
        device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Result<()> {
        if texture_resource_fits(self.atlas_texture.as_ref(), width, height, format) {
            return Ok(());
        }
        self.atlas_texture = Some(TextureResource {
            texture: create_default_texture(device, width, height, format)?,
            width: width.max(1),
            height: height.max(1),
            format,
        });
        Ok(())
    }

    fn ensure_staging_texture(
        &mut self,
        device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Result<()> {
        if texture_resource_fits(self.staging_texture.as_ref(), width, height, format) {
            return Ok(());
        }
        self.staging_texture = Some(TextureResource {
            texture: create_staging_texture(device, width, height, format)?,
            width: width.max(1),
            height: height.max(1),
            format,
        });
        Ok(())
    }

    fn readback_atlas_bgra(
        &self,
        context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
        atlas_texture: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>> {
        let staging = self
            .staging_texture
            .as_ref()
            .context("atlas staging texture unavailable")?;
        let src_box = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: width,
            bottom: height,
            back: 1,
        };
        unsafe {
            context.CopySubresourceRegion(
                &staging.texture,
                0,
                0,
                0,
                0,
                atlas_texture,
                0,
                Some(&src_box),
            );
        }
        readback_staging_texture_bgra(&staging.texture, width, height)
    }
}

fn texture_resource_fits(
    resource: Option<&TextureResource>,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> bool {
    resource.is_some_and(|resource| {
        resource.format == format
            && resource.width >= width.max(1)
            && resource.height >= height.max(1)
    })
}

pub(crate) fn format_label(format: DXGI_FORMAT) -> &'static str {
    if format == DXGI_FORMAT_B8G8R8A8_UNORM {
        "BGRA8"
    } else if format == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB {
        "BGRA8_SRGB"
    } else {
        "UNKNOWN"
    }
}

pub(crate) fn ensure_bgra_format(format: DXGI_FORMAT) -> Result<()> {
    anyhow::ensure!(
        format == DXGI_FORMAT_B8G8R8A8_UNORM || format == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
        "unsupported DXGI atlas dump format: {:?}",
        format
    );
    Ok(())
}

pub(crate) fn copy_dirty_rects_to_compact_atlas(
    resources: &mut AtlasGpuResources,
    frame: &CapturedFrame,
    dirty_rects: &[DirtyRect],
    force_full_frame: bool,
    allow_delta: bool,
    readback_bgra: bool,
) -> Result<AtlasReadback> {
    let mut timings = AtlasTimings::default();
    let stage_started = Instant::now();
    ensure_bgra_format(frame.format)?;
    let fallback_reason = if force_full_frame {
        Some("initial_full_frame".to_string())
    } else {
        None
    };
    let texture_size_limit = max_texture_size_for_device(&frame.device);
    let mut source_rects = if force_full_frame {
        vec![DirtyRect {
            left: 0,
            top: 0,
            right: frame.width,
            bottom: frame.height,
        }]
    } else {
        dirty_rects.to_vec()
    };
    source_rects.retain(|rect| {
        rect.right > rect.left
            && rect.bottom > rect.top
            && rect.right <= frame.width
            && rect.bottom <= frame.height
    });
    if !force_full_frame {
        source_rects = coalesce_dirty_rects(source_rects);
    }
    source_rects = split_rects_to_texture_limit(&source_rects, texture_size_limit);
    timings.rect_prepare = stage_started.elapsed();

    if source_rects.is_empty() {
        return Ok(AtlasReadback {
            width: 1,
            height: 1,
            bgra: vec![0u8; 4],
            dirty_rects: Vec::new(),
            tile_commands: TileCommandStream::empty(1, 1),
            timings,
            fallback_reason: None,
        });
    }

    let stage_started = Instant::now();
    let mut packed = pack_shelf(&source_rects, texture_size_limit)?;
    let mut fallback = fallback_reason;
    if packed.width > texture_size_limit || packed.height > texture_size_limit {
        fallback = Some("atlas_overflow_full_frame".to_string());
        source_rects = split_rects_to_texture_limit(
            &[DirtyRect {
                left: 0,
                top: 0,
                right: frame.width,
                bottom: frame.height,
            }],
            texture_size_limit,
        );
        packed = pack_shelf(&source_rects, texture_size_limit)?;
    }
    timings.pack = stage_started.elapsed();
    anyhow::ensure!(
        packed.width <= texture_size_limit && packed.height <= texture_size_limit,
        "atlas dimensions {}x{} exceed D3D feature-level texture limit {}",
        packed.width,
        packed.height,
        texture_size_limit
    );

    let stage_started = Instant::now();
    resources
        .ensure_atlas_texture(&frame.device, packed.width, packed.height, frame.format)
        .context("ensure atlas texture")?;
    resources
        .ensure_staging_texture(&frame.device, packed.width, packed.height, frame.format)
        .context("ensure atlas staging texture")?;
    timings.ensure_textures = stage_started.elapsed();
    let atlas_texture = resources
        .atlas_texture
        .as_ref()
        .context("atlas texture unavailable after ensure")?
        .texture
        .clone();

    let stage_started = Instant::now();
    clear_texture(&frame.device, &frame.context, &atlas_texture).context("clear atlas texture")?;
    timings.clear = stage_started.elapsed();

    let stage_started = Instant::now();
    for rect in &packed.rects {
        let src_box = D3D11_BOX {
            left: rect.source.left,
            top: rect.source.top,
            front: 0,
            right: rect.source.right,
            bottom: rect.source.bottom,
            back: 1,
        };
        unsafe {
            frame.context.CopySubresourceRegion(
                &atlas_texture,
                0,
                rect.atlas.x,
                rect.atlas.y,
                0,
                &frame.texture,
                0,
                Some(&src_box),
            );
        }
    }
    timings.gpu_copy_dirty = stage_started.elapsed();

    if resources.tile_encoder.is_none() {
        resources.tile_encoder = Some(TileCommandEncoder::new(
            frame.device.clone(),
            frame.context.clone(),
        )?);
    }
    let tile_descriptors = build_tile_command_descriptors(&packed.rects)?;
    let stage_started = Instant::now();
    let tile_commands = resources
        .tile_encoder
        .as_mut()
        .context("tile command encoder unavailable")?
        .encode_atlas(
            &atlas_texture,
            packed.width,
            packed.height,
            frame.width,
            frame.height,
            frame.format,
            &tile_descriptors,
            allow_delta,
        )
        .context("encode atlas tile command stream")?;
    timings.classify_commands = stage_started.elapsed();
    timings.command_setup = tile_commands.timings.setup;
    timings.command_dispatch = tile_commands.timings.dispatch;
    timings.command_staging_copy = tile_commands.timings.staging_copy;
    timings.command_map_wait = tile_commands.timings.map_wait;
    timings.command_result_parse = tile_commands.timings.result_parse;

    let stage_started = Instant::now();
    let bgra = if readback_bgra {
        resources
            .readback_atlas_bgra(&frame.context, &atlas_texture, packed.width, packed.height)
            .context("read back atlas texture")?
    } else {
        Vec::new()
    };
    timings.readback = stage_started.elapsed();
    let emitted_dirty_rects = tile_commands
        .copy_rects
        .iter()
        .map(|rect| DumpDirtyRect {
            desktop: RectU32 {
                x: rect.desktop_x,
                y: rect.desktop_y,
                w: rect.width,
                h: rect.height,
            },
            atlas: RectU32 {
                x: rect.atlas_x,
                y: rect.atlas_y,
                w: rect.width,
                h: rect.height,
            },
        })
        .collect();
    Ok(AtlasReadback {
        width: packed.width,
        height: packed.height,
        bgra,
        dirty_rects: emitted_dirty_rects,
        tile_commands,
        timings,
        fallback_reason: fallback,
    })
}

pub(crate) fn readback_full_frame(frame: &CapturedFrame) -> Result<Vec<u8>> {
    ensure_bgra_format(frame.format)?;
    readback_texture_bgra(
        &frame.device,
        &frame.context,
        &frame.texture,
        frame.width,
        frame.height,
        frame.format,
    )
}

struct PackedAtlas {
    width: u32,
    height: u32,
    rects: Vec<PackedRect>,
}

struct PackedRect {
    source: DirtyRect,
    atlas: RectU32,
}

fn pack_shelf(rects: &[DirtyRect], texture_size_limit: u32) -> Result<PackedAtlas> {
    let atlas_width = rects
        .iter()
        .try_fold(0u32, |sum, rect| {
            sum.checked_add(align_to_tile(rect.right.saturating_sub(rect.left)))
        })
        .context("atlas width overflow")?
        .min(texture_size_limit)
        .max(1);
    let mut current_x = 0u32;
    let mut current_y = 0u32;
    let mut row_height = 0u32;
    let mut packed = Vec::with_capacity(rects.len());

    for rect in rects {
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let packed_width = align_to_tile(width);
        let packed_height = align_to_tile(height);
        if current_x > 0 && current_x.saturating_add(packed_width) > atlas_width {
            current_x = 0;
            current_y = current_y.saturating_add(row_height);
            row_height = 0;
        }
        packed.push(PackedRect {
            source: *rect,
            atlas: RectU32 {
                x: current_x,
                y: current_y,
                w: width,
                h: height,
            },
        });
        current_x = current_x.saturating_add(packed_width);
        row_height = row_height.max(packed_height);
    }

    Ok(PackedAtlas {
        width: atlas_width,
        height: current_y.saturating_add(row_height).max(1),
        rects: packed,
    })
}

fn build_tile_command_descriptors(rects: &[PackedRect]) -> Result<Vec<TileCommandTile>> {
    let mut descriptors = Vec::new();
    for rect in rects {
        let width = rect.source.right - rect.source.left;
        let height = rect.source.bottom - rect.source.top;
        let tiles_x = width.div_ceil(TILE_SIZE);
        let tiles_y = height.div_ceil(TILE_SIZE);
        descriptors.reserve((tiles_x.saturating_mul(tiles_y)) as usize);
        for tile_y in 0..tiles_y {
            let y_offset = tile_y * TILE_SIZE;
            let tile_height = TILE_SIZE.min(height - y_offset);
            for tile_x in 0..tiles_x {
                let x_offset = tile_x * TILE_SIZE;
                let tile_width = TILE_SIZE.min(width - x_offset);
                descriptors.push(TileCommandTile::new(
                    rect.atlas.x + x_offset,
                    rect.atlas.y + y_offset,
                    rect.source.left + x_offset,
                    rect.source.top + y_offset,
                    tile_width,
                    tile_height,
                )?);
            }
        }
    }
    Ok(descriptors)
}

fn align_to_tile(value: u32) -> u32 {
    value.div_ceil(TILE_SIZE).saturating_mul(TILE_SIZE)
}

fn max_texture_size_for_device(device: &windows::Win32::Graphics::Direct3D11::ID3D11Device) -> u32 {
    let feature_level = unsafe { device.GetFeatureLevel() }.0;
    if feature_level >= D3D_FEATURE_LEVEL_11_0.0 {
        CONSERVATIVE_MAX_TEXTURE_SIZE
    } else if feature_level >= D3D_FEATURE_LEVEL_10_0.0 {
        8_192
    } else if feature_level >= D3D_FEATURE_LEVEL_9_3.0 {
        4_096
    } else if feature_level >= D3D_FEATURE_LEVEL_9_2.0 {
        2_048
    } else if feature_level >= D3D_FEATURE_LEVEL_9_1.0 {
        2_048
    } else {
        2_048
    }
}

fn split_rects_to_texture_limit(rects: &[DirtyRect], texture_size_limit: u32) -> Vec<DirtyRect> {
    let limit = texture_size_limit.max(1);
    let mut split = Vec::with_capacity(rects.len());
    for rect in rects {
        let mut top = rect.top;
        while top < rect.bottom {
            let bottom = top.saturating_add(limit).min(rect.bottom);
            let mut left = rect.left;
            while left < rect.right {
                let right = left.saturating_add(limit).min(rect.right);
                split.push(DirtyRect {
                    left,
                    top,
                    right,
                    bottom,
                });
                left = right;
            }
            top = bottom;
        }
    }
    split
}

fn coalesce_dirty_rects(mut rects: Vec<DirtyRect>) -> Vec<DirtyRect> {
    rects.sort_by_key(|rect| (rect.top, rect.left, rect.bottom, rect.right));
    let mut merged: Vec<DirtyRect> = Vec::with_capacity(rects.len());

    for rect in rects {
        let mut pending = rect;
        while let Some(index) = merged
            .iter()
            .position(|existing| should_merge_dirty_rects(*existing, pending))
        {
            let existing = merged.swap_remove(index);
            pending = union_dirty_rect(existing, pending);
        }
        merged.push(pending);
    }

    merged.sort_by_key(|rect| (rect.top, rect.left, rect.bottom, rect.right));
    merged
}

fn should_merge_dirty_rects(a: DirtyRect, b: DirtyRect) -> bool {
    if !rects_touch_or_overlap(a, b) {
        return false;
    }
    let combined_area = dirty_rect_area(a).saturating_add(dirty_rect_area(b));
    let union = union_dirty_rect(a, b);
    let union_area = dirty_rect_area(union);
    union_area.saturating_mul(DIRTY_RECT_MERGE_MAX_AREA_GROWTH_DENOMINATOR)
        <= combined_area.saturating_mul(DIRTY_RECT_MERGE_MAX_AREA_GROWTH_NUMERATOR)
}

fn rects_touch_or_overlap(a: DirtyRect, b: DirtyRect) -> bool {
    a.left <= b.right && b.left <= a.right && a.top <= b.bottom && b.top <= a.bottom
}

fn union_dirty_rect(a: DirtyRect, b: DirtyRect) -> DirtyRect {
    DirtyRect {
        left: a.left.min(b.left),
        top: a.top.min(b.top),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    }
}

fn dirty_rect_area(rect: DirtyRect) -> u64 {
    u64::from(rect.right.saturating_sub(rect.left))
        * u64::from(rect.bottom.saturating_sub(rect.top))
}

fn create_default_texture(
    device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width.max(1),
        Height: height.max(1),
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe {
        device
            .CreateTexture2D(&desc, None, Some(&mut texture))
            .map_err(|err| {
                anyhow::anyhow!(
                    "CreateTexture2D(default {}x{} {:?}) failed: {err}",
                    width,
                    height,
                    format
                )
            })?;
    }
    texture.context("CreateTexture2D(default) returned null")
}

fn create_staging_texture(
    device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width.max(1),
        Height: height.max(1),
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe {
        device
            .CreateTexture2D(&desc, None, Some(&mut texture))
            .map_err(|err| {
                anyhow::anyhow!(
                    "CreateTexture2D(staging {}x{} {:?}) failed: {err}",
                    width,
                    height,
                    format
                )
            })?;
    }
    texture.context("CreateTexture2D(staging) returned null")
}

fn clear_texture(
    device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    texture: &ID3D11Texture2D,
) -> Result<()> {
    let mut rtv: Option<ID3D11RenderTargetView> = None;
    unsafe {
        device
            .CreateRenderTargetView(texture, None, Some(&mut rtv))
            .map_err(|err| anyhow::anyhow!("CreateRenderTargetView failed: {err}"))?;
    }
    let rtv = rtv.context("CreateRenderTargetView returned null")?;
    unsafe {
        context.ClearRenderTargetView(&rtv, &[0.0, 0.0, 0.0, 0.0]);
    }
    Ok(())
}

fn readback_texture_bgra(
    device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    source: &ID3D11Texture2D,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> Result<Vec<u8>> {
    let staging = create_staging_texture(device, width, height, format)?;
    unsafe {
        context.CopyResource(&staging, source);
    }
    readback_staging_texture_bgra(&staging, width, height)
}

fn readback_staging_texture_bgra(
    staging: &ID3D11Texture2D,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let surface: IDXGISurface1 = staging.cast().context("cast staging texture to surface")?;
    let mut mapped = DXGI_MAPPED_RECT::default();
    unsafe {
        surface
            .Map(&mut mapped, DXGI_MAP_READ)
            .map_err(|err| anyhow::anyhow!("staging surface Map failed: {err}"))?;
    }
    let result = copy_mapped_bgra(&mapped, width, height);
    let _ = unsafe { surface.Unmap() };
    result
}

fn copy_mapped_bgra(mapped: &DXGI_MAPPED_RECT, width: u32, height: u32) -> Result<Vec<u8>> {
    let row_bytes = width as usize * 4;
    let pitch = mapped.Pitch as usize;
    anyhow::ensure!(
        pitch >= row_bytes,
        "mapped pitch {pitch} is smaller than row bytes {row_bytes}"
    );
    let mut out = vec![0u8; row_bytes * height as usize];
    for row in 0..height as usize {
        let src = unsafe { mapped.pBits.add(row * pitch) };
        let src = unsafe { slice::from_raw_parts(src as *const u8, row_bytes) };
        let dst = &mut out[row * row_bytes..(row + 1) * row_bytes];
        dst.copy_from_slice(src);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalescing_merges_touching_chain_and_preserves_separate_region() {
        let rect = |left, right| DirtyRect {
            left,
            top: 0,
            right,
            bottom: 10,
        };
        let merged = coalesce_dirty_rects(vec![
            rect(20, 30),
            rect(100, 110),
            rect(0, 10),
            rect(10, 20),
        ]);
        let bounds: Vec<_> = merged
            .iter()
            .map(|r| (r.left, r.top, r.right, r.bottom))
            .collect();
        assert_eq!(bounds, vec![(0, 0, 30, 10), (100, 0, 110, 10)]);
    }
}
