//! Frame dump to disk (BMP) for testing capture backends.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use crate::capture::Frame;

/// Creates the dump directory if it does not exist.
pub fn prepare_dump_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).context("create capture dump directory")?;
    Ok(())
}

/// Writes a frame as a BMP file. Expects BGRA top-down pixel data; BMP is bottom-up so rows are flipped.
pub fn write_frame_as_bmp(frame: &Frame, path: &Path) -> Result<()> {
    if !matches!(frame.format, crate::capture::PixelFormat::Bgra8) {
        anyhow::bail!("only Bgra8 format is supported for BMP dump");
    }
    let width = frame.width;
    let height = frame.height;
    let stride = frame.stride;

    // BMP row size must be multiple of 4 bytes
    let row_bytes = (width as usize * 4 + 3) & !3;
    let image_size = row_bytes * height as usize;

    // File header: 14 bytes
    let file_header_size = 14u32;
    let info_header_size = 40u32;
    let pixel_offset = file_header_size + info_header_size;
    let file_size = pixel_offset + image_size as u32;

    let mut f = File::create(path).context("create BMP file")?;

    // BMP file header (14 bytes)
    f.write_all(b"BM")?;
    f.write_all(&file_size.to_le_bytes())?;
    f.write_all(&0u16.to_le_bytes())?;
    f.write_all(&0u16.to_le_bytes())?;
    f.write_all(&pixel_offset.to_le_bytes())?;

    // BITMAPINFOHEADER (40 bytes)
    f.write_all(&info_header_size.to_le_bytes())?;
    f.write_all(&(width as i32).to_le_bytes())?;
    f.write_all(&(height as i32).to_le_bytes())?; // positive = bottom-up
    f.write_all(&1u16.to_le_bytes())?; // biPlanes = 1
    f.write_all(&32u16.to_le_bytes())?; // biBitCount = 32
    f.write_all(&0u32.to_le_bytes())?; // biCompression = BI_RGB
    f.write_all(&(image_size as u32).to_le_bytes())?;
    f.write_all(&0i32.to_le_bytes())?; // biXPelsPerMeter
    f.write_all(&0i32.to_le_bytes())?; // biYPelsPerMeter
    f.write_all(&0u32.to_le_bytes())?; // biClrUsed
    f.write_all(&0u32.to_le_bytes())?; // biClrImportant

    // Pixel data: BMP is stored bottom-up, frame is top-down BGRA
    let src_row = stride as usize;
    for y in (0..height as usize).rev() {
        let row_start = y * src_row;
        let row_end = row_start + (width as usize * 4);
        if row_end <= frame.data.len() {
            f.write_all(&frame.data[row_start..row_end])?;
            // Pad row to 4-byte boundary
            let padding = row_bytes - (width as usize * 4);
            if padding > 0 {
                f.write_all(&vec![0u8; padding])?;
            }
        }
    }

    f.flush().context("flush BMP file")?;
    Ok(())
}
