//! Shared Talos file logging helpers.
//!
//! The **active** log is always the template path (e.g. `talos_worker.log`, `talos_viewer.log`).
//! After local midnight (or when a stale file is found on startup), that file is **zipped** to
//! `name_DD-MM-YYYY.zip` (UK date). A new empty template file is used for the new day. The zip
//! contains a single member named `name_DD-MM-YYYY.log` for clarity when extracted.
//!
//! Legacy dated plain logs (`name_DD-MM-YYYY.log` from older builds) are zipped on maintenance
//! when their calendar date is before today.
//!
//! Zip archives older than 45 days (by the UK date in the filename) are deleted during the same
//! maintenance pass as midnight rotation.
//!
//! If the target archive name already exists, that **existing** file is renamed to `name_DD-MM-YYYY_1.zip`,
//! `_2`, etc., so the freshly rotated log keeps the canonical dated name.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration as StdDuration, SystemTime};

use chrono::{DateTime, Duration, Local, NaiveDate};
use tracing_subscriber::fmt::MakeWriter;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const SECS_PER_DAY: u64 = 86_400;
const RETENTION_DAYS: i64 = 45;
const MAINTENANCE_POLL_SECS: u64 = 60;

/// Format a local calendar date as `DD-MM-YYYY` (UK general).
pub fn format_uk_date(date: NaiveDate) -> String {
    date.format("%d-%m-%Y").to_string()
}

/// Path for a rotated zip archive: `dir/{stem}_{DD-MM-YYYY}.zip` for template `dir/{stem}.{ext}`.
pub fn archived_zip_path(template: &Path, date: NaiveDate) -> Option<PathBuf> {
    let dir = template.parent()?;
    let stem = template.file_stem()?.to_str()?;
    Some(dir.join(format!("{}_{}.zip", stem, format_uk_date(date))))
}

/// Inner member name inside the zip (matches the former plain archive filename).
fn archived_member_name(template: &Path, date: NaiveDate) -> Option<String> {
    let stem = template.file_stem()?.to_str()?;
    let ext = template.extension()?.to_str()?;
    Some(format!("{}_{}.{}", stem, format_uk_date(date), ext))
}

/// Logical dated log filename (same as the single member name inside `archived_zip_path`'s zip).
pub fn archived_log_path(template: &Path, date: NaiveDate) -> Option<PathBuf> {
    let dir = template.parent()?;
    let stem = template.file_stem()?.to_str()?;
    let ext = template.extension()?.to_str()?;
    Some(dir.join(format!("{}_{}.{}", stem, format_uk_date(date), ext)))
}

fn local_date_from_system_time(ts: SystemTime) -> Option<NaiveDate> {
    let secs = ts.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs() as i64;
    let dt = DateTime::from_timestamp(secs, 0)?;
    Some(dt.with_timezone(&Local).date_naive())
}

fn today_local() -> NaiveDate {
    Local::now().date_naive()
}

/// If `path` exists, rename it to `{same_stem}_1.{ext}`, `{same_stem}_2.{ext}`, … (first free).
/// Used so the **new** rotation can take the canonical `stem_DD-MM-YYYY.zip` name.
fn relocate_existing_file(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "archive path has no parent"))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "archive path has no stem"))?;
    let ext = path.extension().and_then(|s| s.to_str()).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "archive path has no extension")
    })?;
    for n in 1u32..100_000 {
        let bumped = dir.join(format!("{stem}_{n}.{ext}"));
        if !bumped.exists() {
            fs::rename(path, bumped)?;
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not relocate existing archive for collision",
    ))
}

fn zip_plain_file_to_archive(
    src: &Path,
    dest_zip: &Path,
    inner_member_name: &str,
) -> io::Result<()> {
    let reader = File::open(src).map(BufReader::new)?;
    let out = File::create(dest_zip)?;
    let mut zip = ZipWriter::new(out);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(inner_member_name, options)?;
    let mut reader = reader;
    io::copy(&mut reader, &mut zip)?;
    zip.finish()?;
    Ok(())
}

/// Zip `template` (must be closed by caller) into the UK-dated `.zip` for `archive_date`, then remove `template`.
fn rotate_file_to_archive(template: &Path, archive_date: NaiveDate) -> io::Result<()> {
    if !template.exists() {
        return Ok(());
    }
    let Some(archive_zip) = archived_zip_path(template, archive_date) else {
        return Ok(());
    };
    let Some(inner_name) = archived_member_name(template, archive_date) else {
        return Ok(());
    };
    relocate_existing_file(&archive_zip)?;
    zip_plain_file_to_archive(template, &archive_zip, &inner_name)?;
    fs::remove_file(template)?;
    Ok(())
}

fn template_stem_ext(template: &Path) -> Option<(&str, &str)> {
    Some((
        template.file_stem()?.to_str()?,
        template.extension()?.to_str()?,
    ))
}

/// Parse `stem_DD-MM-YYYY.ext` or `stem_DD-MM-YYYY_suffix.ext` (legacy collision) → date from the UK segment.
fn parse_dated_log_date(name: &str, stem: &str, ext: &str) -> Option<NaiveDate> {
    let prefix = format!("{stem}_");
    let suffix = format!(".{ext}");
    if !name.starts_with(&prefix) || !name.ends_with(&suffix) {
        return None;
    }
    let mid = &name[prefix.len()..name.len() - suffix.len()];
    let date_part = mid.split('_').next()?;
    NaiveDate::parse_from_str(date_part, "%d-%m-%Y").ok()
}

/// Zip legacy plain `stem_DD-MM-YYYY.ext` files when their calendar date is before today.
fn compress_legacy_dated_logs(template: &Path) -> io::Result<()> {
    let Some((stem, ext)) = template_stem_ext(template) else {
        return Ok(());
    };
    let Some(dir) = template.parent() else {
        return Ok(());
    };
    let today = today_local();
    let mut to_zip: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path == template {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(parsed) = parse_dated_log_date(name, stem, ext) else {
            continue;
        };
        if parsed < today {
            to_zip.push(path);
        }
    }
    for log_path in to_zip {
        let Some(name) = log_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(parsed) = parse_dated_log_date(name, stem, ext) else {
            continue;
        };
        let Some(zip_path) = archived_zip_path(template, parsed) else {
            continue;
        };
        let Some(inner) = archived_member_name(template, parsed) else {
            continue;
        };
        relocate_existing_file(&zip_path)?;
        zip_plain_file_to_archive(&log_path, &zip_path, &inner)?;
        let _ = fs::remove_file(&log_path);
    }
    Ok(())
}

fn parse_zip_archive_date(file_name: &str, stem: &str) -> Option<NaiveDate> {
    let prefix = format!("{stem}_");
    if !file_name.starts_with(&prefix) || !file_name.ends_with(".zip") {
        return None;
    }
    let mid = &file_name[prefix.len()..file_name.len() - 4];
    let date_part = mid.split('_').next()?;
    NaiveDate::parse_from_str(date_part, "%d-%m-%Y").ok()
}

/// Remove `stem_*.zip` archives whose UK date is strictly older than 45 days before today.
fn prune_old_zip_archives(template: &Path) -> io::Result<()> {
    let Some(stem) = template.file_stem().and_then(|s| s.to_str()) else {
        return Ok(());
    };
    let Some(dir) = template.parent() else {
        return Ok(());
    };
    let today = today_local();
    let Some(cutoff) = today.checked_sub_signed(Duration::days(RETENTION_DAYS)) else {
        return Ok(());
    };
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(archive_date) = parse_zip_archive_date(name, stem) else {
            continue;
        };
        if archive_date < cutoff {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

/// Compress legacy dated `.log` files and delete zip archives past retention. Best called after rotation and on startup.
pub fn run_log_maintenance(template: &Path) -> io::Result<()> {
    compress_legacy_dated_logs(template)?;
    prune_old_zip_archives(template)?;
    Ok(())
}

/// If `template` exists and is stale, zip it to `stem_DD-MM-YYYY.zip` using the file's
/// last-modified **local calendar date**. Stale: earlier local day than today, or age ≥ 24h.
pub fn rotate_flat_log_if_stale(template: &Path) -> io::Result<()> {
    if !template.exists() {
        return Ok(());
    }
    let meta = fs::metadata(template)?;
    let mtime = match meta.modified() {
        Ok(t) => t,
        Err(_) => return Ok(()),
    };
    let mtime_date = local_date_from_system_time(mtime).unwrap_or_else(today_local);
    let today = today_local();
    let age_secs = SystemTime::now()
        .duration_since(mtime)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stale_day = mtime_date < today;
    let stale_24h = age_secs >= SECS_PER_DAY;
    if !stale_day && !stale_24h {
        return Ok(());
    }
    rotate_file_to_archive(template, mtime_date)?;
    run_log_maintenance(template)?;
    Ok(())
}

/// Opens the live log (`template`) for append after rotating a stale file on startup.
pub fn open_today_log_append(template: &Path) -> io::Result<std::fs::File> {
    if let Some(parent) = template.parent() {
        fs::create_dir_all(parent)?;
    }
    rotate_flat_log_if_stale(template)?;
    OpenOptions::new().create(true).append(true).open(template)
}

// --- tracing MakeWriter: roll at local midnight (close file, zip, reopen template) ---

struct DailyFileState {
    /// Local calendar day this open `template` file is for.
    date: NaiveDate,
    file: Option<std::fs::File>,
}

/// Writes to `template` (e.g. `talos_worker.log`); rolls over at local midnight.
pub struct DailyFileMakeWriter {
    template: PathBuf,
    state: Arc<Mutex<DailyFileState>>,
}

impl DailyFileMakeWriter {
    pub fn try_new(template: PathBuf) -> io::Result<Self> {
        if let Some(parent) = template.parent() {
            fs::create_dir_all(parent)?;
        }
        rotate_flat_log_if_stale(&template)?;
        let today = today_local();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&template)?;
        Ok(Self {
            template: template.clone(),
            state: Self::spawn_maintenance_thread(
                template,
                Arc::new(Mutex::new(DailyFileState {
                    date: today,
                    file: Some(file),
                })),
            ),
        })
    }

    fn spawn_maintenance_thread(
        template: PathBuf,
        state: Arc<Mutex<DailyFileState>>,
    ) -> Arc<Mutex<DailyFileState>> {
        let maintenance_state = Arc::clone(&state);
        thread::spawn(move || loop {
            thread::sleep(StdDuration::from_secs(MAINTENANCE_POLL_SECS));
            let _ = Self::refresh_if_new_day(&maintenance_state, &template);
        });
        state
    }

    #[cfg(test)]
    fn try_new_without_background_thread(template: PathBuf) -> io::Result<Self> {
        if let Some(parent) = template.parent() {
            fs::create_dir_all(parent)?;
        }
        rotate_flat_log_if_stale(&template)?;
        let today = today_local();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&template)?;
        Ok(Self {
            template,
            state: Arc::new(Mutex::new(DailyFileState {
                date: today,
                file: Some(file),
            })),
        })
    }

    fn refresh_if_new_day(state: &Arc<Mutex<DailyFileState>>, template: &Path) -> io::Result<()> {
        let today = today_local();
        let mut guard = match state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.date == today {
            return Ok(());
        }
        let archive_date = guard.date;
        // Close handle so Windows allows reading/zipping the live log file.
        guard.file.take();
        if template.exists() {
            let _ = rotate_file_to_archive(template, archive_date);
        }
        let _ = run_log_maintenance(template);
        guard.date = today;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(template)?;
        guard.file = Some(file);
        Ok(())
    }
}

pub struct DailyFileWriterGuard<'a> {
    guard: MutexGuard<'a, DailyFileState>,
}

impl Write for DailyFileWriterGuard<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.guard.file.as_mut() {
            Some(f) => f.write(buf),
            None => Err(io::Error::other("log file handle missing after rotation")),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.guard.file.as_mut() {
            Some(f) => f.flush(),
            None => Err(io::Error::other("log file handle missing after rotation")),
        }
    }
}

impl<'a> MakeWriter<'a> for DailyFileMakeWriter {
    type Writer = DailyFileWriterGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        let _ = Self::refresh_if_new_day(&self.state, &self.template);
        let guard = match self.state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        DailyFileWriterGuard { guard }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::io::Read;
    use zip::ZipArchive;

    #[test]
    fn daily_writer_zips_previous_day_log_when_refreshed() {
        let root = unique_temp_dir();
        fs::create_dir_all(&root).unwrap();
        let template = root.join("talos_worker.log");

        {
            let writer =
                DailyFileMakeWriter::try_new_without_background_thread(template.clone()).unwrap();
            {
                let mut file = writer.state.lock().unwrap();
                file.file
                    .as_mut()
                    .unwrap()
                    .write_all(b"previous day\n")
                    .unwrap();
                file.file.as_mut().unwrap().flush().unwrap();
                file.date = today_local().checked_sub_signed(Duration::days(1)).unwrap();
            }

            let _guard = writer.make_writer();
        }

        let archive_date = today_local().checked_sub_signed(Duration::days(1)).unwrap();
        let archive = archived_zip_path(&template, archive_date).unwrap();
        assert!(archive.exists(), "expected {}", archive.display());
        assert!(template.exists(), "live log should be reopened");

        let mut zip = ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        let mut member = zip
            .by_name(&archived_member_name(&template, archive_date).unwrap())
            .unwrap();
        let mut contents = String::new();
        member.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "previous day\n");

        let _ = fs::remove_dir_all(root);
    }

    fn unique_temp_dir() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!(
            "talos_log_util_test_{}_{}",
            std::process::id(),
            suffix
        ))
    }
}
