#![cfg(target_os = "windows")]

use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use crate::{
    atlas::{
        copy_dirty_rects_to_compact_atlas, format_label, readback_full_frame, AtlasGpuResources,
    },
    dump::{self, DumpFrameMetadata, DumpMoveRect, RectU32},
    dxgi_capture::{CaptureEvent, DxgiAtlasCapturer, MoveRect},
    replay::ReplayState,
};

#[derive(Clone, Debug)]
pub(crate) struct DumpOptions {
    pub output: PathBuf,
    pub frames: u64,
    pub interval: Duration,
    pub full_frame_validation: bool,
    pub capture_output_index: usize,
}

impl DumpOptions {
    pub(crate) fn parse(args: &[String]) -> Result<Self> {
        let mut output = None;
        let mut frames = 60u64;
        let mut fps = Some(30u32);
        let mut interval_ms = None;
        let mut full_frame_validation = false;
        let mut capture_output_index = 0usize;
        let mut i = 0usize;
        while i < args.len() {
            match args[i].as_str() {
                "--output" => {
                    i += 1;
                    output = args.get(i).map(PathBuf::from);
                }
                "--frames" => {
                    i += 1;
                    let value = args.get(i).context("missing --frames value")?;
                    frames = value.parse().context("invalid --frames value")?;
                }
                "--fps" => {
                    i += 1;
                    let value = args.get(i).context("missing --fps value")?;
                    fps = Some(value.parse().context("invalid --fps value")?);
                }
                "--interval-ms" => {
                    i += 1;
                    let value = args.get(i).context("missing --interval-ms value")?;
                    interval_ms = Some(
                        value
                            .parse::<u64>()
                            .context("invalid --interval-ms value")?,
                    );
                }
                "--full-frame-validation" => {
                    full_frame_validation = true;
                }
                "--capture-output-index" => {
                    i += 1;
                    let value = args
                        .get(i)
                        .context("missing --capture-output-index value")?;
                    capture_output_index = value
                        .parse()
                        .context("invalid --capture-output-index value")?;
                }
                "--help" | "-h" => {
                    anyhow::bail!("{}", usage());
                }
                other => anyhow::bail!(
                    "unknown capture-dxgi-atlas-dump argument: {other}\n{}",
                    usage()
                ),
            }
            i += 1;
        }
        let output = output.context("missing --output argument")?;
        let interval = interval_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs_f64(1.0 / fps.unwrap_or(30).max(1) as f64));
        anyhow::ensure!(frames > 0, "--frames must be greater than zero");
        Ok(Self {
            output,
            frames,
            interval,
            full_frame_validation,
            capture_output_index,
        })
    }
}

pub(crate) fn run(options: DumpOptions) -> Result<()> {
    info!(
        target: "talos_worker_helper",
        output = %options.output.display(),
        frames = options.frames,
        interval_ms = options.interval.as_millis(),
        full_frame_validation = options.full_frame_validation,
        capture_output_index = options.capture_output_index,
        "dxgi atlas dump starting"
    );
    fs::create_dir_all(&options.output)
        .with_context(|| format!("create {}", options.output.display()))?;
    let mut capturer = DxgiAtlasCapturer::new(options.capture_output_index)
        .context("initialize DXGI atlas capturer")?;
    let mut atlas_resources = AtlasGpuResources::default();
    let mut replay = ReplayState::new();
    let mut frame_id = 1u64;
    let mut frame_perf = Vec::with_capacity(options.frames as usize);

    while frame_id <= options.frames {
        let loop_started = Instant::now();
        let capture_started = Instant::now();
        let capture_event = capturer.capture_next();
        let capture_elapsed = capture_started.elapsed();
        match capture_event {
            Ok(CaptureEvent::Frame(frame)) => {
                let frame_started = Instant::now();
                let force_full_frame = frame_id == 1;
                let dirty_rects = if force_full_frame {
                    Vec::new()
                } else {
                    frame.metadata.dirty_rects.clone()
                };
                let atlas = copy_dirty_rects_to_compact_atlas(
                    &mut atlas_resources,
                    &frame,
                    &dirty_rects,
                    force_full_frame,
                    false,
                    true,
                )
                .context("copy dirty rects to compact GPU atlas")?;
                let validation_started = Instant::now();
                let validation_bgra = if options.full_frame_validation {
                    Some(readback_full_frame(&frame).context("read back validation full frame")?)
                } else {
                    None
                };
                let validation_elapsed = validation_started.elapsed();
                let metadata_started = Instant::now();
                let metadata = DumpFrameMetadata {
                    frame_id,
                    timestamp_unix_ms: timestamp_unix_ms(),
                    desktop_width: frame.width,
                    desktop_height: frame.height,
                    atlas_width: atlas.width,
                    atlas_height: atlas.height,
                    pixel_format: format_label(frame.format).to_string(),
                    accumulated_frames: frame.metadata.accumulated_frames,
                    rects_coalesced: frame.metadata.rects_coalesced,
                    dirty_rects: atlas.dirty_rects.clone(),
                    move_rects: if force_full_frame {
                        Vec::new()
                    } else {
                        build_dump_move_rects(&frame.metadata.move_rects, frame.width, frame.height)
                    },
                    fallback_reason: atlas.fallback_reason.clone(),
                };
                let metadata_elapsed = metadata_started.elapsed();
                let frame_dir = options.output.join(format!("frame_{frame_id:06}"));
                let mkdir_started = Instant::now();
                fs::create_dir_all(&frame_dir)
                    .with_context(|| format!("create {}", frame_dir.display()))?;
                let mkdir_elapsed = mkdir_started.elapsed();

                let write_atlas_bgra_started = Instant::now();
                dump::write_bgra(&frame_dir.join("atlas.bgra"), &atlas.bgra)?;
                let write_atlas_bgra_elapsed = write_atlas_bgra_started.elapsed();

                let write_atlas_bmp_started = Instant::now();
                dump::write_bmp(
                    &frame_dir.join("atlas.bmp"),
                    atlas.width,
                    atlas.height,
                    &atlas.bgra,
                )?;
                let write_atlas_bmp_elapsed = write_atlas_bmp_started.elapsed();

                let write_tile_commands_started = Instant::now();
                dump::write_bgra(
                    &frame_dir.join("tile_commands.bin"),
                    &atlas.tile_commands.bytes,
                )?;
                let write_tile_commands_elapsed = write_tile_commands_started.elapsed();

                let write_validation_started = Instant::now();
                if let Some(full_frame) = validation_bgra.as_ref() {
                    dump::write_bgra(&frame_dir.join("optional_full_frame.bgra"), full_frame)?;
                    dump::write_bmp(
                        &frame_dir.join("optional_full_frame.bmp"),
                        frame.width,
                        frame.height,
                        full_frame,
                    )?;
                }
                let write_validation_elapsed = write_validation_started.elapsed();

                let write_metadata_started = Instant::now();
                dump::write_metadata(&frame_dir.join("metadata.json"), &metadata)?;
                let write_metadata_elapsed = write_metadata_started.elapsed();

                let replay_started = Instant::now();
                replay.replay_frame(
                    &frame_dir,
                    &metadata,
                    &atlas.tile_commands,
                    validation_bgra.as_deref(),
                )?;
                let replay_elapsed = replay_started.elapsed();
                let frame_elapsed = frame_started.elapsed();
                let loop_elapsed = loop_started.elapsed();
                let command_counts = CommandCounts {
                    solid: atlas.tile_commands.command_counts.solid,
                    raw_key: atlas.tile_commands.command_counts.raw_key,
                    xor_raw: atlas.tile_commands.command_counts.xor_raw,
                    xor_sparse: atlas.tile_commands.command_counts.xor_sparse,
                    masked_quant_delta: atlas.tile_commands.command_counts.masked_quant_delta,
                    lossy_ui_block: atlas.tile_commands.command_counts.lossy_ui_block,
                    sharp_ui_block: atlas.tile_commands.command_counts.sharp_ui_block,
                    skipped: atlas.tile_commands.command_counts.skipped,
                };
                let perf = FramePerf {
                    frame_id,
                    atlas_width: metadata.atlas_width,
                    atlas_height: metadata.atlas_height,
                    dirty_rects: metadata.dirty_rects.len(),
                    move_rects: metadata.move_rects.len(),
                    commands_before: atlas.tile_commands.descriptor_count(),
                    commands_after: atlas.tile_commands.command_count,
                    command_bytes: atlas.tile_commands.byte_len,
                    command_counts,
                    timings: FrameTimings {
                        total: frame_elapsed,
                        capture: capture_elapsed,
                        atlas_total: atlas.timings.total(),
                        rects: atlas.timings.rect_prepare,
                        pack: atlas.timings.pack,
                        ensure_textures: atlas.timings.ensure_textures,
                        clear: atlas.timings.clear,
                        gpu_copy: atlas.timings.gpu_copy_dirty,
                        readback: atlas.timings.readback,
                        classify: atlas.timings.classify_commands,
                        validation: validation_elapsed,
                        metadata: metadata_elapsed,
                        mkdir: mkdir_elapsed,
                        write_atlas_bgra: write_atlas_bgra_elapsed,
                        write_atlas_bmp: write_atlas_bmp_elapsed,
                        write_commands: write_tile_commands_elapsed,
                        write_validation: write_validation_elapsed,
                        write_metadata: write_metadata_elapsed,
                        replay: replay_elapsed,
                        loop_total: loop_elapsed,
                    },
                };
                print_frame_progress(&perf);
                frame_perf.push(perf);
                info!(
                    target: "talos_worker_helper",
                    frame_id,
                    dirty_rects = metadata.dirty_rects.len(),
                    move_rects = metadata.move_rects.len(),
                    atlas_width = metadata.atlas_width,
                    atlas_height = metadata.atlas_height,
                    dir = %frame_dir.display(),
                    "dxgi atlas dump frame written"
                );
                frame_id = frame_id.saturating_add(1);
            }
            Ok(CaptureEvent::Timeout) => {
                eprintln!(
                    "capture_dump_perf timeout capture_ms={:.3}",
                    duration_ms(capture_elapsed)
                );
                debug!("dxgi atlas dump capture timeout");
            }
            Ok(CaptureEvent::AccessLost) => {
                warn!("dxgi atlas dump access lost; recreating capturer");
                atlas_resources.reset();
                capturer = DxgiAtlasCapturer::new(options.capture_output_index)
                    .context("reinitialize DXGI atlas capturer after access lost")?;
            }
            Err(err) => return Err(err),
        }
        let elapsed = loop_started.elapsed();
        if elapsed < options.interval {
            thread::sleep(options.interval - elapsed);
        }
    }
    info!(
        target: "talos_worker_helper",
        frames_written = options.frames,
        output = %options.output.display(),
        "dxgi atlas dump finished"
    );
    print_perf_summary(&frame_perf);
    Ok(())
}

#[derive(Clone, Copy, Default)]
struct CommandCounts {
    solid: usize,
    raw_key: usize,
    xor_raw: usize,
    xor_sparse: usize,
    masked_quant_delta: usize,
    lossy_ui_block: usize,
    sharp_ui_block: usize,
    skipped: usize,
}

impl CommandCounts {
    fn add_assign(&mut self, other: CommandCounts) {
        self.solid += other.solid;
        self.raw_key += other.raw_key;
        self.xor_raw += other.xor_raw;
        self.xor_sparse += other.xor_sparse;
        self.masked_quant_delta += other.masked_quant_delta;
        self.lossy_ui_block += other.lossy_ui_block;
        self.sharp_ui_block += other.sharp_ui_block;
        self.skipped += other.skipped;
    }
}

#[derive(Clone, Copy, Default)]
struct FrameTimings {
    total: Duration,
    capture: Duration,
    atlas_total: Duration,
    rects: Duration,
    pack: Duration,
    ensure_textures: Duration,
    clear: Duration,
    gpu_copy: Duration,
    readback: Duration,
    classify: Duration,
    validation: Duration,
    metadata: Duration,
    mkdir: Duration,
    write_atlas_bgra: Duration,
    write_atlas_bmp: Duration,
    write_commands: Duration,
    write_validation: Duration,
    write_metadata: Duration,
    replay: Duration,
    loop_total: Duration,
}

impl FrameTimings {
    fn add_assign(&mut self, other: FrameTimings) {
        self.total += other.total;
        self.capture += other.capture;
        self.atlas_total += other.atlas_total;
        self.rects += other.rects;
        self.pack += other.pack;
        self.ensure_textures += other.ensure_textures;
        self.clear += other.clear;
        self.gpu_copy += other.gpu_copy;
        self.readback += other.readback;
        self.classify += other.classify;
        self.validation += other.validation;
        self.metadata += other.metadata;
        self.mkdir += other.mkdir;
        self.write_atlas_bgra += other.write_atlas_bgra;
        self.write_atlas_bmp += other.write_atlas_bmp;
        self.write_commands += other.write_commands;
        self.write_validation += other.write_validation;
        self.write_metadata += other.write_metadata;
        self.replay += other.replay;
        self.loop_total += other.loop_total;
    }

    fn disk_write_total(self) -> Duration {
        self.write_atlas_bgra
            + self.write_atlas_bmp
            + self.write_commands
            + self.write_validation
            + self.write_metadata
    }
}

struct FramePerf {
    frame_id: u64,
    atlas_width: u32,
    atlas_height: u32,
    dirty_rects: usize,
    move_rects: usize,
    commands_before: u32,
    commands_after: u32,
    command_bytes: u32,
    command_counts: CommandCounts,
    timings: FrameTimings,
}

fn print_frame_progress(perf: &FramePerf) {
    eprintln!(
        concat!(
            "Frame {frame:>3}: {total:>8} total | atlas {atlas:>8} | classify {classify:>8} | ",
            "write {write:>8} | replay {replay:>8} | {atlas_width}x{atlas_height}, ",
            "{dirty} dirty, {move_rects} move, commands {before} -> {after}, bin {bytes}"
        ),
        frame = perf.frame_id,
        total = format_duration(perf.timings.total),
        atlas = format_duration(perf.timings.atlas_total),
        classify = format_duration(perf.timings.classify),
        write = format_duration(perf.timings.disk_write_total()),
        replay = format_duration(perf.timings.replay),
        atlas_width = perf.atlas_width,
        atlas_height = perf.atlas_height,
        dirty = perf.dirty_rects,
        move_rects = perf.move_rects,
        before = perf.commands_before,
        after = perf.commands_after,
        bytes = format_bytes(u64::from(perf.command_bytes)),
    );
}

fn print_perf_summary(frames: &[FramePerf]) {
    if frames.is_empty() {
        eprintln!();
        eprintln!("Capture dump performance summary");
        eprintln!("  No frames were written.");
        return;
    }

    let mut totals = FrameTimings::default();
    let mut command_totals = CommandCounts::default();
    let mut commands_before = 0u64;
    let mut commands_after = 0u64;
    let mut command_bytes = 0u64;
    for frame in frames {
        totals.add_assign(frame.timings);
        command_totals.add_assign(frame.command_counts);
        commands_before += u64::from(frame.commands_before);
        commands_after += u64::from(frame.commands_after);
        command_bytes += u64::from(frame.command_bytes);
    }

    let slowest = frames
        .iter()
        .max_by_key(|frame| frame.timings.total)
        .expect("frames is not empty");
    let quickest = frames
        .iter()
        .min_by_key(|frame| frame.timings.total)
        .expect("frames is not empty");
    let delta = slowest
        .timings
        .total
        .checked_sub(quickest.timings.total)
        .unwrap_or_default();
    let frame_count = frames.len() as f64;

    eprintln!();
    eprintln!("Capture dump performance summary");
    eprintln!("================================");
    eprintln!("Frames written: {}", frames.len());
    eprintln!(
        "Total processing time: {} ({:.3} ms avg/frame)",
        format_duration(totals.total),
        duration_ms(totals.total) / frame_count
    );
    eprintln!(
        "Loop wall time: {} ({:.3} ms avg/frame)",
        format_duration(totals.loop_total),
        duration_ms(totals.loop_total) / frame_count
    );
    eprintln!(
        "Slowest frame: #{} at {}",
        slowest.frame_id,
        format_duration(slowest.timings.total)
    );
    eprintln!(
        "Quickest frame: #{} at {}",
        quickest.frame_id,
        format_duration(quickest.timings.total)
    );
    eprintln!("Slowest/quickest delta: {}", format_duration(delta));
    eprintln!();
    eprintln!("Component totals");
    print_component("DXGI capture", totals.capture, totals.total);
    print_component("Atlas total", totals.atlas_total, totals.total);
    print_component("  Rect prep", totals.rects, totals.total);
    print_component("  Pack", totals.pack, totals.total);
    print_component("  Ensure textures", totals.ensure_textures, totals.total);
    print_component("  Clear atlas", totals.clear, totals.total);
    print_component("  GPU dirty copy", totals.gpu_copy, totals.total);
    print_component("  GPU readback", totals.readback, totals.total);
    print_component("  Command classify/payloads", totals.classify, totals.total);
    print_component("Validation readback", totals.validation, totals.total);
    print_component("Metadata build", totals.metadata, totals.total);
    print_component("Directory create", totals.mkdir, totals.total);
    print_component("Disk writes total", totals.disk_write_total(), totals.total);
    print_component("  atlas.bgra", totals.write_atlas_bgra, totals.total);
    print_component("  atlas.bmp", totals.write_atlas_bmp, totals.total);
    print_component("  tile_commands.bin", totals.write_commands, totals.total);
    print_component(
        "  optional full frame",
        totals.write_validation,
        totals.total,
    );
    print_component("  metadata.json", totals.write_metadata, totals.total);
    print_component("Replay/reconstruct", totals.replay, totals.total);
    eprintln!();
    eprintln!("Command totals");
    eprintln!(
        "  Descriptors/commands: {} -> {} ({:.2}% reduction)",
        commands_before,
        commands_after,
        reduction_percent(commands_before, commands_after)
    );
    eprintln!("  Binary command bytes: {}", format_bytes(command_bytes));
    eprintln!(
        "  solid={} raw_key={} xor_raw={} xor_sparse={} masked_quant_delta={} lossy_ui_block={} sharp_ui_block={} skipped={}",
        command_totals.solid,
        command_totals.raw_key,
        command_totals.xor_raw,
        command_totals.xor_sparse,
        command_totals.masked_quant_delta,
        command_totals.lossy_ui_block,
        command_totals.sharp_ui_block,
        command_totals.skipped
    );
}

fn print_component(label: &str, duration: Duration, total: Duration) {
    eprintln!(
        "  {label:<25} {:>10} {:>6.2}%",
        format_duration(duration),
        percent_of(duration, total)
    );
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn format_duration(duration: Duration) -> String {
    let millis = duration_ms(duration);
    if millis >= 1000.0 {
        format!("{:.3}s", millis / 1000.0)
    } else {
        format!("{millis:.3}ms")
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    if bytes as f64 >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB)
    } else if bytes as f64 >= KIB {
        format!("{:.2} KiB", bytes as f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn percent_of(value: Duration, total: Duration) -> f64 {
    let total_ms = duration_ms(total);
    if total_ms <= f64::EPSILON {
        0.0
    } else {
        duration_ms(value) * 100.0 / total_ms
    }
}

fn reduction_percent(before: u64, after: u64) -> f64 {
    if before == 0 {
        0.0
    } else {
        100.0 * (1.0 - after as f64 / before as f64)
    }
}

fn usage() -> &'static str {
    "usage: talos_worker_helper.exe capture-dxgi-atlas-dump --output <dir> [--frames <n>] [--fps <n>|--interval-ms <n>] [--full-frame-validation] [--capture-output-index <n>]"
}

fn timestamp_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn build_dump_move_rects(move_rects: &[MoveRect], width: u32, height: u32) -> Vec<DumpMoveRect> {
    move_rects
        .iter()
        .filter_map(|rect| clip_move_rect(*rect, width, height))
        .collect()
}

fn clip_move_rect(rect: MoveRect, frame_width: u32, frame_height: u32) -> Option<DumpMoveRect> {
    let width = rect.right.checked_sub(rect.left)?;
    let height = rect.bottom.checked_sub(rect.top)?;
    if width == 0 || height == 0 {
        return None;
    }
    if rect.left.checked_add(width)? > frame_width
        || rect.top.checked_add(height)? > frame_height
        || rect.source_x.checked_add(width)? > frame_width
        || rect.source_y.checked_add(height)? > frame_height
    {
        return None;
    }
    Some(DumpMoveRect {
        src: RectU32 {
            x: rect.source_x,
            y: rect.source_y,
            w: width,
            h: height,
        },
        dst: RectU32 {
            x: rect.left,
            y: rect.top,
            w: width,
            h: height,
        },
    })
}
