use std::path::Path;

use anyhow::{Context, Result};

use crate::dump::{
    write_bgra, write_bmp, write_comparison, DumpFrameMetadata, RectU32, ReplayComparison,
};
use crate::tile_commands::{reconstruct_atlas_from_stream, TileCommandStream};

pub(crate) struct ReplayState {
    previous: Option<Vec<u8>>,
}

impl ReplayState {
    pub(crate) fn new() -> Self {
        Self { previous: None }
    }

    pub(crate) fn replay_frame(
        &mut self,
        frame_dir: &Path,
        metadata: &DumpFrameMetadata,
        tile_commands: &TileCommandStream,
        validation_bgra: Option<&[u8]>,
    ) -> Result<()> {
        let frame_len = frame_len(metadata.desktop_width, metadata.desktop_height)?;
        let previous = self
            .previous
            .as_deref()
            .filter(|previous| previous.len() == frame_len);
        let mut reconstructed = previous
            .map(|previous| previous.to_vec())
            .unwrap_or_else(|| vec![0u8; frame_len]);

        if let Some(previous) = previous {
            apply_move_rects(previous, &mut reconstructed, metadata)?;
        } else if !metadata.move_rects.is_empty() {
            anyhow::bail!("cannot replay move rects without a previous desktop shadow");
        }
        let delta_reference = reconstructed.clone();
        let command_reconstructed_atlas = reconstruct_atlas_from_stream(
            tile_commands,
            Some(&delta_reference),
            metadata.desktop_width,
            metadata.desktop_height,
        )
        .context("reconstruct atlas from stream")?;
        apply_dirty_rects(&mut reconstructed, metadata, &command_reconstructed_atlas)?;

        write_bgra(&frame_dir.join("reconstructed.bgra"), &reconstructed)?;
        write_bmp(
            &frame_dir.join("reconstructed.bmp"),
            metadata.desktop_width,
            metadata.desktop_height,
            &reconstructed,
        )?;

        let comparison = if let Some(expected) = validation_bgra {
            compare_frames(
                expected,
                &reconstructed,
                metadata.desktop_width,
                metadata.desktop_height,
            )?
        } else {
            ReplayComparison {
                compared: false,
                mismatched_pixels: 0,
                first_mismatch: None,
            }
        };
        write_comparison(&frame_dir.join("replay_compare.json"), &comparison)?;
        self.previous = Some(reconstructed);
        Ok(())
    }
}

fn apply_move_rects(
    previous: &[u8],
    reconstructed: &mut [u8],
    metadata: &DumpFrameMetadata,
) -> Result<()> {
    let desktop_stride = metadata.desktop_width as usize * 4;
    for move_rect in &metadata.move_rects {
        validate_rect(
            move_rect.src,
            metadata.desktop_width,
            metadata.desktop_height,
        )
        .context("validate move source rect")?;
        validate_rect(
            move_rect.dst,
            metadata.desktop_width,
            metadata.desktop_height,
        )
        .context("validate move destination rect")?;
        anyhow::ensure!(
            move_rect.src.w == move_rect.dst.w && move_rect.src.h == move_rect.dst.h,
            "move rect source and destination dimensions differ"
        );
        let row_bytes = move_rect.src.w as usize * 4;
        for row in 0..move_rect.src.h as usize {
            let src =
                ((move_rect.src.y as usize + row) * desktop_stride) + move_rect.src.x as usize * 4;
            let dst =
                ((move_rect.dst.y as usize + row) * desktop_stride) + move_rect.dst.x as usize * 4;
            reconstructed[dst..dst + row_bytes].copy_from_slice(&previous[src..src + row_bytes]);
        }
    }
    Ok(())
}

fn apply_dirty_rects(
    reconstructed: &mut [u8],
    metadata: &DumpFrameMetadata,
    atlas_bgra: &[u8],
) -> Result<()> {
    let desktop_stride = metadata.desktop_width as usize * 4;
    let atlas_stride = metadata.atlas_width as usize * 4;
    let expected_atlas_len = frame_len(metadata.atlas_width, metadata.atlas_height)?;
    anyhow::ensure!(
        atlas_bgra.len() == expected_atlas_len,
        "atlas length mismatch: expected {expected_atlas_len}, got {}",
        atlas_bgra.len()
    );
    for dirty_rect in &metadata.dirty_rects {
        validate_rect(
            dirty_rect.desktop,
            metadata.desktop_width,
            metadata.desktop_height,
        )
        .context("validate dirty desktop rect")?;
        validate_rect(
            dirty_rect.atlas,
            metadata.atlas_width,
            metadata.atlas_height,
        )
        .context("validate dirty atlas rect")?;
        anyhow::ensure!(
            dirty_rect.desktop.w == dirty_rect.atlas.w
                && dirty_rect.desktop.h == dirty_rect.atlas.h,
            "dirty desktop and atlas dimensions differ"
        );
        let row_bytes = dirty_rect.desktop.w as usize * 4;
        for row in 0..dirty_rect.desktop.h as usize {
            let src = ((dirty_rect.atlas.y as usize + row) * atlas_stride)
                + dirty_rect.atlas.x as usize * 4;
            let dst = ((dirty_rect.desktop.y as usize + row) * desktop_stride)
                + dirty_rect.desktop.x as usize * 4;
            reconstructed[dst..dst + row_bytes].copy_from_slice(&atlas_bgra[src..src + row_bytes]);
        }
    }
    Ok(())
}

fn compare_frames(
    expected: &[u8],
    actual: &[u8],
    width: u32,
    height: u32,
) -> Result<ReplayComparison> {
    let expected_len = frame_len(width, height)?;
    anyhow::ensure!(
        expected.len() == expected_len && actual.len() == expected_len,
        "comparison length mismatch"
    );
    let mut mismatched_pixels = 0u64;
    let mut first_mismatch = None;
    for pixel_index in 0..(width as usize * height as usize) {
        let start = pixel_index * 4;
        if expected[start..start + 4] != actual[start..start + 4] {
            mismatched_pixels = mismatched_pixels.saturating_add(1);
            if first_mismatch.is_none() {
                first_mismatch = Some(RectU32 {
                    x: (pixel_index % width as usize) as u32,
                    y: (pixel_index / width as usize) as u32,
                    w: 1,
                    h: 1,
                });
            }
        }
    }
    Ok(ReplayComparison {
        compared: true,
        mismatched_pixels,
        first_mismatch,
    })
}

fn validate_rect(rect: RectU32, width: u32, height: u32) -> Result<()> {
    anyhow::ensure!(rect.w > 0 && rect.h > 0, "rect has zero dimensions");
    anyhow::ensure!(
        rect.x
            .checked_add(rect.w)
            .is_some_and(|right| right <= width)
            && rect
                .y
                .checked_add(rect.h)
                .is_some_and(|bottom| bottom <= height),
        "rect exceeds bounds"
    );
    Ok(())
}

fn frame_len(width: u32, height: u32) -> Result<usize> {
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .map(|bytes| bytes as usize)
        .context("frame dimensions overflow")
}
