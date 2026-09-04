#![cfg(windows)]

use std::{slice, time::Duration};

use anyhow::{anyhow, Result};
use windows::{
    core::{s, Interface, PCSTR},
    Win32::Graphics::{
        Direct3D::{Fxc::D3DCompile, ID3DBlob, ID3DInclude},
        Direct3D11::{
            ID3D11Buffer, ID3D11ClassLinkage, ID3D11ComputeShader, ID3D11Device,
            ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
            ID3D11UnorderedAccessView, D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE,
            D3D11_BIND_UNORDERED_ACCESS, D3D11_BUFFER_DESC, D3D11_CPU_ACCESS_READ,
            D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
        },
        Dxgi::{
            Common::{DXGI_FORMAT, DXGI_FORMAT_R32G32B32A32_UINT, DXGI_SAMPLE_DESC},
            IDXGISurface1, DXGI_MAPPED_RECT, DXGI_MAP_READ,
        },
    },
};

use crate::capture::DirtyRect;

const TILE_SIZE: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirtyRectTileClass {
    TextUi,
    PhotoVideo,
    MixedOrUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirtyRectRectClass {
    TextUi,
    PhotoVideo,
    MixedOrUnknown,
}

#[derive(Clone, Debug)]
pub(crate) struct DirtyRectClassifierFrameSummary {
    pub dirty_rect_count: usize,
    pub classified_rect_count: usize,
    pub tile_count: usize,
    pub text_ui_rect_count: usize,
    pub photo_video_rect_count: usize,
    pub mixed_rect_count: usize,
    pub text_ui_tile_count: usize,
    pub photo_video_tile_count: usize,
    pub mixed_tile_count: usize,
    pub classifier_time: Duration,
}

pub(crate) struct DirtyRectClassifier {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    compute_shader: ID3D11ComputeShader,
    params_buffer: ID3D11Buffer,
    source_texture: Option<ID3D11Texture2D>,
    source_srv: Option<ID3D11ShaderResourceView>,
    source_width: u32,
    source_height: u32,
    source_format: DXGI_FORMAT,
    output_texture: Option<ID3D11Texture2D>,
    output_surface: Option<IDXGISurface1>,
    output_uav: Option<ID3D11UnorderedAccessView>,
    staging_texture: Option<ID3D11Texture2D>,
    staging_surface: Option<IDXGISurface1>,
    tiles_width_capacity: u32,
    tiles_height_capacity: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ClassifierParams {
    rect_left: u32,
    rect_top: u32,
    rect_width: u32,
    rect_height: u32,
    frame_width: u32,
    frame_height: u32,
    tiles_x: u32,
    tiles_y: u32,
}

#[derive(Clone, Copy, Default)]
struct TileStats {
    edge_x_sum: u32,
    edge_y_sum: u32,
    luma_range: u32,
    chroma_mean: u32,
}

impl DirtyRectClassifier {
    pub(crate) fn new(device: ID3D11Device, context: ID3D11DeviceContext) -> Result<Self> {
        let compute_shader = create_compute_shader(&device)?;
        let params_buffer = create_params_buffer(&device)?;
        Ok(Self {
            device,
            context,
            compute_shader,
            params_buffer,
            source_texture: None,
            source_srv: None,
            source_width: 0,
            source_height: 0,
            source_format: DXGI_FORMAT(0),
            output_texture: None,
            output_surface: None,
            output_uav: None,
            staging_texture: None,
            staging_surface: None,
            tiles_width_capacity: 0,
            tiles_height_capacity: 0,
        })
    }

    pub(crate) fn classify_frame(
        &mut self,
        source_texture: &ID3D11Texture2D,
        frame_width: u32,
        frame_height: u32,
        dirty_rects: &[DirtyRect],
    ) -> Result<DirtyRectClassifierFrameSummary> {
        self.ensure_source_resources(source_texture, frame_width, frame_height)?;
        unsafe {
            self.context.CopyResource(
                self.source_texture
                    .as_ref()
                    .ok_or_else(|| anyhow!("classifier source texture unavailable"))?,
                source_texture,
            );
        }

        let started_at = std::time::Instant::now();
        let mut summary = DirtyRectClassifierFrameSummary {
            dirty_rect_count: dirty_rects.len(),
            classified_rect_count: 0,
            tile_count: 0,
            text_ui_rect_count: 0,
            photo_video_rect_count: 0,
            mixed_rect_count: 0,
            text_ui_tile_count: 0,
            photo_video_tile_count: 0,
            mixed_tile_count: 0,
            classifier_time: Duration::ZERO,
        };

        for rect in dirty_rects {
            let width = rect.right.saturating_sub(rect.left);
            let height = rect.bottom.saturating_sub(rect.top);
            if width == 0 || height == 0 {
                continue;
            }
            let tiles_x = width.div_ceil(TILE_SIZE);
            let tiles_y = height.div_ceil(TILE_SIZE);
            self.ensure_output_resources(tiles_x, tiles_y)?;
            let params = ClassifierParams {
                rect_left: rect.left,
                rect_top: rect.top,
                rect_width: width,
                rect_height: height,
                frame_width,
                frame_height,
                tiles_x,
                tiles_y,
            };
            self.dispatch_rect(params)?;
            let tile_stats = self.read_tile_stats(tiles_x, tiles_y)?;
            let mut rect_text_tiles = 0usize;
            let mut rect_photo_tiles = 0usize;
            let mut rect_mixed_tiles = 0usize;
            for stats in tile_stats {
                match classify_tile(stats) {
                    DirtyRectTileClass::TextUi => rect_text_tiles += 1,
                    DirtyRectTileClass::PhotoVideo => rect_photo_tiles += 1,
                    DirtyRectTileClass::MixedOrUnknown => rect_mixed_tiles += 1,
                }
            }
            if rect_text_tiles == 0 && rect_photo_tiles == 0 && rect_mixed_tiles == 0 {
                continue;
            }
            summary.classified_rect_count += 1;
            summary.tile_count += rect_text_tiles + rect_photo_tiles + rect_mixed_tiles;
            summary.text_ui_tile_count += rect_text_tiles;
            summary.photo_video_tile_count += rect_photo_tiles;
            summary.mixed_tile_count += rect_mixed_tiles;
            match classify_rect(rect_text_tiles, rect_photo_tiles, rect_mixed_tiles) {
                DirtyRectRectClass::TextUi => summary.text_ui_rect_count += 1,
                DirtyRectRectClass::PhotoVideo => summary.photo_video_rect_count += 1,
                DirtyRectRectClass::MixedOrUnknown => summary.mixed_rect_count += 1,
            }
        }

        summary.classifier_time = started_at.elapsed();
        Ok(summary)
    }

    fn ensure_source_resources(
        &mut self,
        source_texture: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let desc = texture_desc(source_texture);
        let recreate = self.source_texture.is_none()
            || self.source_width != width
            || self.source_height != height
            || self.source_format != desc.Format;
        if !recreate {
            return Ok(());
        }
        let texture_desc = D3D11_TEXTURE2D_DESC {
            Width: desc.Width,
            Height: desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: desc.Format,
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
                .CreateTexture2D(&texture_desc, None, Some(&mut texture))
                .map_err(|err| anyhow!("classifier source texture creation failed: {err}"))?;
        }
        let texture =
            texture.ok_or_else(|| anyhow!("classifier source texture creation returned null"))?;
        let mut srv = None;
        unsafe {
            self.device
                .CreateShaderResourceView(&texture, None, Some(&mut srv))
                .map_err(|err| anyhow!("classifier source SRV creation failed: {err}"))?;
        }
        self.source_texture = Some(texture);
        self.source_srv =
            Some(srv.ok_or_else(|| anyhow!("classifier source SRV creation returned null"))?);
        self.source_width = width;
        self.source_height = height;
        self.source_format = desc.Format;
        Ok(())
    }

    fn ensure_output_resources(&mut self, tiles_x: u32, tiles_y: u32) -> Result<()> {
        let recreate = self.output_texture.is_none()
            || tiles_x > self.tiles_width_capacity
            || tiles_y > self.tiles_height_capacity;
        if !recreate {
            return Ok(());
        }
        let width = tiles_x.max(1);
        let height = tiles_y.max(1);
        let output_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R32G32B32A32_UINT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut output_texture = None;
        unsafe {
            self.device
                .CreateTexture2D(&output_desc, None, Some(&mut output_texture))
                .map_err(|err| anyhow!("classifier output texture creation failed: {err}"))?;
        }
        let output_texture =
            output_texture.ok_or_else(|| anyhow!("classifier output texture returned null"))?;
        let mut uav = None;
        unsafe {
            self.device
                .CreateUnorderedAccessView(&output_texture, None, Some(&mut uav))
                .map_err(|err| anyhow!("classifier UAV creation failed: {err}"))?;
        }

        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R32G32B32A32_UINT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging_texture = None;
        unsafe {
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging_texture))
                .map_err(|err| anyhow!("classifier staging texture creation failed: {err}"))?;
        }
        let staging_texture =
            staging_texture.ok_or_else(|| anyhow!("classifier staging texture returned null"))?;
        let output_surface: IDXGISurface1 = output_texture
            .cast()
            .map_err(|err| anyhow!("classifier output surface cast failed: {err}"))?;
        let staging_surface: IDXGISurface1 = staging_texture
            .cast()
            .map_err(|err| anyhow!("classifier staging surface cast failed: {err}"))?;
        self.output_texture = Some(output_texture);
        self.output_surface = Some(output_surface);
        self.output_uav =
            Some(uav.ok_or_else(|| anyhow!("classifier UAV creation returned null"))?);
        self.staging_texture = Some(staging_texture);
        self.staging_surface = Some(staging_surface);
        self.tiles_width_capacity = width;
        self.tiles_height_capacity = height;
        Ok(())
    }

    fn dispatch_rect(&mut self, params: ClassifierParams) -> Result<()> {
        let source_srv = self
            .source_srv
            .as_ref()
            .ok_or_else(|| anyhow!("classifier source SRV unavailable"))?;
        let output_uav = self
            .output_uav
            .as_ref()
            .ok_or_else(|| anyhow!("classifier output UAV unavailable"))?;
        let params_data = [params];
        let subresource = D3D11_SUBRESOURCE_DATA {
            pSysMem: params_data.as_ptr() as *const _,
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };
        unsafe {
            self.context
                .UpdateSubresource(&self.params_buffer, 0, None, subresource.pSysMem, 0, 0);
            let srvs = [Some(source_srv.clone())];
            let uavs = [Some(output_uav.clone())];
            let initial_counts = [0u32];
            let cbs = [Some(self.params_buffer.clone())];
            self.context.CSSetShader(&self.compute_shader, None);
            self.context.CSSetShaderResources(0, Some(&srvs));
            self.context.CSSetUnorderedAccessViews(
                0,
                1,
                Some(uavs.as_ptr()),
                Some(initial_counts.as_ptr()),
            );
            self.context.CSSetConstantBuffers(0, Some(&cbs));
            self.context
                .Dispatch(params.tiles_x.max(1), params.tiles_y.max(1), 1);
            let null_srvs = [None];
            let null_uavs = [None];
            let null_cbs = [None];
            self.context.CSSetShaderResources(0, Some(&null_srvs));
            self.context
                .CSSetUnorderedAccessViews(0, 1, Some(null_uavs.as_ptr()), None);
            self.context.CSSetConstantBuffers(0, Some(&null_cbs));
        }
        Ok(())
    }

    fn read_tile_stats(&mut self, tiles_x: u32, tiles_y: u32) -> Result<Vec<TileStats>> {
        let output_texture = self
            .output_texture
            .as_ref()
            .ok_or_else(|| anyhow!("classifier output texture unavailable"))?;
        let staging_texture = self
            .staging_texture
            .as_ref()
            .ok_or_else(|| anyhow!("classifier staging texture unavailable"))?;
        let staging_surface = self
            .staging_surface
            .as_ref()
            .ok_or_else(|| anyhow!("classifier staging surface unavailable"))?;
        unsafe {
            self.context.CopyResource(staging_texture, output_texture);
        }
        let mut mapped = DXGI_MAPPED_RECT::default();
        unsafe {
            staging_surface
                .Map(&mut mapped, DXGI_MAP_READ)
                .map_err(|err| anyhow!("classifier staging map failed: {err}"))?;
        }
        let mut stats = Vec::with_capacity((tiles_x * tiles_y) as usize);
        for row in 0..tiles_y as usize {
            let row_ptr = unsafe { mapped.pBits.add(row * mapped.Pitch as usize) } as *const u32;
            let row_data = unsafe { slice::from_raw_parts(row_ptr, tiles_x as usize * 4) };
            for col in 0..tiles_x as usize {
                let idx = col * 4;
                stats.push(TileStats {
                    edge_x_sum: row_data[idx],
                    edge_y_sum: row_data[idx + 1],
                    luma_range: row_data[idx + 2],
                    chroma_mean: row_data[idx + 3],
                });
            }
        }
        let _ = unsafe { staging_surface.Unmap() };
        Ok(stats)
    }
}

fn classify_tile(stats: TileStats) -> DirtyRectTileClass {
    let total_edge = stats.edge_x_sum.saturating_add(stats.edge_y_sum);
    let dominant_axis = stats.edge_x_sum.max(stats.edge_y_sum);
    if total_edge >= 3_200
        && stats.luma_range >= 90
        && stats.chroma_mean <= 42
        && dominant_axis.saturating_mul(100) >= total_edge.saturating_mul(58)
    {
        DirtyRectTileClass::TextUi
    } else if stats.chroma_mean >= 78 || (total_edge <= 1_400 && stats.luma_range <= 72) {
        DirtyRectTileClass::PhotoVideo
    } else {
        DirtyRectTileClass::MixedOrUnknown
    }
}

fn classify_rect(
    text_ui_tiles: usize,
    photo_video_tiles: usize,
    mixed_tiles: usize,
) -> DirtyRectRectClass {
    if text_ui_tiles > photo_video_tiles && text_ui_tiles >= mixed_tiles {
        DirtyRectRectClass::TextUi
    } else if photo_video_tiles > text_ui_tiles && photo_video_tiles >= mixed_tiles {
        DirtyRectRectClass::PhotoVideo
    } else {
        DirtyRectRectClass::MixedOrUnknown
    }
}

fn create_compute_shader(device: &ID3D11Device) -> Result<ID3D11ComputeShader> {
    const SHADER_SOURCE: &str = r#"
Texture2D<float4> sourceTex : register(t0);
RWTexture2D<uint4> outTex : register(u0);

cbuffer Params : register(b0) {
    uint rect_left;
    uint rect_top;
    uint rect_width;
    uint rect_height;
    uint frame_width;
    uint frame_height;
    uint tiles_x;
    uint tiles_y;
};

float luminance(float3 rgb) {
    return dot(rgb, float3(0.2126, 0.7152, 0.0722));
}

[numthreads(1, 1, 1)]
void cs_main(uint3 dispatch_id : SV_DispatchThreadID) {
    uint tile_x = dispatch_id.x;
    uint tile_y = dispatch_id.y;
    if (tile_x >= tiles_x || tile_y >= tiles_y) {
        return;
    }

    uint start_x = rect_left + tile_x * 16;
    uint start_y = rect_top + tile_y * 16;
    uint end_x = min(start_x + 16, rect_left + rect_width);
    uint end_y = min(start_y + 16, rect_top + rect_height);

    float min_luma = 1.0;
    float max_luma = 0.0;
    float edge_x = 0.0;
    float edge_y = 0.0;
    float chroma_sum = 0.0;
    uint sample_count = 0;

    for (uint y = start_y; y < end_y; ++y) {
        for (uint x = start_x; x < end_x; ++x) {
            float3 rgb = sourceTex.Load(int3(x, y, 0)).rgb;
            float luma = luminance(rgb);
            min_luma = min(min_luma, luma);
            max_luma = max(max_luma, luma);
            chroma_sum += abs(rgb.r - rgb.g) + abs(rgb.g - rgb.b) + abs(rgb.b - rgb.r);
            if (x + 1 < end_x) {
                float3 right_rgb = sourceTex.Load(int3(x + 1, y, 0)).rgb;
                edge_x += abs(luma - luminance(right_rgb));
            }
            if (y + 1 < end_y) {
                float3 down_rgb = sourceTex.Load(int3(x, y + 1, 0)).rgb;
                edge_y += abs(luma - luminance(down_rgb));
            }
            sample_count += 1;
        }
    }

    float luma_range = saturate(max_luma - min_luma);
    float chroma_mean = sample_count > 0 ? chroma_sum / sample_count : 0.0;
    outTex[dispatch_id.xy] = uint4(
        (uint)round(edge_x * 4096.0),
        (uint)round(edge_y * 4096.0),
        (uint)round(luma_range * 255.0),
        (uint)round(chroma_mean * 255.0)
    );
}
"#;

    let blob = compile_shader(SHADER_SOURCE, "cs_main", "cs_5_0")?;
    let bytes = blob_bytes(&blob);
    let mut compute_shader = None;
    unsafe {
        device
            .CreateComputeShader(
                bytes,
                None::<&ID3D11ClassLinkage>,
                Some(&mut compute_shader),
            )
            .map_err(|err| anyhow!("classifier compute shader creation failed: {err}"))?;
    }
    compute_shader.ok_or_else(|| anyhow!("classifier compute shader creation returned null"))
}

fn create_params_buffer(device: &ID3D11Device) -> Result<ID3D11Buffer> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: std::mem::size_of::<ClassifierParams>() as u32,
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
            .map_err(|err| anyhow!("classifier params buffer creation failed: {err}"))?;
    }
    buffer.ok_or_else(|| anyhow!("classifier params buffer creation returned null"))
}

fn compile_shader(source: &str, entry_name: &str, target_name: &str) -> Result<ID3DBlob> {
    let mut code = None;
    let mut errors = None;
    let entry = match entry_name {
        "cs_main" => s!("cs_main"),
        other => return Err(anyhow!("unsupported shader entry point: {other}")),
    };
    let target = match target_name {
        "cs_5_0" => s!("cs_5_0"),
        other => return Err(anyhow!("unsupported shader target: {other}")),
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
        Ok(()) => code.ok_or_else(|| anyhow!("shader compiler returned no bytecode")),
        Err(err) => {
            let compiler_output = errors
                .as_ref()
                .map(blob_to_string)
                .unwrap_or_else(|| "no compiler output".to_string());
            Err(anyhow!(
                "classifier shader compile failed for {entry_name}/{target_name}: {err}; {compiler_output}"
            ))
        }
    }
}

fn texture_desc(texture: &ID3D11Texture2D) -> D3D11_TEXTURE2D_DESC {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        texture.GetDesc(&mut desc);
    }
    desc
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe { slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) }
}

fn blob_to_string(blob: &ID3DBlob) -> String {
    String::from_utf8_lossy(blob_bytes(blob)).trim().to_string()
}
