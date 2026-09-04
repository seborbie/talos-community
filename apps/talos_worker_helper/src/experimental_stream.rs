#![cfg(target_os = "windows")]

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{ensure, Context, Result};
use talos_protocol::{
    build_display_experimental_atlas_commands, build_display_experimental_atlas_commands_chunk,
    build_display_frame_begin, build_display_frame_end, build_display_move_rect, DisplayAtlasRect,
    DisplayStreamDescriptor, DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL,
    DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE, DISPLAY_STREAM_META_TYPE,
    HELPER_PIPE_HANDSHAKE_MAGIC, HELPER_PIPE_MAX_AUTH_TOKEN_LEN, HELPER_PIPE_PROTOCOL_VERSION,
};
use winapi::um::winnt::HANDLE;

use crate::{
    atlas::{copy_dirty_rects_to_compact_atlas, AtlasGpuResources, AtlasReadback},
    dxgi_capture::{CaptureEvent, DirtyRect, DxgiAtlasCapturer, MoveRect},
    tile_commands::{TileCommandCopyRect, TileCommandStream},
};

const PIPE_AUTH_TAG: u8 = 3;
const PIPE_DISPLAY_DELTA_TAG: u8 = 5;
const PIPE_LIVENESS_TAG: u8 = 6;
const PIPE_METADATA_TAG: u8 = 0;
const LIVENESS_INTERVAL: Duration = Duration::from_secs(2);
const TILE_CACHE_FULL_REFRESH_INTERVAL: Duration = Duration::from_secs(10);
const EXPERIMENTAL_ATLAS_CHUNK_TARGET_BYTES: usize = 512 * 1024;

struct PipeStreamSender {
    handle: HANDLE,
}

impl PipeStreamSender {
    fn new(handle: HANDLE) -> Self {
        Self { handle }
    }

    fn write_chunk(&self, tag: u8, payload: &[u8]) -> Result<()> {
        crate::write_pipe_all(self.handle, &[tag])?;
        crate::write_pipe_all(self.handle, &(payload.len() as u32).to_le_bytes())?;
        crate::write_pipe_all(self.handle, payload)?;
        Ok(())
    }

    fn write_metadata(&self, payload: &[u8]) -> Result<()> {
        self.write_chunk(PIPE_METADATA_TAG, payload)
    }

    fn write_display_delta(&self, payload: &[u8]) -> Result<()> {
        self.write_chunk(PIPE_DISPLAY_DELTA_TAG, payload)
    }

    fn write_liveness(&self) -> Result<()> {
        self.write_chunk(PIPE_LIVENESS_TAG, &[])
    }
}

impl Drop for PipeStreamSender {
    fn drop(&mut self) {
        unsafe {
            winapi::um::handleapi::CloseHandle(self.handle);
        }
    }
}

#[derive(Default)]
struct ExperimentalStreamSummary {
    sent_frames: u64,
    skipped_frames: u64,
    total: Duration,
    capture: Duration,
    capture_acquire_wait: Duration,
    capture_process: Duration,
    atlas: Duration,
    classify: Duration,
    classify_setup: Duration,
    classify_dispatch: Duration,
    classify_staging_copy: Duration,
    classify_map_wait: Duration,
    classify_result_parse: Duration,
    readback: Duration,
    pipe: Duration,
    skipped_total: Duration,
    skipped_capture: Duration,
    skipped_capture_acquire_wait: Duration,
    skipped_capture_process: Duration,
    skipped_atlas: Duration,
    skipped_classify: Duration,
    skipped_classify_setup: Duration,
    skipped_classify_dispatch: Duration,
    skipped_classify_staging_copy: Duration,
    skipped_classify_map_wait: Duration,
    skipped_classify_result_parse: Duration,
    max_total: Duration,
    max_capture: Duration,
    max_capture_acquire_wait: Duration,
    max_capture_process: Duration,
    max_atlas: Duration,
    max_classify: Duration,
    max_classify_map_wait: Duration,
    max_pipe: Duration,
    wire_bytes: u64,
    tile_bytes: u64,
    atlas_bytes: u64,
    tile_cmds: u64,
    solid_cmds: u64,
    raw_key_cmds: u64,
    xor_raw_cmds: u64,
    xor_sparse_cmds: u64,
    masked_quant_delta_cmds: u64,
    lossy_ui_block_cmds: u64,
    sharp_ui_block_cmds: u64,
    skipped_tile_cmds: u64,
    delta_reference_resets: u64,
    delta_bytes_saved_estimate: u64,
}

impl ExperimentalStreamSummary {
    fn add_sent(&mut self, sample: HelperFrameSample) {
        self.sent_frames = self.sent_frames.saturating_add(1);
        self.total += sample.total;
        self.capture += sample.capture;
        self.capture_acquire_wait += sample.capture_acquire_wait;
        self.capture_process += sample.capture_process;
        self.atlas += sample.atlas;
        self.classify += sample.classify;
        self.classify_setup += sample.classify_setup;
        self.classify_dispatch += sample.classify_dispatch;
        self.classify_staging_copy += sample.classify_staging_copy;
        self.classify_map_wait += sample.classify_map_wait;
        self.classify_result_parse += sample.classify_result_parse;
        self.readback += sample.readback;
        self.pipe += sample.pipe;
        self.max_total = self.max_total.max(sample.total);
        self.max_capture = self.max_capture.max(sample.capture);
        self.max_capture_acquire_wait = self
            .max_capture_acquire_wait
            .max(sample.capture_acquire_wait);
        self.max_capture_process = self.max_capture_process.max(sample.capture_process);
        self.max_atlas = self.max_atlas.max(sample.atlas);
        self.max_classify = self.max_classify.max(sample.classify);
        self.max_classify_map_wait = self.max_classify_map_wait.max(sample.classify_map_wait);
        self.max_pipe = self.max_pipe.max(sample.pipe);
        self.wire_bytes = self.wire_bytes.saturating_add(sample.wire_bytes as u64);
        self.tile_bytes = self.tile_bytes.saturating_add(sample.tile_bytes as u64);
        self.atlas_bytes = self.atlas_bytes.saturating_add(sample.atlas_bytes as u64);
        self.tile_cmds = self.tile_cmds.saturating_add(sample.tile_cmds as u64);
        self.solid_cmds = self.solid_cmds.saturating_add(sample.solid_cmds as u64);
        self.raw_key_cmds = self.raw_key_cmds.saturating_add(sample.raw_key_cmds as u64);
        self.xor_raw_cmds = self.xor_raw_cmds.saturating_add(sample.xor_raw_cmds as u64);
        self.xor_sparse_cmds = self
            .xor_sparse_cmds
            .saturating_add(sample.xor_sparse_cmds as u64);
        self.masked_quant_delta_cmds = self
            .masked_quant_delta_cmds
            .saturating_add(sample.masked_quant_delta_cmds as u64);
        self.lossy_ui_block_cmds = self
            .lossy_ui_block_cmds
            .saturating_add(sample.lossy_ui_block_cmds as u64);
        self.sharp_ui_block_cmds = self
            .sharp_ui_block_cmds
            .saturating_add(sample.sharp_ui_block_cmds as u64);
        self.skipped_tile_cmds = self
            .skipped_tile_cmds
            .saturating_add(sample.skipped_tile_cmds as u64);
        self.delta_bytes_saved_estimate = self
            .delta_bytes_saved_estimate
            .saturating_add(sample.delta_bytes_saved_estimate as u64);
    }

    fn add_delta_reference_reset(&mut self) {
        self.delta_reference_resets = self.delta_reference_resets.saturating_add(1);
    }

    fn add_skipped(
        &mut self,
        total: Duration,
        capture: Duration,
        capture_acquire_wait: Duration,
        capture_process: Duration,
        atlas: Duration,
        classify: Duration,
        classify_setup: Duration,
        classify_dispatch: Duration,
        classify_staging_copy: Duration,
        classify_map_wait: Duration,
        classify_result_parse: Duration,
    ) {
        self.skipped_frames = self.skipped_frames.saturating_add(1);
        self.skipped_total += total;
        self.skipped_capture += capture;
        self.skipped_capture_acquire_wait += capture_acquire_wait;
        self.skipped_capture_process += capture_process;
        self.skipped_atlas += atlas;
        self.skipped_classify += classify;
        self.skipped_classify_setup += classify_setup;
        self.skipped_classify_dispatch += classify_dispatch;
        self.skipped_classify_staging_copy += classify_staging_copy;
        self.skipped_classify_map_wait += classify_map_wait;
        self.skipped_classify_result_parse += classify_result_parse;
    }

    fn log(&self, pipe_name: &str, elapsed: Duration, final_frame_id: u64, error: Option<&str>) {
        let err = error.unwrap_or("none");
        append_experimental_log(&format!(
            concat!(
                "experimental stream summary pipe={} frames={} sent={} skipped={} ",
                "elapsed_ms={:.3} error={}"
            ),
            pipe_name,
            final_frame_id,
            self.sent_frames,
            self.skipped_frames,
            ms(elapsed),
            err,
        ));
        append_experimental_log(&format!(
            concat!(
                "experimental stream timing summary total_ms={:.3} avg_total_ms={:.3} max_total_ms={:.3} ",
                "capture_ms={:.3} avg_capture_ms={:.3} max_capture_ms={:.3} ",
                "capture_acquire_wait_ms={:.3} avg_capture_acquire_wait_ms={:.3} max_capture_acquire_wait_ms={:.3} ",
                "capture_process_ms={:.3} avg_capture_process_ms={:.3} max_capture_process_ms={:.3} ",
                "atlas_ms={:.3} avg_atlas_ms={:.3} max_atlas_ms={:.3} ",
                "classify_ms={:.3} avg_classify_ms={:.3} max_classify_ms={:.3} ",
                "classify_setup_ms={:.3} classify_dispatch_ms={:.3} classify_staging_copy_ms={:.3} ",
                "classify_map_wait_ms={:.3} max_classify_map_wait_ms={:.3} classify_result_parse_ms={:.3} ",
                "readback_ms={:.3} pipe_ms={:.3} avg_pipe_ms={:.3} max_pipe_ms={:.3} ",
                "skipped_total_ms={:.3} skipped_capture_ms={:.3} skipped_capture_acquire_wait_ms={:.3} ",
                "skipped_capture_process_ms={:.3} skipped_atlas_ms={:.3} skipped_classify_ms={:.3} ",
                "skipped_classify_setup_ms={:.3} skipped_classify_dispatch_ms={:.3} ",
                "skipped_classify_staging_copy_ms={:.3} skipped_classify_map_wait_ms={:.3} ",
                "skipped_classify_result_parse_ms={:.3}"
            ),
            ms(self.total),
            avg_ms(self.total, self.sent_frames),
            ms(self.max_total),
            ms(self.capture),
            avg_ms(self.capture, self.sent_frames),
            ms(self.max_capture),
            ms(self.capture_acquire_wait),
            avg_ms(self.capture_acquire_wait, self.sent_frames),
            ms(self.max_capture_acquire_wait),
            ms(self.capture_process),
            avg_ms(self.capture_process, self.sent_frames),
            ms(self.max_capture_process),
            ms(self.atlas),
            avg_ms(self.atlas, self.sent_frames),
            ms(self.max_atlas),
            ms(self.classify),
            avg_ms(self.classify, self.sent_frames),
            ms(self.max_classify),
            ms(self.classify_setup),
            ms(self.classify_dispatch),
            ms(self.classify_staging_copy),
            ms(self.classify_map_wait),
            ms(self.max_classify_map_wait),
            ms(self.classify_result_parse),
            ms(self.readback),
            ms(self.pipe),
            avg_ms(self.pipe, self.sent_frames),
            ms(self.max_pipe),
            ms(self.skipped_total),
            ms(self.skipped_capture),
            ms(self.skipped_capture_acquire_wait),
            ms(self.skipped_capture_process),
            ms(self.skipped_atlas),
            ms(self.skipped_classify),
            ms(self.skipped_classify_setup),
            ms(self.skipped_classify_dispatch),
            ms(self.skipped_classify_staging_copy),
            ms(self.skipped_classify_map_wait),
            ms(self.skipped_classify_result_parse),
        ));
        append_experimental_log(&format!(
            concat!(
                "experimental stream byte summary wire_bytes={} avg_wire_bytes={:.1} ",
                "tile_bytes={} avg_tile_bytes={:.1} atlas_bytes={} tile_cmds={} avg_tile_cmds={:.1} ",
                "solid={} raw_key={} xor_raw={} xor_sparse={} masked_quant_delta={} lossy_ui_block={} sharp_ui_block={} skipped_tiles={} ",
                "delta_reference_resets={} delta_bytes_saved_estimate={}"
            ),
            self.wire_bytes,
            avg_u64(self.wire_bytes, self.sent_frames),
            self.tile_bytes,
            avg_u64(self.tile_bytes, self.sent_frames),
            self.atlas_bytes,
            self.tile_cmds,
            avg_u64(self.tile_cmds, self.sent_frames),
            self.solid_cmds,
            self.raw_key_cmds,
            self.xor_raw_cmds,
            self.xor_sparse_cmds,
            self.masked_quant_delta_cmds,
            self.lossy_ui_block_cmds,
            self.sharp_ui_block_cmds,
            self.skipped_tile_cmds,
            self.delta_reference_resets,
            self.delta_bytes_saved_estimate,
        ));
    }
}

struct HelperFrameSample {
    total: Duration,
    capture: Duration,
    capture_acquire_wait: Duration,
    capture_process: Duration,
    atlas: Duration,
    classify: Duration,
    classify_setup: Duration,
    classify_dispatch: Duration,
    classify_staging_copy: Duration,
    classify_map_wait: Duration,
    classify_result_parse: Duration,
    readback: Duration,
    pipe: Duration,
    wire_bytes: usize,
    tile_bytes: usize,
    atlas_bytes: usize,
    tile_cmds: u32,
    solid_cmds: usize,
    raw_key_cmds: usize,
    xor_raw_cmds: usize,
    xor_sparse_cmds: usize,
    masked_quant_delta_cmds: usize,
    lossy_ui_block_cmds: usize,
    sharp_ui_block_cmds: usize,
    skipped_tile_cmds: usize,
    delta_bytes_saved_estimate: u32,
}

pub(crate) fn run_experimental_stream_to_pipe(
    pipe_name: &str,
    auth_token: &str,
    tuning: talos_worker::encode::EncodeTuning,
    fps: u32,
    stop: Arc<AtomicBool>,
    capture_output_switch_rx: mpsc::Receiver<usize>,
    stream_bitrate_rx: mpsc::Receiver<u32>,
) -> Result<()> {
    ensure!(!auth_token.trim().is_empty(), "missing pipe auth token");
    ensure!(
        auth_token.len() <= HELPER_PIPE_MAX_AUTH_TOKEN_LEN,
        "pipe auth token too long"
    );

    let handle = open_named_pipe_writer(pipe_name).context("open experimental stream pipe")?;
    let sender = PipeStreamSender::new(handle);
    let mut auth_payload = Vec::with_capacity(6 + auth_token.len());
    auth_payload.extend_from_slice(&HELPER_PIPE_HANDSHAKE_MAGIC);
    auth_payload.extend_from_slice(&HELPER_PIPE_PROTOCOL_VERSION.to_be_bytes());
    auth_payload.extend_from_slice(auth_token.as_bytes());
    sender
        .write_chunk(PIPE_AUTH_TAG, &auth_payload)
        .context("write pipe auth handshake")?;

    crate::helper_log(
        "experimental_capture_stream_start",
        Some(&format!("pipe={pipe_name} fps={fps}")),
    );
    append_experimental_log(&format!(
        "experimental stream start pipe={pipe_name} fps={fps} at={}",
        unix_ms(),
    ));

    let interval = if fps > 0 {
        Duration::from_secs_f64(1.0 / fps as f64)
    } else {
        Duration::from_millis(33)
    };
    let mut active_capture_output_index = 0usize;
    talos_worker::control::set_remote_input_capture_output_index(active_capture_output_index);
    let mut capturer = initialize_experimental_capturer(active_capture_output_index, "startup")?;
    announce_experimental_stream(
        &sender,
        tuning,
        fps,
        active_capture_output_index,
        &capturer,
        "startup",
    )?;
    let mut frame_id = 0u64;
    send_black_bootstrap_frame(
        &sender,
        &capturer,
        active_capture_output_index,
        &mut frame_id,
    )?;
    let mut atlas_resources = AtlasGpuResources::default();
    let mut tile_cache_dimensions: Option<(u32, u32)> = None;
    let mut last_tile_cache_full_refresh = Instant::now();
    let mut stream_announced = true;
    let mut force_full_frame = true;
    let mut last_liveness_sent = Instant::now();
    let stream_started = Instant::now();
    let mut summary = ExperimentalStreamSummary::default();

    let stream_result = (|| -> Result<()> {
        while !stop.load(Ordering::SeqCst) {
            let loop_started = Instant::now();
            while let Ok(index) = capture_output_switch_rx.try_recv() {
                match initialize_experimental_capturer(index, "output_switch") {
                    Ok(new_capturer) => {
                        capturer = new_capturer;
                        atlas_resources.reset();
                        tile_cache_dimensions = None;
                        active_capture_output_index = index;
                        announce_experimental_stream(
                            &sender,
                            tuning,
                            fps,
                            active_capture_output_index,
                            &capturer,
                            "output_switch",
                        )?;
                        stream_announced = true;
                        force_full_frame = true;
                        talos_worker::control::set_remote_input_capture_output_index(index);
                        crate::helper_log(
                            "experimental_capture_output_switched",
                            Some(&format!("index={index}")),
                        );
                    }
                    Err(err) => {
                        crate::helper_log(
                            "experimental_capture_output_switch_err",
                            Some(&format!("index={index} err={err}")),
                        );
                    }
                }
            }
            while stream_bitrate_rx.try_recv().is_ok() {}

            let capture_started = Instant::now();
            let frame = match capturer.capture_next() {
                Ok(CaptureEvent::Frame(frame)) => frame,
                Ok(CaptureEvent::Timeout) => {
                    send_liveness_if_due(&sender, &mut last_liveness_sent);
                    pace_loop(loop_started, interval);
                    continue;
                }
                Ok(CaptureEvent::AccessLost) => {
                    atlas_resources.reset();
                    tile_cache_dimensions = None;
                    stream_announced = false;
                    force_full_frame = true;
                    crate::helper_log("experimental_capture_access_lost", None);
                    match initialize_experimental_capturer(
                        active_capture_output_index,
                        "access_lost",
                    ) {
                        Ok(new_capturer) => {
                            capturer = new_capturer;
                            announce_experimental_stream(
                                &sender,
                                tuning,
                                fps,
                                active_capture_output_index,
                                &capturer,
                                "access_lost",
                            )?;
                            stream_announced = true;
                        }
                        Err(err) => {
                            crate::helper_log(
                                "experimental_capture_reinit_err",
                                Some(&one_line_error(&err)),
                            );
                        }
                    }
                    send_liveness_if_due(&sender, &mut last_liveness_sent);
                    pace_loop(loop_started, interval);
                    continue;
                }
                Err(err) => {
                    crate::helper_log("experimental_capture_frame_err", Some(&format!("{err}")));
                    send_liveness_if_due(&sender, &mut last_liveness_sent);
                    pace_loop(loop_started, interval);
                    continue;
                }
            };
            let capture_elapsed = capture_started.elapsed();
            let capture_acquire_wait = frame.timings.acquire_wait.min(capture_elapsed);
            let capture_process = capture_elapsed.saturating_sub(capture_acquire_wait);

            let move_rects = clip_move_rects(&frame.metadata.move_rects, frame.width, frame.height);
            if !force_full_frame && frame.metadata.dirty_rects.is_empty() && move_rects.is_empty() {
                send_liveness_if_due(&sender, &mut last_liveness_sent);
                pace_loop(loop_started, interval);
                continue;
            }

            if !stream_announced {
                let metadata = build_stream_metadata(
                    tuning,
                    fps,
                    active_capture_output_index as u32,
                    frame.width,
                    frame.height,
                )?;
                sender
                    .write_metadata(&metadata)
                    .context("write experimental display stream metadata")?;
                stream_announced = true;
            }

            frame_id = frame_id.saturating_add(1);
            let allow_delta = move_rects.is_empty();
            let dimensions = (frame.width, frame.height);
            let mut encode_full_frame = force_full_frame;
            if tile_cache_dimensions != Some(dimensions) {
                atlas_resources.reset_tile_cache();
                summary.add_delta_reference_reset();
                tile_cache_dimensions = Some(dimensions);
                last_tile_cache_full_refresh = Instant::now();
                encode_full_frame = true;
            } else if last_tile_cache_full_refresh.elapsed() >= TILE_CACHE_FULL_REFRESH_INTERVAL {
                atlas_resources.reset_tile_cache();
                summary.add_delta_reference_reset();
                last_tile_cache_full_refresh = Instant::now();
                encode_full_frame = true;
            }

            let active_dirty_rects = if encode_full_frame {
                vec![full_desktop_dirty_rect(frame.width, frame.height)]
            } else {
                frame.metadata.dirty_rects.clone()
            };

            let atlas_started = Instant::now();
            let atlas = copy_dirty_rects_to_compact_atlas(
                &mut atlas_resources,
                &frame,
                &active_dirty_rects,
                encode_full_frame,
                allow_delta,
                false,
            )
            .context("copy dirty rects to experimental atlas")?;
            force_full_frame = false;
            let atlas_elapsed = atlas_started.elapsed();

            let record_rects = atlas_display_rects(&atlas);
            let total_tiles = frame
                .width
                .div_ceil(crate::tile_commands::TILE_SIZE)
                .saturating_mul(frame.height.div_ceil(crate::tile_commands::TILE_SIZE))
                .max(1);
            let dirty_tiles = atlas.tile_commands.command_count;
            let dirty_ratio = f64::from(dirty_tiles) / f64::from(total_tiles);
            let estimated_atx2_wire_bytes =
                atlas.tile_commands.byte_len as usize + record_rects.len().saturating_mul(24) + 64;
            if atlas.tile_commands.command_count == 0 && move_rects.is_empty() {
                let skipped_total = loop_started.elapsed();
                summary.add_skipped(
                    skipped_total,
                    capture_elapsed,
                    capture_acquire_wait,
                    capture_process,
                    atlas_elapsed,
                    atlas.timings.classify_commands,
                    atlas.timings.command_setup,
                    atlas.timings.command_dispatch,
                    atlas.timings.command_staging_copy,
                    atlas.timings.command_map_wait,
                    atlas.timings.command_result_parse,
                );
                append_experimental_log(&format!(
                    concat!(
                        "frame={} skipped=unchanged_full_frame total_ms={:.3} capture_ms={:.3} ",
                        "capture_acquire_wait_ms={:.3} capture_process_ms={:.3} ",
                        "atlas_ms={:.3} classify_ms={:.3} classify_setup_ms={:.3} ",
                        "classify_dispatch_ms={:.3} classify_staging_copy_ms={:.3} ",
                        "classify_map_wait_ms={:.3} classify_result_parse_ms={:.3} ",
                        "readback_ms={:.3} desktop={}x{}"
                    ),
                    frame_id,
                    ms(skipped_total),
                    ms(capture_elapsed),
                    ms(capture_acquire_wait),
                    ms(capture_process),
                    ms(atlas_elapsed),
                    ms(atlas.timings.classify_commands),
                    ms(atlas.timings.command_setup),
                    ms(atlas.timings.command_dispatch),
                    ms(atlas.timings.command_staging_copy),
                    ms(atlas.timings.command_map_wait),
                    ms(atlas.timings.command_result_parse),
                    ms(atlas.timings.readback),
                    frame.width,
                    frame.height,
                ));
                send_liveness_if_due(&sender, &mut last_liveness_sent);
                pace_loop(loop_started, interval);
                continue;
            }

            let tile_commands = &atlas.tile_commands;
            let mut record_bytes = 0usize;
            let write_started = Instant::now();
            let frame_begin = build_display_frame_begin(frame_id, frame.width, frame.height);
            record_bytes += frame_begin.len();
            sender.write_display_delta(&frame_begin)?;

            for rect in &move_rects {
                let move_record = build_display_move_rect(
                    frame_id,
                    rect.src_x,
                    rect.src_y,
                    rect.dst_x,
                    rect.dst_y,
                    rect.width,
                    rect.height,
                );
                record_bytes += move_record.len();
                sender.write_display_delta(&move_record)?;
            }

            let atlas_chunks_sent = write_atx2_records(
                &sender,
                frame_id,
                atlas.width,
                atlas.height,
                &record_rects,
                tile_commands,
                move_rects.is_empty(),
                &mut record_bytes,
            )?;
            let emitted_lossy = tile_commands.command_counts.lossy_ui_block > 0
                || tile_commands.command_counts.sharp_ui_block > 0;
            let full_lossless_refresh = !emitted_lossy
                && move_rects.is_empty()
                && tile_commands.command_counts.skipped == 0
                && rects_cover_full_desktop(&record_rects, frame.width, frame.height);
            atlas_resources
                .commit_delta_reference(&frame, emitted_lossy, full_lossless_refresh)
                .context("commit experimental delta reference")?;

            let frame_end = build_display_frame_end(frame_id);
            record_bytes += frame_end.len();
            sender.write_display_delta(&frame_end)?;
            last_liveness_sent = Instant::now();
            let write_elapsed = write_started.elapsed();

            let total_elapsed = loop_started.elapsed();
            summary.add_sent(HelperFrameSample {
                total: total_elapsed,
                capture: capture_elapsed,
                capture_acquire_wait,
                capture_process,
                atlas: atlas_elapsed,
                classify: atlas.timings.classify_commands,
                classify_setup: atlas.timings.command_setup,
                classify_dispatch: atlas.timings.command_dispatch,
                classify_staging_copy: atlas.timings.command_staging_copy,
                classify_map_wait: atlas.timings.command_map_wait,
                classify_result_parse: atlas.timings.command_result_parse,
                readback: atlas.timings.readback,
                pipe: write_elapsed,
                wire_bytes: record_bytes,
                tile_bytes: tile_commands.byte_len as usize,
                atlas_bytes: atlas.bgra.len(),
                tile_cmds: tile_commands.command_count,
                solid_cmds: tile_commands.command_counts.solid,
                raw_key_cmds: tile_commands.command_counts.raw_key,
                xor_raw_cmds: tile_commands.command_counts.xor_raw,
                xor_sparse_cmds: tile_commands.command_counts.xor_sparse,
                masked_quant_delta_cmds: tile_commands.command_counts.masked_quant_delta,
                lossy_ui_block_cmds: tile_commands.command_counts.lossy_ui_block,
                sharp_ui_block_cmds: tile_commands.command_counts.sharp_ui_block,
                skipped_tile_cmds: tile_commands.command_counts.skipped,
                delta_bytes_saved_estimate: tile_commands.delta_bytes_saved_estimate,
            });
            append_experimental_log(&format!(
                concat!(
                    "frame={} route=atx2 total_ms={:.3} capture_ms={:.3} ",
                    "capture_acquire_wait_ms={:.3} capture_process_ms={:.3} atlas_ms={:.3} ",
                    "classify_ms={:.3} classify_setup_ms={:.3} classify_dispatch_ms={:.3} ",
                    "classify_staging_copy_ms={:.3} classify_map_wait_ms={:.3} ",
                    "classify_result_parse_ms={:.3} readback_ms={:.3} pipe_ms={:.3} ",
                    "desktop={}x{} atlas={}x{} dirty={} dirty_ratio={:.3} move={} ",
                    "tile_cmds={} tile_bytes={} atlas_bytes={} estimated_atx2_wire_bytes={} wire_bytes={} ",
                    "raw_equivalent_bytes={} delta_saved_estimate={} ",
                    "solid={} raw_key={} xor_raw={} xor_sparse={} masked_quant_delta={} lossy_ui_block={} sharp_ui_block={} skipped_tiles={} ",
                    "chunks={}"
                ),
                frame_id,
                ms(total_elapsed),
                ms(capture_elapsed),
                ms(capture_acquire_wait),
                ms(capture_process),
                ms(atlas_elapsed),
                ms(atlas.timings.classify_commands),
                ms(atlas.timings.command_setup),
                ms(atlas.timings.command_dispatch),
                ms(atlas.timings.command_staging_copy),
                ms(atlas.timings.command_map_wait),
                ms(atlas.timings.command_result_parse),
                ms(atlas.timings.readback),
                ms(write_elapsed),
                frame.width,
                frame.height,
                atlas.width,
                atlas.height,
                record_rects.len(),
                dirty_ratio,
                move_rects.len(),
                tile_commands.command_count,
                tile_commands.byte_len,
                atlas.bgra.len(),
                estimated_atx2_wire_bytes,
                record_bytes,
                tile_commands.raw_equivalent_bytes,
                tile_commands.delta_bytes_saved_estimate,
                tile_commands.command_counts.solid,
                tile_commands.command_counts.raw_key,
                tile_commands.command_counts.xor_raw,
                tile_commands.command_counts.xor_sparse,
                tile_commands.command_counts.masked_quant_delta,
                tile_commands.command_counts.lossy_ui_block,
                tile_commands.command_counts.sharp_ui_block,
                tile_commands.command_counts.skipped,
                atlas_chunks_sent,
            ));

            pace_loop(loop_started, interval);
        }
        Ok(())
    })();

    let error_detail = stream_result.as_ref().err().map(|err| one_line_error(err));
    summary.log(
        pipe_name,
        stream_started.elapsed(),
        frame_id,
        error_detail.as_deref(),
    );
    append_experimental_log(&format!(
        "experimental stream stop frames={frame_id} at={}",
        unix_ms()
    ));
    stream_result?;
    Ok(())
}

fn open_named_pipe_writer(pipe_name: &str) -> Result<HANDLE> {
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::fileapi::{CreateFileW, OPEN_EXISTING};
    use winapi::um::handleapi::INVALID_HANDLE_VALUE;
    use winapi::um::winnt::{FILE_ATTRIBUTE_NORMAL, GENERIC_WRITE};

    let pipe_wide: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();
    for _ in 0..60 {
        let handle = unsafe {
            CreateFileW(
                pipe_wide.as_ptr(),
                GENERIC_WRITE,
                0,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            return Ok(handle);
        }
        let err = unsafe { GetLastError() };
        if err == winapi::shared::winerror::ERROR_PIPE_BUSY
            || err == winapi::shared::winerror::ERROR_FILE_NOT_FOUND
        {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        anyhow::bail!("CreateFileW pipe failed: {}", err);
    }
    anyhow::bail!("timed out connecting to pipe");
}

fn build_stream_metadata(
    tuning: talos_worker::encode::EncodeTuning,
    fps: u32,
    active_capture_output_index: u32,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let descriptor = DisplayStreamDescriptor::experimental_capture(width, height);
    let mut metadata = serde_json::json!({
        "bitrate_kbps": tuning.bitrate_kbps(),
        "preset": tuning.preset.as_str(),
        "encoding_fps": fps,
        "agent_monitor_hz": null,
        "activeIndex": active_capture_output_index,
        "captureType": "experimental",
        "experimental": {
            "payload": "atx2",
            "tileCommandFormat": "ATX2",
        },
    });
    if let Some(obj) = metadata.as_object_mut() {
        obj.insert(
            DISPLAY_STREAM_META_TYPE.to_string(),
            serde_json::to_value(descriptor).context("serialize display stream descriptor")?,
        );
    }
    let json_bytes = serde_json::to_vec(&metadata).context("serialize experimental metadata")?;
    let mut msg = Vec::with_capacity(8 + json_bytes.len());
    msg.extend_from_slice(b"RMMD");
    msg.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    msg.extend_from_slice(&json_bytes);
    Ok(msg)
}

fn announce_experimental_stream(
    sender: &PipeStreamSender,
    tuning: talos_worker::encode::EncodeTuning,
    fps: u32,
    active_capture_output_index: usize,
    capturer: &DxgiAtlasCapturer,
    reason: &str,
) -> Result<()> {
    let (width, height) = capturer
        .desktop_dimensions()
        .context("read experimental desktop dimensions")?;
    let metadata = build_stream_metadata(
        tuning,
        fps,
        active_capture_output_index as u32,
        width,
        height,
    )?;
    sender
        .write_metadata(&metadata)
        .context("write experimental display stream metadata")?;
    crate::helper_log(
        "experimental_stream_metadata_sent",
        Some(&format!(
            "reason={} output={} desktop={}x{}",
            reason, active_capture_output_index, width, height
        )),
    );
    append_experimental_log(&format!(
        "experimental metadata sent reason={} output={} desktop={}x{} at={}",
        reason,
        active_capture_output_index,
        width,
        height,
        unix_ms()
    ));
    Ok(())
}

fn send_black_bootstrap_frame(
    sender: &PipeStreamSender,
    capturer: &DxgiAtlasCapturer,
    active_capture_output_index: usize,
    frame_id: &mut u64,
) -> Result<()> {
    let (width, height) = capturer
        .desktop_dimensions()
        .context("read experimental bootstrap dimensions")?;
    *frame_id = frame_id.saturating_add(1);
    let tile_commands = TileCommandStream::solid_color(width, height, [0, 0, 0, 0xff])
        .context("build experimental black bootstrap tile commands")?;
    let rects = vec![DisplayAtlasRect {
        dst_x: 0,
        dst_y: 0,
        atlas_x: 0,
        atlas_y: 0,
        width,
        height,
    }];
    let frame_begin = build_display_frame_begin(*frame_id, width, height);
    sender
        .write_display_delta(&frame_begin)
        .context("write experimental black bootstrap frame begin")?;
    let atlas_record = build_display_experimental_atlas_commands(
        *frame_id,
        width,
        height,
        &rects,
        &tile_commands.bytes,
    );
    sender
        .write_display_delta(&atlas_record)
        .context("write experimental black bootstrap atlas")?;
    let frame_end = build_display_frame_end(*frame_id);
    sender
        .write_display_delta(&frame_end)
        .context("write experimental black bootstrap frame end")?;
    crate::helper_log(
        "experimental_black_bootstrap_sent",
        Some(&format!(
            "frame={} output={} desktop={}x{} tile_bytes={} atlas_bytes={}",
            *frame_id, active_capture_output_index, width, height, tile_commands.byte_len, 0
        )),
    );
    append_experimental_log(&format!(
        "experimental black bootstrap frame={} output={} desktop={}x{} tile_bytes={} atlas_bytes={} at={}",
        *frame_id,
        active_capture_output_index,
        width,
        height,
        tile_commands.byte_len,
        0,
        unix_ms()
    ));
    Ok(())
}

#[derive(Clone, Copy)]
struct ClippedMoveRect {
    src_x: u32,
    src_y: u32,
    dst_x: u32,
    dst_y: u32,
    width: u32,
    height: u32,
}

fn clip_move_rects(
    rects: &[MoveRect],
    desktop_width: u32,
    desktop_height: u32,
) -> Vec<ClippedMoveRect> {
    rects
        .iter()
        .filter_map(|rect| clip_move_rect(*rect, desktop_width, desktop_height))
        .collect()
}

fn clip_move_rect(
    rect: MoveRect,
    desktop_width: u32,
    desktop_height: u32,
) -> Option<ClippedMoveRect> {
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return None;
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if rect.right > desktop_width
        || rect.bottom > desktop_height
        || rect.source_x.checked_add(width)? > desktop_width
        || rect.source_y.checked_add(height)? > desktop_height
    {
        return None;
    }
    Some(ClippedMoveRect {
        src_x: rect.source_x,
        src_y: rect.source_y,
        dst_x: rect.left,
        dst_y: rect.top,
        width,
        height,
    })
}

fn atlas_display_rects(atlas: &AtlasReadback) -> Vec<DisplayAtlasRect> {
    atlas
        .dirty_rects
        .iter()
        .map(|rect| DisplayAtlasRect {
            dst_x: rect.desktop.x,
            dst_y: rect.desktop.y,
            width: rect.desktop.w,
            height: rect.desktop.h,
            atlas_x: rect.atlas.x,
            atlas_y: rect.atlas.y,
        })
        .collect()
}

fn display_rects_from_copy_rects(rects: &[TileCommandCopyRect]) -> Vec<DisplayAtlasRect> {
    rects
        .iter()
        .map(|rect| DisplayAtlasRect {
            dst_x: rect.desktop_x,
            dst_y: rect.desktop_y,
            width: rect.width,
            height: rect.height,
            atlas_x: rect.atlas_x,
            atlas_y: rect.atlas_y,
        })
        .collect()
}

fn rects_cover_full_desktop(rects: &[DisplayAtlasRect], width: u32, height: u32) -> bool {
    if rects.is_empty() || width == 0 || height == 0 {
        return false;
    }
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut area = 0u64;
    for rect in rects {
        let Some(right) = rect.dst_x.checked_add(rect.width) else {
            return false;
        };
        let Some(bottom) = rect.dst_y.checked_add(rect.height) else {
            return false;
        };
        if right > width || bottom > height {
            return false;
        }
        min_x = min_x.min(rect.dst_x);
        min_y = min_y.min(rect.dst_y);
        max_x = max_x.max(right);
        max_y = max_y.max(bottom);
        area = area.saturating_add(u64::from(rect.width) * u64::from(rect.height));
    }
    min_x == 0
        && min_y == 0
        && max_x == width
        && max_y == height
        && area >= u64::from(width) * u64::from(height)
}

fn full_desktop_dirty_rect(width: u32, height: u32) -> DirtyRect {
    DirtyRect {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    }
}

fn write_atx2_records(
    sender: &PipeStreamSender,
    frame_id: u64,
    atlas_width: u32,
    atlas_height: u32,
    rects: &[DisplayAtlasRect],
    tile_commands: &TileCommandStream,
    allow_progressive_chunks: bool,
    record_bytes: &mut usize,
) -> Result<usize> {
    if tile_commands.bytes.is_empty() {
        return Ok(0);
    }
    if tile_commands.bytes.len() <= EXPERIMENTAL_ATLAS_CHUNK_TARGET_BYTES {
        let record = build_display_experimental_atlas_commands(
            frame_id,
            atlas_width,
            atlas_height,
            rects,
            &tile_commands.bytes,
        );
        *record_bytes = (*record_bytes).saturating_add(record.len());
        sender.write_display_delta(&record)?;
        return Ok(1);
    }

    let chunks = tile_commands
        .wire_chunks(EXPERIMENTAL_ATLAS_CHUNK_TARGET_BYTES)
        .context("split experimental ATX2 command stream into wire chunks")?;
    let chunk_count = chunks.len();
    for (index, chunk) in chunks.iter().enumerate() {
        let mut flags = if allow_progressive_chunks {
            DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_PROGRESSIVE
        } else {
            0
        };
        if index + 1 == chunk_count {
            flags |= DISPLAY_EXPERIMENTAL_ATLAS_CHUNK_FLAG_FINAL;
        }
        let chunk_rects = display_rects_from_copy_rects(&chunk.copy_rects);
        let chunk_rects = if chunk_rects.is_empty() {
            rects
        } else {
            chunk_rects.as_slice()
        };
        let record = build_display_experimental_atlas_commands_chunk(
            frame_id,
            flags,
            index as u32,
            chunk_count as u32,
            atlas_width,
            atlas_height,
            chunk_rects,
            &chunk.bytes,
        );
        *record_bytes = (*record_bytes).saturating_add(record.len());
        sender.write_display_delta(&record)?;
    }
    Ok(chunk_count)
}

fn pace_loop(loop_started: Instant, interval: Duration) {
    let elapsed = loop_started.elapsed();
    if elapsed < interval {
        std::thread::sleep(interval - elapsed);
    }
}

fn send_liveness_if_due(sender: &PipeStreamSender, last_sent: &mut Instant) {
    if last_sent.elapsed() < LIVENESS_INTERVAL {
        return;
    }
    if sender.write_liveness().is_ok() {
        *last_sent = Instant::now();
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn avg_ms(duration: Duration, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        ms(duration) / count as f64
    }
}

fn avg_u64(value: u64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        value as f64 / count as f64
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn experimental_log_path() -> PathBuf {
    PathBuf::from(r"C:\ProgramData\Talos\logs\talos_worker_helper_experimental.log")
}

fn append_experimental_log(line: &str) {
    let path = experimental_log_path();
    let _ = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "{line}")
    })();
}

fn initialize_experimental_capturer(
    active_capture_output_index: usize,
    reason: &str,
) -> Result<DxgiAtlasCapturer> {
    attach_experimental_capture_desktop(reason);
    match DxgiAtlasCapturer::new(active_capture_output_index)
        .context("initialize experimental dxgi capturer")
    {
        Ok(capturer) => Ok(capturer),
        Err(err) => {
            let err_detail = one_line_error(&err);
            crate::helper_log("experimental_dxgi_init_err", Some(&err_detail));
            append_experimental_log(&format!(
                "experimental dxgi init failed reason={} output={} err={} at={}",
                reason,
                active_capture_output_index,
                err_detail,
                unix_ms()
            ));
            Err(err)
        }
    }
}

fn attach_experimental_capture_desktop(reason: &str) {
    let before =
        talos_worker::control::input_desktop_name().unwrap_or_else(|| "<unknown>".to_string());
    let attached = talos_worker::control::attach_thread_to_input_desktop();
    let after =
        talos_worker::control::input_desktop_name().unwrap_or_else(|| "<unknown>".to_string());
    crate::helper_log(
        "experimental_capture_desktop_attach",
        Some(&format!(
            "reason={} attached={} input_before={} input_after={}",
            reason, attached, before, after
        )),
    );
    append_experimental_log(&format!(
        "experimental desktop attach reason={} attached={} input_before={} input_after={} at={}",
        reason,
        attached,
        before,
        after,
        unix_ms()
    ));
}

fn one_line_error(err: &anyhow::Error) -> String {
    format!("{err:#}").replace('\r', " ").replace('\n', " | ")
}
