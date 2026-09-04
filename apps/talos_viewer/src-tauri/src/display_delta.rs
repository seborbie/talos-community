#![cfg(windows)]

use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

use talos_protocol::{
    decode_display_record, DisplayAtlasRect, DisplayRecord,
    DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL,
};
use tracing::{debug, warn};

use crate::{
    mf_h264::H264Decoder,
    viewport_d3d11::ExperimentalMoveRect,
    viewport_video::{
        present_cached_frame_with_damage, present_experimental_atlas_commands_gpu, CachedFrame,
        ExperimentalPresentResult, FrameDamageRect,
    },
    ViewportArc,
};

const MAX_DEFERRED_EXPERIMENTAL_FRAMES: usize = 64;
const MAX_DEFERRED_EXPERIMENTAL_BYTES: usize = 192 * 1024 * 1024;
const LARGE_EXPERIMENTAL_FRAME_BYTES: usize = 1024 * 1024;

struct PendingExperimentalAtlas {
    atlas_width: u32,
    atlas_height: u32,
    rects: Vec<DisplayAtlasRect>,
    tile_commands: Vec<u8>,
    record_bytes: usize,
}

struct DeferredExperimentalFrame {
    frame_id: u64,
    desktop_width: u32,
    desktop_height: u32,
    atlas: PendingExperimentalAtlas,
    moves: Vec<ExperimentalMoveRect>,
    present_after_composite: bool,
}

impl DeferredExperimentalFrame {
    fn queued_bytes(&self) -> usize {
        self.atlas
            .tile_commands
            .len()
            .saturating_add(self.atlas.record_bytes)
            .saturating_add(
                self.atlas
                    .rects
                    .len()
                    .saturating_mul(std::mem::size_of::<DisplayAtlasRect>()),
            )
            .saturating_add(
                self.moves
                    .len()
                    .saturating_mul(std::mem::size_of::<ExperimentalMoveRect>()),
            )
    }
}

struct ExperimentalSummaryContext {
    session_id: String,
    transport: String,
}

#[derive(Default)]
struct ExperimentalViewerSummary {
    frames: u64,
    deferred_frames: u64,
    dropped_deferred_frames: u64,
    max_deferred_frames: usize,
    max_deferred_bytes: usize,
    total: Duration,
    validate: Duration,
    upload: Duration,
    decode: Duration,
    moves: Duration,
    dirty: Duration,
    present: Duration,
    max_total: Duration,
    max_present: Duration,
    max_upload: Duration,
    max_decode: Duration,
    tile_bytes: u64,
    record_bytes: u64,
    tile_cmds: u64,
    dirty_rects: u64,
    move_rects: u64,
}

impl ExperimentalViewerSummary {
    fn add(&mut self, sample: ViewerFrameSample) {
        self.frames = self.frames.saturating_add(1);
        self.total += sample.total;
        self.validate += sample.validate;
        self.upload += sample.upload;
        self.decode += sample.decode;
        self.moves += sample.moves;
        self.dirty += sample.dirty;
        self.present += sample.present;
        self.max_total = self.max_total.max(sample.total);
        self.max_present = self.max_present.max(sample.present);
        self.max_upload = self.max_upload.max(sample.upload);
        self.max_decode = self.max_decode.max(sample.decode);
        self.tile_bytes = self.tile_bytes.saturating_add(sample.tile_bytes as u64);
        self.record_bytes = self.record_bytes.saturating_add(sample.record_bytes as u64);
        self.tile_cmds = self.tile_cmds.saturating_add(sample.tile_cmds as u64);
        self.dirty_rects = self.dirty_rects.saturating_add(sample.dirty_rects as u64);
        self.move_rects = self.move_rects.saturating_add(sample.move_rects as u64);
    }

    fn add_deferred(&mut self, deferred_frames: usize, deferred_bytes: usize) {
        self.deferred_frames = self.deferred_frames.saturating_add(1);
        self.max_deferred_frames = self.max_deferred_frames.max(deferred_frames);
        self.max_deferred_bytes = self.max_deferred_bytes.max(deferred_bytes);
    }

    fn add_dropped_deferred(&mut self) {
        self.dropped_deferred_frames = self.dropped_deferred_frames.saturating_add(1);
    }

    fn log(&self, session_id: &str, transport: &str, close_reason: &str) {
        let normal_session_ends = u8::from(close_reason == "normal");
        let forced_session_ends = u8::from(close_reason == "forced_drop");
        let abnormal_session_ends = u8::from(normal_session_ends == 0);
        append_experimental_log(&format!(
            concat!(
                "viewer experimental summary session_id={} transport={} close_reason={} ",
                "normal_session_ends={} abnormal_session_ends={} forced_session_ends={} frames={} ",
                "deferred_frames={} dropped_deferred_frames={} max_deferred_frames={} max_deferred_bytes={} ",
                "total_ms={:.3} avg_total_ms={:.3} max_total_ms={:.3}"
            ),
            session_id,
            transport,
            close_reason,
            normal_session_ends,
            abnormal_session_ends,
            forced_session_ends,
            self.frames,
            self.deferred_frames,
            self.dropped_deferred_frames,
            self.max_deferred_frames,
            self.max_deferred_bytes,
            duration_ms(self.total),
            avg_duration_ms(self.total, self.frames),
            duration_ms(self.max_total),
        ));
        append_experimental_log(&format!(
            concat!(
                "viewer experimental timing summary session_id={} transport={} ",
                "validate_ms={:.3} avg_validate_ms={:.3} upload_ms={:.3} avg_upload_ms={:.3} max_upload_ms={:.3} ",
                "decode_ms={:.3} avg_decode_ms={:.3} max_decode_ms={:.3} move_ms={:.3} dirty_ms={:.3} ",
                "present_ms={:.3} avg_present_ms={:.3} max_present_ms={:.3}"
            ),
            session_id,
            transport,
            duration_ms(self.validate),
            avg_duration_ms(self.validate, self.frames),
            duration_ms(self.upload),
            avg_duration_ms(self.upload, self.frames),
            duration_ms(self.max_upload),
            duration_ms(self.decode),
            avg_duration_ms(self.decode, self.frames),
            duration_ms(self.max_decode),
            duration_ms(self.moves),
            duration_ms(self.dirty),
            duration_ms(self.present),
            avg_duration_ms(self.present, self.frames),
            duration_ms(self.max_present),
        ));
        append_experimental_log(&format!(
            concat!(
                "viewer experimental byte summary session_id={} transport={} tile_bytes={} avg_tile_bytes={:.1} ",
                "record_bytes={} avg_record_bytes={:.1} tile_cmds={} avg_tile_cmds={:.1} ",
                "dirty_rects={} move_rects={}"
            ),
            session_id,
            transport,
            self.tile_bytes,
            avg_u64(self.tile_bytes, self.frames),
            self.record_bytes,
            avg_u64(self.record_bytes, self.frames),
            self.tile_cmds,
            avg_u64(self.tile_cmds, self.frames),
            self.dirty_rects,
            self.move_rects,
        ));
    }
}

struct ViewerFrameSample {
    total: Duration,
    validate: Duration,
    upload: Duration,
    decode: Duration,
    moves: Duration,
    dirty: Duration,
    present: Duration,
    tile_bytes: usize,
    record_bytes: usize,
    tile_cmds: u32,
    dirty_rects: usize,
    move_rects: usize,
}

pub(crate) struct ModernDisplayCompositor {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
    decoder: Option<H264Decoder>,
    decoder_size: Option<(u32, u32)>,
    current_frame_id: Option<u64>,
    damage: Vec<FrameDamageRect>,
    experimental_frame: bool,
    experimental_chunked_frame: bool,
    experimental_atlas: Option<PendingExperimentalAtlas>,
    experimental_moves: Vec<ExperimentalMoveRect>,
    deferred_experimental_frames: VecDeque<DeferredExperimentalFrame>,
    deferred_experimental_bytes: usize,
    experimental_summary: ExperimentalViewerSummary,
    experimental_summary_context: Option<ExperimentalSummaryContext>,
    experimental_summary_logged: bool,
}

impl ModernDisplayCompositor {
    pub(crate) fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            bgra: Vec::new(),
            decoder: None,
            decoder_size: None,
            current_frame_id: None,
            damage: Vec::new(),
            experimental_frame: false,
            experimental_chunked_frame: false,
            experimental_atlas: None,
            experimental_moves: Vec::new(),
            deferred_experimental_frames: VecDeque::new(),
            deferred_experimental_bytes: 0,
            experimental_summary: ExperimentalViewerSummary::default(),
            experimental_summary_context: None,
            experimental_summary_logged: false,
        }
    }

    pub(crate) fn set_experimental_summary_context(&mut self, session_id: &str, transport: &str) {
        self.experimental_summary_context = Some(ExperimentalSummaryContext {
            session_id: session_id.to_string(),
            transport: transport.to_string(),
        });
    }

    pub(crate) fn log_experimental_summary(
        &mut self,
        session_id: &str,
        transport: &str,
        close_reason: &str,
    ) {
        if self.experimental_summary_logged {
            return;
        }
        self.experimental_summary
            .log(session_id, transport, close_reason);
        self.experimental_summary_logged = true;
    }

    pub(crate) fn dimensions(&self) -> Option<(u32, u32)> {
        if self.width == 0 || self.height == 0 {
            return None;
        }
        Some((self.width, self.height))
    }

    pub(crate) fn handle_record(
        &mut self,
        _session_id: &str,
        viewport: &ViewportArc,
        record_bytes: &[u8],
    ) -> Result<(), String> {
        let decode_started = Instant::now();
        let decoded = decode_display_record(record_bytes).map_err(|err| err.to_string())?;
        let decode_elapsed = decode_started.elapsed();
        let record_label = display_record_label(&decoded);
        let apply_started = Instant::now();
        let result = match decoded {
            DisplayRecord::FrameBegin {
                frame_id,
                width,
                height,
            } => {
                debug!(
                    frame_id,
                    width, height, "viewer modern compositor frame begin"
                );
                self.begin_frame(frame_id, width, height)?;
                Ok(())
            }
            DisplayRecord::FrameEnd { frame_id } => {
                debug!(
                    frame_id,
                    damage_rects = self.damage.len(),
                    width = self.width,
                    height = self.height,
                    "viewer modern compositor frame end"
                );
                self.end_frame(viewport, frame_id)
            }
            DisplayRecord::MoveRect {
                frame_id,
                src_x,
                src_y,
                dst_x,
                dst_y,
                width,
                height,
            } => {
                debug!(
                    frame_id,
                    src_x, src_y, dst_x, dst_y, width, height, "viewer modern compositor move rect"
                );
                self.ensure_frame(frame_id)?;
                if self.experimental_frame {
                    self.validate_rect(src_x, src_y, width, height)?;
                    self.validate_rect(dst_x, dst_y, width, height)?;
                    self.experimental_moves.push(ExperimentalMoveRect {
                        src_x,
                        src_y,
                        dst_x,
                        dst_y,
                        width,
                        height,
                    });
                    return Ok(());
                }
                self.apply_move_rect(src_x, src_y, dst_x, dst_y, width, height)
            }
            DisplayRecord::AtlasH264 {
                frame_id,
                flags,
                atlas_width,
                atlas_height,
                rects,
                payload,
            } => {
                debug!(
                    frame_id,
                    flags,
                    atlas_width,
                    atlas_height,
                    rects = rects.len(),
                    payload_len = payload.len(),
                    "viewer modern compositor h264 atlas record"
                );
                self.ensure_frame(frame_id)?;
                self.apply_h264_atlas(atlas_width, atlas_height, &rects, &payload)
            }
            DisplayRecord::ExperimentalAtlasCommands {
                frame_id,
                atlas_width,
                atlas_height,
                rects,
                tile_commands,
            } => {
                debug!(
                    frame_id,
                    atlas_width,
                    atlas_height,
                    rects = rects.len(),
                    tile_commands_len = tile_commands.len(),
                    "viewer experimental atlas commands record"
                );
                self.ensure_frame(frame_id)?;
                self.experimental_frame = true;
                self.validate_experimental_atlas(atlas_width, atlas_height, &rects)?;
                self.experimental_atlas = Some(PendingExperimentalAtlas {
                    atlas_width,
                    atlas_height,
                    rects,
                    tile_commands,
                    record_bytes: record_bytes.len(),
                });
                Ok(())
            }
            DisplayRecord::ExperimentalAtlasCommandsChunk {
                frame_id,
                flags,
                chunk_index,
                chunk_count,
                atlas_width,
                atlas_height,
                rects,
                tile_commands,
            } => {
                debug!(
                    frame_id,
                    flags,
                    chunk_index,
                    chunk_count,
                    atlas_width,
                    atlas_height,
                    rects = rects.len(),
                    tile_commands_len = tile_commands.len(),
                    "viewer experimental atlas command chunk record"
                );
                self.ensure_frame(frame_id)?;
                self.experimental_frame = true;
                self.experimental_chunked_frame = true;
                self.validate_experimental_atlas(atlas_width, atlas_height, &rects)?;
                let present_after_composite =
                    flags & DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL != 0;
                let moves = std::mem::take(&mut self.experimental_moves);
                let frame = DeferredExperimentalFrame {
                    frame_id,
                    desktop_width: self.width,
                    desktop_height: self.height,
                    atlas: PendingExperimentalAtlas {
                        atlas_width,
                        atlas_height,
                        rects,
                        tile_commands,
                        record_bytes: record_bytes.len(),
                    },
                    moves,
                    present_after_composite,
                };
                self.queue_deferred_experimental_frame(frame);
                self.flush_deferred_experimental_frames(viewport)?;
                append_experimental_log(&format!(
                    concat!(
                        "viewer experimental chunk queued frame={} chunk_index={} chunk_count={} ",
                        "flags={} present_after_composite={} record_bytes={} decode_ms={:.3}"
                    ),
                    frame_id,
                    chunk_index,
                    chunk_count,
                    flags,
                    present_after_composite,
                    record_bytes.len(),
                    duration_ms(decode_elapsed),
                ));
                Ok(())
            }
            DisplayRecord::Keyframe {
                frame_id,
                width,
                height,
                payload,
                ..
            } => {
                debug!(
                    frame_id,
                    width,
                    height,
                    payload_len = payload.len(),
                    "viewer modern compositor bgra keyframe"
                );
                self.begin_frame(frame_id, width, height)?;
                if payload.len() != self.bgra.len() {
                    return Err("display keyframe payload length mismatch".to_string());
                }
                self.bgra.copy_from_slice(&payload);
                self.damage.push(FrameDamageRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                });
                Ok(())
            }
        };
        let apply_elapsed = apply_started.elapsed();
        if record_bytes.len() >= 256 * 1024
            || record_label.starts_with("experimental")
            || record_label.starts_with("atlas_h264")
        {
            append_experimental_log(&format!(
                concat!(
                    "viewer experimental record handled type={} record_bytes={} ",
                    "decode_ms={:.3} apply_ms={:.3} ok={}"
                ),
                record_label,
                record_bytes.len(),
                duration_ms(decode_elapsed),
                duration_ms(apply_elapsed),
                result.is_ok(),
            ));
        }
        result
    }

    fn begin_frame(&mut self, frame_id: u64, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Err("display frame has zero dimensions".to_string());
        }
        let len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "display frame dimensions overflow".to_string())?
            as usize;
        if self.width != width || self.height != height || self.bgra.len() != len {
            self.width = width;
            self.height = height;
            self.bgra = vec![0; len];
        }
        self.current_frame_id = Some(frame_id);
        self.damage.clear();
        self.experimental_frame = false;
        self.experimental_chunked_frame = false;
        self.experimental_atlas = None;
        self.experimental_moves.clear();
        Ok(())
    }

    fn ensure_frame(&self, frame_id: u64) -> Result<(), String> {
        if self.current_frame_id != Some(frame_id) {
            return Err("display record frame id does not match active frame".to_string());
        }
        Ok(())
    }

    fn end_frame(&mut self, viewport: &ViewportArc, frame_id: u64) -> Result<(), String> {
        self.ensure_frame(frame_id)?;
        if self.experimental_frame {
            self.end_experimental_frame(viewport, frame_id)?;
            self.current_frame_id = None;
            self.damage.clear();
            self.experimental_frame = false;
            self.experimental_chunked_frame = false;
            self.experimental_atlas = None;
            self.experimental_moves.clear();
            return Ok(());
        }
        let present_started = Instant::now();
        let argb = bgra_to_words(&self.bgra);
        let mut guard = viewport.lock().map_err(|err| err.to_string())?;
        let last_rect = guard.last_rect;
        let has_surface = guard.surface.is_some();
        let has_gpu_viewport = guard.gpu_viewport.is_some();
        guard.cached_frame = Some(CachedFrame {
            width: self.width,
            height: self.height,
            argb,
        });
        debug!(
            frame_id,
            width = self.width,
            height = self.height,
            damage_rects = self.damage.len(),
            last_rect = ?last_rect,
            has_surface,
            has_gpu_viewport,
            "viewer modern compositor cached frame; presenting"
        );
        present_cached_frame_with_damage(&mut guard, &self.damage, self.damage.is_empty())?;
        let _present_elapsed = present_started.elapsed();
        debug!(
            frame_id,
            damage_rects = self.damage.len(),
            "viewer modern compositor present returned"
        );
        self.current_frame_id = None;
        self.damage.clear();
        Ok(())
    }

    fn apply_move_rect(
        &mut self,
        src_x: u32,
        src_y: u32,
        dst_x: u32,
        dst_y: u32,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        self.validate_rect(src_x, src_y, width, height)?;
        self.validate_rect(dst_x, dst_y, width, height)?;
        let row_len = width as usize * 4;
        let mut temp = vec![0u8; row_len * height as usize];
        for row in 0..height {
            let src = self.pixel_offset(src_x, src_y + row);
            let dst = row as usize * row_len;
            temp[dst..dst + row_len].copy_from_slice(&self.bgra[src..src + row_len]);
        }
        for row in 0..height {
            let src = row as usize * row_len;
            let dst = self.pixel_offset(dst_x, dst_y + row);
            self.bgra[dst..dst + row_len].copy_from_slice(&temp[src..src + row_len]);
        }
        self.damage.push(FrameDamageRect {
            x: dst_x,
            y: dst_y,
            width,
            height,
        });
        Ok(())
    }

    fn apply_h264_atlas(
        &mut self,
        atlas_width: u32,
        atlas_height: u32,
        rects: &[talos_protocol::DisplayAtlasRect],
        payload: &[u8],
    ) -> Result<(), String> {
        if self.decoder_size != Some((atlas_width, atlas_height)) {
            debug!(
                atlas_width,
                atlas_height, "viewer modern compositor initializing h264 atlas decoder"
            );
            self.decoder = Some(H264Decoder::new(atlas_width, atlas_height, 30)?);
            self.decoder_size = Some((atlas_width, atlas_height));
        }
        let Some(decoder) = self.decoder.as_mut() else {
            return Err("h264 decoder unavailable".to_string());
        };
        let Some(atlas_bgra) = decoder.decode(payload)? else {
            debug!(
                atlas_width,
                atlas_height,
                payload_len = payload.len(),
                rects = rects.len(),
                "viewer modern compositor h264 atlas decode produced no frame"
            );
            return Ok(());
        };
        debug!(
            atlas_width,
            atlas_height,
            payload_len = payload.len(),
            atlas_bgra_len = atlas_bgra.len(),
            rects = rects.len(),
            "viewer modern compositor h264 atlas decoded"
        );
        let atlas_stride = atlas_width as usize * 4;
        for rect in rects {
            self.validate_rect(rect.dst_x, rect.dst_y, rect.width, rect.height)?;
            if rect.atlas_x + rect.width > atlas_width || rect.atlas_y + rect.height > atlas_height
            {
                return Err("display atlas rect exceeds atlas bounds".to_string());
            }
            let row_len = rect.width as usize * 4;
            for row in 0..rect.height {
                let src =
                    ((rect.atlas_y + row) as usize * atlas_stride) + rect.atlas_x as usize * 4;
                let dst = self.pixel_offset(rect.dst_x, rect.dst_y + row);
                self.bgra[dst..dst + row_len].copy_from_slice(&atlas_bgra[src..src + row_len]);
            }
            self.damage.push(FrameDamageRect {
                x: rect.dst_x,
                y: rect.dst_y,
                width: rect.width,
                height: rect.height,
            });
        }
        Ok(())
    }

    fn validate_rect(&self, x: u32, y: u32, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 || x + width > self.width || y + height > self.height {
            return Err("display record rect exceeds desktop bounds".to_string());
        }
        Ok(())
    }

    fn pixel_offset(&self, x: u32, y: u32) -> usize {
        ((y as usize * self.width as usize) + x as usize) * 4
    }

    fn validate_experimental_atlas(
        &self,
        atlas_width: u32,
        atlas_height: u32,
        rects: &[DisplayAtlasRect],
    ) -> Result<(), String> {
        if atlas_width == 0 || atlas_height == 0 {
            return Err("experimental atlas has zero dimensions".to_string());
        }
        for rect in rects {
            self.validate_rect(rect.dst_x, rect.dst_y, rect.width, rect.height)?;
            if rect.atlas_x.checked_add(rect.width).is_none()
                || rect.atlas_y.checked_add(rect.height).is_none()
                || rect.atlas_x + rect.width > atlas_width
                || rect.atlas_y + rect.height > atlas_height
            {
                return Err("experimental atlas rect exceeds atlas bounds".to_string());
            }
        }
        Ok(())
    }

    fn end_experimental_frame(
        &mut self,
        viewport: &ViewportArc,
        frame_id: u64,
    ) -> Result<(), String> {
        if self.experimental_chunked_frame && self.experimental_atlas.is_none() {
            if !self.experimental_moves.is_empty() {
                return Err(
                    "experimental chunked frame cannot carry trailing move rects".to_string(),
                );
            }
            return self.flush_deferred_experimental_frames(viewport);
        }
        let Some(atlas) = self.experimental_atlas.take() else {
            return Err("experimental frame ended without atlas command record".to_string());
        };
        let frame = DeferredExperimentalFrame {
            frame_id,
            desktop_width: self.width,
            desktop_height: self.height,
            atlas,
            moves: std::mem::take(&mut self.experimental_moves),
            present_after_composite: true,
        };
        self.queue_deferred_experimental_frame(frame);
        self.flush_deferred_experimental_frames(viewport)
    }

    fn queue_deferred_experimental_frame(&mut self, frame: DeferredExperimentalFrame) {
        let queued_bytes = frame.queued_bytes();
        if queued_bytes >= LARGE_EXPERIMENTAL_FRAME_BYTES {
            while self
                .deferred_experimental_frames
                .front()
                .is_some_and(|pending| pending.frame_id < frame.frame_id)
            {
                let Some(dropped) = self.deferred_experimental_frames.pop_front() else {
                    break;
                };
                self.deferred_experimental_bytes = self
                    .deferred_experimental_bytes
                    .saturating_sub(dropped.queued_bytes());
                self.experimental_summary.add_dropped_deferred();
                append_experimental_log(&format!(
                    concat!(
                        "viewer experimental deferred frame dropped frame={} ",
                        "newer_frame={} pending_frames={} pending_bytes={} reason=coalesced_newer_large_frame"
                    ),
                    dropped.frame_id,
                    frame.frame_id,
                    self.deferred_experimental_frames.len(),
                    self.deferred_experimental_bytes,
                ));
            }
        }
        self.deferred_experimental_bytes = self
            .deferred_experimental_bytes
            .saturating_add(queued_bytes);
        self.deferred_experimental_frames.push_back(frame);

        while self.deferred_experimental_frames.len() > MAX_DEFERRED_EXPERIMENTAL_FRAMES
            || self.deferred_experimental_bytes > MAX_DEFERRED_EXPERIMENTAL_BYTES
        {
            let Some(dropped) = self.deferred_experimental_frames.pop_front() else {
                self.deferred_experimental_bytes = 0;
                break;
            };
            self.deferred_experimental_bytes = self
                .deferred_experimental_bytes
                .saturating_sub(dropped.queued_bytes());
            self.experimental_summary.add_dropped_deferred();
            append_experimental_log(&format!(
                concat!(
                    "viewer experimental deferred frame dropped frame={} ",
                    "pending_frames={} pending_bytes={} reason=queue_limit"
                ),
                dropped.frame_id,
                self.deferred_experimental_frames.len(),
                self.deferred_experimental_bytes,
            ));
            warn!(
                frame_id = dropped.frame_id,
                pending_frames = self.deferred_experimental_frames.len(),
                pending_bytes = self.deferred_experimental_bytes,
                "viewer experimental deferred frame dropped because queue limit was reached"
            );
        }
    }

    pub(crate) fn has_deferred_experimental_frames(&self) -> bool {
        !self.deferred_experimental_frames.is_empty()
    }

    pub(crate) fn deferred_experimental_frame_count(&self) -> usize {
        self.deferred_experimental_frames.len()
    }

    pub(crate) fn flush_deferred_experimental_frames(
        &mut self,
        viewport: &ViewportArc,
    ) -> Result<(), String> {
        while let Some(frame) = self.deferred_experimental_frames.pop_front() {
            let queued_bytes = frame.queued_bytes();
            self.deferred_experimental_bytes = self
                .deferred_experimental_bytes
                .saturating_sub(queued_bytes);
            if let Some(reason) = self.present_deferred_experimental_frame(viewport, &frame)? {
                self.deferred_experimental_bytes = self
                    .deferred_experimental_bytes
                    .saturating_add(queued_bytes);
                self.deferred_experimental_frames.push_front(frame);
                self.experimental_summary.add_deferred(
                    self.deferred_experimental_frames.len(),
                    self.deferred_experimental_bytes,
                );
                append_experimental_log(&format!(
                    concat!(
                        "viewer experimental frame deferred frame={} reason={} ",
                        "pending_frames={} pending_bytes={}"
                    ),
                    self.deferred_experimental_frames
                        .front()
                        .map(|frame| frame.frame_id)
                        .unwrap_or_default(),
                    reason,
                    self.deferred_experimental_frames.len(),
                    self.deferred_experimental_bytes,
                ));
                debug!(
                    reason,
                    pending_frames = self.deferred_experimental_frames.len(),
                    pending_bytes = self.deferred_experimental_bytes,
                    "viewer experimental frame deferred until viewport is ready"
                );
                return Ok(());
            }
        }
        Ok(())
    }

    fn present_deferred_experimental_frame(
        &mut self,
        viewport: &ViewportArc,
        frame: &DeferredExperimentalFrame,
    ) -> Result<Option<&'static str>, String> {
        let started = Instant::now();
        let atlas_width = frame.atlas.atlas_width;
        let atlas_height = frame.atlas.atlas_height;
        let dirty_rect_count = frame.atlas.rects.len();
        let move_rect_count = frame.moves.len();
        let tile_bytes = frame.atlas.tile_commands.len();
        let record_bytes = frame.atlas.record_bytes;
        let mut guard = viewport.lock().map_err(|err| err.to_string())?;
        let result = match present_experimental_atlas_commands_gpu(
            &mut guard,
            frame.desktop_width,
            frame.desktop_height,
            atlas_width,
            atlas_height,
            &frame.atlas.rects,
            &frame.moves,
            &frame.atlas.tile_commands,
            frame.present_after_composite,
        ) {
            Ok(result) => result,
            Err(err) if is_missing_experimental_previous_error(&err) => {
                self.experimental_summary.add_dropped_deferred();
                append_experimental_log(&format!(
                    concat!(
                        "viewer experimental frame dropped frame={} reason=missing_previous ",
                        "desktop={}x{} atlas={}x{} dirty={} move={} tile_bytes={} err={}"
                    ),
                    frame.frame_id,
                    frame.desktop_width,
                    frame.desktop_height,
                    atlas_width,
                    atlas_height,
                    dirty_rect_count,
                    move_rect_count,
                    tile_bytes,
                    err,
                ));
                warn!(
                    frame_id = frame.frame_id,
                    error = %err,
                    "viewer experimental frame dropped because previous ATX2 desktop is unavailable"
                );
                return Ok(None);
            }
            Err(err) => return Err(err),
        };
        let stats = match result {
            ExperimentalPresentResult::Presented(stats) => stats,
            ExperimentalPresentResult::Deferred(reason) => return Ok(Some(reason)),
        };
        let total_elapsed = started.elapsed();
        self.experimental_summary.add(ViewerFrameSample {
            total: total_elapsed,
            validate: stats.composite.validate,
            upload: stats.composite.upload,
            decode: stats.composite.decode,
            moves: stats.composite.moves,
            dirty: stats.composite.dirty,
            present: stats.present,
            tile_bytes,
            record_bytes,
            tile_cmds: stats.composite.command_count,
            dirty_rects: dirty_rect_count,
            move_rects: move_rect_count,
        });
        append_experimental_log(&format!(
            concat!(
                "frame={} total_ms={:.3} validate_ms={:.3} upload_ms={:.3} ",
                "decode_ms={:.3} move_ms={:.3} dirty_ms={:.3} present_ms={:.3} ",
                "desktop={}x{} atlas={}x{} dirty={} move={} ",
                "tile_cmds={} tile_bytes={} record_bytes={} present_after_composite={}"
            ),
            frame.frame_id,
            duration_ms(total_elapsed),
            stats.composite.validate.as_secs_f64() * 1000.0,
            stats.composite.upload.as_secs_f64() * 1000.0,
            stats.composite.decode.as_secs_f64() * 1000.0,
            stats.composite.moves.as_secs_f64() * 1000.0,
            stats.composite.dirty.as_secs_f64() * 1000.0,
            stats.present.as_secs_f64() * 1000.0,
            frame.desktop_width,
            frame.desktop_height,
            atlas_width,
            atlas_height,
            dirty_rect_count,
            move_rect_count,
            stats.composite.command_count,
            tile_bytes,
            record_bytes,
            frame.present_after_composite,
        ));
        debug!(
            frame_id = frame.frame_id,
            desktop_width = frame.desktop_width,
            desktop_height = frame.desktop_height,
            atlas_width = atlas_width,
            atlas_height = atlas_height,
            dirty_rects = dirty_rect_count,
            move_rects = move_rect_count,
            command_count = stats.composite.command_count,
            present_after_composite = frame.present_after_composite,
            "viewer experimental frame composited"
        );
        Ok(None)
    }
}

impl Drop for ModernDisplayCompositor {
    fn drop(&mut self) {
        if self.experimental_summary_logged {
            return;
        }
        if let Some(context) = self.experimental_summary_context.take() {
            self.experimental_summary
                .log(&context.session_id, &context.transport, "forced_drop");
            self.experimental_summary_logged = true;
        }
    }
}

fn is_missing_experimental_previous_error(err: &str) -> bool {
    err.contains("before previous desktop frame")
        || err.contains("before initial desktop frame")
        || err.contains("previous desktop texture unavailable")
}

fn display_record_label(record: &DisplayRecord) -> &'static str {
    match record {
        DisplayRecord::FrameBegin { .. } => "frame_begin",
        DisplayRecord::FrameEnd { .. } => "frame_end",
        DisplayRecord::Keyframe { .. } => "keyframe",
        DisplayRecord::MoveRect { .. } => "move_rect",
        DisplayRecord::AtlasH264 { .. } => "atlas_h264",
        DisplayRecord::ExperimentalAtlasCommands { .. } => "experimental_atlas_commands",
        DisplayRecord::ExperimentalAtlasCommandsChunk { .. } => "experimental_atlas_commands_chunk",
    }
}

fn bgra_to_words(bgra: &[u8]) -> Vec<u32> {
    bgra.chunks_exact(4)
        .map(|pixel| u32::from_le_bytes([pixel[0], pixel[1], pixel[2], pixel[3]]))
        .collect()
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn avg_duration_ms(duration: Duration, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        duration_ms(duration) / count as f64
    }
}

fn avg_u64(value: u64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        value as f64 / count as f64
    }
}

pub(crate) fn append_experimental_log(line: &str) {
    let path = PathBuf::from(r"C:\ProgramData\Talos\logs\talos_viewer_experimental.log");
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "{line}")
    })();
    if let Err(err) = result {
        warn!(error = %err, "failed to append viewer experimental display log");
    }
}
