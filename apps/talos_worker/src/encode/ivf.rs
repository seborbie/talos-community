//! IVF (Interchangeable Video Format) writer for VP8 stream dump.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{Context, Result};

/// Builds the 32-byte IVF file header (DKIF, VP80, width, height, fps, frame count 0).
pub fn build_header(width: u32, height: u32, fps: u32) -> [u8; 32] {
    let mut h = [0u8; 32];
    h[0..4].copy_from_slice(b"DKIF");
    h[4..6].copy_from_slice(&0u16.to_le_bytes());
    h[6..8].copy_from_slice(&32u16.to_le_bytes());
    h[8..12].copy_from_slice(b"VP80");
    h[12..14].copy_from_slice(&(width as u16).to_le_bytes());
    h[14..16].copy_from_slice(&(height as u16).to_le_bytes());
    h[16..20].copy_from_slice(&fps.to_le_bytes());
    h[20..24].copy_from_slice(&1u32.to_le_bytes());
    h[24..28].copy_from_slice(&0u32.to_le_bytes());
    h[28..32].copy_from_slice(&0u32.to_le_bytes());
    h
}

pub struct IvfWriter {
    file: File,
    frame_count: u32,
}

impl IvfWriter {
    pub fn new(path: &Path, width: u32, height: u32, fps: u32) -> Result<Self> {
        let mut file = File::create(path).context("create IVF file")?;
        let h = build_header(width, height, fps);
        file.write_all(&h)?;
        file.flush().context("flush IVF header")?;

        Ok(Self {
            file,
            frame_count: 0,
        })
    }

    pub fn write_frame(&mut self, payload: &[u8], pts: u64) -> Result<()> {
        self.file.write_all(&(payload.len() as u32).to_le_bytes())?;
        self.file.write_all(&pts.to_le_bytes())?;
        self.file.write_all(payload)?;
        self.frame_count = self.frame_count.saturating_add(1);
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        self.file.seek(SeekFrom::Start(24))?;
        self.file.write_all(&self.frame_count.to_le_bytes())?;
        self.file.flush().context("flush IVF file")?;
        Ok(())
    }
}
