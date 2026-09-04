use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub(crate) struct RectU32 {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DumpDirtyRect {
    pub desktop: RectU32,
    pub atlas: RectU32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DumpMoveRect {
    pub src: RectU32,
    pub dst: RectU32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DumpFrameMetadata {
    pub frame_id: u64,
    pub timestamp_unix_ms: u64,
    pub desktop_width: u32,
    pub desktop_height: u32,
    pub atlas_width: u32,
    pub atlas_height: u32,
    pub pixel_format: String,
    pub accumulated_frames: u32,
    pub rects_coalesced: bool,
    pub dirty_rects: Vec<DumpDirtyRect>,
    pub move_rects: Vec<DumpMoveRect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ReplayComparison {
    pub compared: bool,
    pub mismatched_pixels: u64,
    pub first_mismatch: Option<RectU32>,
}

pub(crate) fn write_metadata(path: &Path, metadata: &DumpFrameMetadata) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(metadata).context("serialize dump metadata")?;
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

pub(crate) fn write_comparison(path: &Path, comparison: &ReplayComparison) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(comparison).context("serialize replay comparison")?;
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

pub(crate) fn write_bgra(path: &Path, bgra: &[u8]) -> Result<()> {
    fs::write(path, bgra).with_context(|| format!("write {}", path.display()))
}

pub(crate) fn write_bmp(path: &Path, width: u32, height: u32, bgra: &[u8]) -> Result<()> {
    let row_bytes = width as usize * 4;
    let expected = row_bytes
        .checked_mul(height as usize)
        .context("BMP dimensions overflow")?;
    anyhow::ensure!(
        bgra.len() == expected,
        "BMP BGRA length mismatch: expected {expected}, got {}",
        bgra.len()
    );

    let file_size = 14usize
        .checked_add(40)
        .and_then(|header| header.checked_add(bgra.len()))
        .context("BMP file size overflow")?;
    let mut writer =
        BufWriter::new(File::create(path).with_context(|| format!("create {}", path.display()))?);

    writer.write_all(b"BM")?;
    writer.write_all(&(file_size as u32).to_le_bytes())?;
    writer.write_all(&[0u8; 4])?;
    writer.write_all(&(54u32).to_le_bytes())?;

    writer.write_all(&(40u32).to_le_bytes())?;
    writer.write_all(&(width as i32).to_le_bytes())?;
    writer.write_all(&(-(height as i32)).to_le_bytes())?;
    writer.write_all(&(1u16).to_le_bytes())?;
    writer.write_all(&(32u16).to_le_bytes())?;
    writer.write_all(&(0u32).to_le_bytes())?;
    writer.write_all(&(bgra.len() as u32).to_le_bytes())?;
    writer.write_all(&(0i32).to_le_bytes())?;
    writer.write_all(&(0i32).to_le_bytes())?;
    writer.write_all(&(0u32).to_le_bytes())?;
    writer.write_all(&(0u32).to_le_bytes())?;
    writer.write_all(bgra)?;
    writer.flush()?;
    Ok(())
}
