use std::{
    fs,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use talos_protocol::{
    FileTransferConflictMode, FileTransferEntry, FileTransferRequest, FileTransferResponse,
    OperationErrorCode, FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_BYTES,
    FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_FILES, FILE_TRANSFER_STORE_ARCHIVE_THRESHOLD_BYTES,
    FILE_TRANSFER_STORE_ARCHIVE_THRESHOLD_FILES,
};
use walkdir::WalkDir;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

#[derive(Debug)]
pub enum TransferError {
    Message(String),
    InvalidPath(String),
    Io {
        kind: std::io::ErrorKind,
        message: String,
    },
    Conflict {
        path: String,
        message: String,
    },
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransferError::Message(message) => write!(f, "{message}"),
            TransferError::InvalidPath(message) => write!(f, "{message}"),
            TransferError::Io { message, .. } => write!(f, "{message}"),
            TransferError::Conflict { path, message } => write!(f, "conflict at {path}: {message}"),
        }
    }
}

impl std::error::Error for TransferError {}

impl From<std::io::Error> for TransferError {
    fn from(value: std::io::Error) -> Self {
        TransferError::Io {
            kind: value.kind(),
            message: value.to_string(),
        }
    }
}

impl From<walkdir::Error> for TransferError {
    fn from(value: walkdir::Error) -> Self {
        if let Some(io_error) = value.io_error() {
            return TransferError::Io {
                kind: io_error.kind(),
                message: value.to_string(),
            };
        }

        TransferError::Message(value.to_string())
    }
}

impl From<zip::result::ZipError> for TransferError {
    fn from(value: zip::result::ZipError) -> Self {
        TransferError::Message(value.to_string())
    }
}

impl TransferError {
    pub fn operation_error_code(&self) -> OperationErrorCode {
        match self {
            TransferError::Conflict { .. } => OperationErrorCode::Conflict,
            TransferError::InvalidPath(_) => OperationErrorCode::InvalidPath,
            TransferError::Io { kind, .. } => match kind {
                std::io::ErrorKind::PermissionDenied => OperationErrorCode::PermissionDenied,
                std::io::ErrorKind::NotFound => OperationErrorCode::NotFound,
                std::io::ErrorKind::AlreadyExists => OperationErrorCode::Conflict,
                std::io::ErrorKind::InvalidInput => OperationErrorCode::InvalidRequest,
                _ => OperationErrorCode::Internal,
            },
            TransferError::Message(_) => OperationErrorCode::Internal,
        }
    }
}

pub struct PreparedDownload {
    pub source_path: PathBuf,
    pub file_name: String,
    pub is_archive: bool,
    pub size_bytes: u64,
    pub cleanup_source: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ArchivePreparationProgress {
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

pub struct UploadContext {
    pub transfer_id: String,
    pub temp_input_path: PathBuf,
    pub destination_path: PathBuf,
    pub file_name: String,
    pub is_archive: bool,
    pub extract_archive: bool,
    pub conflict_mode: FileTransferConflictMode,
    pub bytes_received: u64,
}

pub fn list_dir(path: &str) -> Result<FileTransferResponse, TransferError> {
    let normalized = path.trim();
    if normalized.is_empty() {
        return Ok(FileTransferResponse::ListDirResult {
            path: "/".to_string(),
            entries: list_roots()?,
        });
    }
    #[cfg(windows)]
    if normalized == "/" {
        return Ok(FileTransferResponse::ListDirResult {
            path: "/".to_string(),
            entries: list_roots()?,
        });
    }
    #[cfg(target_os = "macos")]
    if normalized == "/" {
        return Ok(FileTransferResponse::ListDirResult {
            path: "/".to_string(),
            entries: list_roots()?,
        });
    }

    let dir = normalize_existing_path(normalized)?;
    if !dir.is_dir() {
        return Err(TransferError::InvalidPath(
            "path is not a directory".to_string(),
        ));
    }

    let mut entries = fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .map(entry_to_file_transfer_entry)
        .collect::<Vec<_>>();

    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            return b.is_dir.cmp(&a.is_dir);
        }
        a.name.to_lowercase().cmp(&b.name.to_lowercase())
    });

    Ok(FileTransferResponse::ListDirResult {
        path: dir.to_string_lossy().to_string(),
        entries,
    })
}

fn entry_to_file_transfer_entry(entry: fs::DirEntry) -> FileTransferEntry {
    let path = entry.path();
    let name = entry.file_name().to_string_lossy().to_string();
    match entry.metadata() {
        Ok(metadata) => FileTransferEntry {
            name,
            path: path.to_string_lossy().to_string(),
            is_dir: metadata.is_dir(),
            size_bytes: if metadata.is_file() {
                metadata.len()
            } else {
                0
            },
            modified_unix_ms: metadata.modified().ok().and_then(system_time_to_unix_ms),
        },
        Err(_) => {
            let is_dir = entry
                .file_type()
                .map(|file_type| file_type.is_dir())
                .unwrap_or(false);
            FileTransferEntry {
                name,
                path: path.to_string_lossy().to_string(),
                is_dir,
                size_bytes: 0,
                modified_unix_ms: None,
            }
        }
    }
}

pub fn rename_path(from_path: &str, to_path: &str) -> Result<FileTransferResponse, TransferError> {
    let from = normalize_existing_path(from_path.trim())?;
    let to_trimmed = to_path.trim();
    if to_trimmed.is_empty() {
        return Err(TransferError::InvalidPath(
            "destination path must not be empty".to_string(),
        ));
    }
    let to = PathBuf::from(to_trimmed);
    if !to.is_absolute() {
        return Err(TransferError::InvalidPath(
            "destination path must be absolute".to_string(),
        ));
    }
    if to.exists() {
        return Err(TransferError::InvalidPath(
            "destination already exists".to_string(),
        ));
    }
    let parent = to.parent().ok_or_else(|| {
        TransferError::InvalidPath("unable to resolve destination parent directory".to_string())
    })?;
    if !parent.exists() || !parent.is_dir() {
        return Err(TransferError::InvalidPath(
            "destination parent is not a directory".to_string(),
        ));
    }
    fs::rename(&from, &to)?;
    Ok(FileTransferResponse::Ok {})
}

pub fn delete_path(path: &str, recursive: bool) -> Result<FileTransferResponse, TransferError> {
    let target = normalize_existing_path(path.trim())?;
    if target.is_dir() {
        if recursive {
            fs::remove_dir_all(&target)?;
        } else {
            fs::remove_dir(&target)?;
        }
    } else {
        fs::remove_file(&target)?;
    }
    Ok(FileTransferResponse::Ok {})
}

pub fn begin_download(paths: &[String]) -> Result<PreparedDownload, TransferError> {
    begin_download_with_progress(paths, |_| {})
}

pub fn begin_download_with_progress<F>(
    paths: &[String],
    on_progress: F,
) -> Result<PreparedDownload, TransferError>
where
    F: FnMut(ArchivePreparationProgress),
{
    let never_cancel = AtomicBool::new(false);
    begin_download_with_progress_cancel(paths, &never_cancel, on_progress)
}

pub fn begin_download_with_progress_cancel<F>(
    paths: &[String],
    cancelled: &AtomicBool,
    mut on_progress: F,
) -> Result<PreparedDownload, TransferError>
where
    F: FnMut(ArchivePreparationProgress),
{
    if paths.is_empty() {
        return Err(TransferError::Message("no paths selected".to_string()));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(TransferError::Message("cancelled".to_string()));
    }

    let normalized_paths = paths
        .iter()
        .map(|path| normalize_existing_path(path))
        .collect::<Result<Vec<_>, _>>()?;

    if normalized_paths.len() == 1 && normalized_paths[0].is_file() {
        let source_path = normalized_paths[0].clone();
        let file_name = source_path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| "download.bin".to_string());
        let size_bytes = fs::metadata(&source_path)?.len();
        return Ok(PreparedDownload {
            source_path,
            file_name,
            is_archive: false,
            size_bytes,
            cleanup_source: false,
        });
    }

    let (file_count, total_bytes, contains_dir) = summarize_paths(&normalized_paths)?;
    let should_zip = contains_dir
        || normalized_paths.len() > 1
        || file_count > FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_FILES as usize
        || total_bytes > FILE_TRANSFER_DEFAULT_ZIP_THRESHOLD_BYTES;
    if !should_zip {
        return Err(TransferError::Message(
            "unable to prepare non-zip multi-path download".to_string(),
        ));
    }

    let archive_path = build_temp_path("rmm_download", "zip");
    // If we hit any error after creating the temp path (including cancellation during zipping),
    // make sure we don't leak the partial archive in the OS temp directory.
    struct CleanupOnDrop(Option<PathBuf>);
    impl Drop for CleanupOnDrop {
        fn drop(&mut self) {
            if let Some(path) = self.0.take() {
                let _ = fs::remove_file(path);
            }
        }
    }
    let mut archive_cleanup = CleanupOnDrop(Some(archive_path.clone()));
    on_progress(ArchivePreparationProgress {
        files_done: 0,
        files_total: file_count,
        bytes_done: 0,
        bytes_total: total_bytes,
    });
    match create_zip_archive(
        &archive_path,
        &normalized_paths,
        file_count,
        total_bytes,
        cancelled,
        &mut on_progress,
    ) {
        Ok(()) => {}
        Err(error) => return Err(error),
    }
    let size_bytes = fs::metadata(&archive_path)?.len();
    let file_name = archive_path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "download.zip".to_string());
    archive_cleanup.0 = None; // caller is responsible for cleanup via PreparedDownload.cleanup_source
    Ok(PreparedDownload {
        source_path: archive_path,
        file_name,
        is_archive: true,
        size_bytes,
        cleanup_source: true,
    })
}

pub fn begin_upload(request: &FileTransferRequest) -> Result<UploadContext, TransferError> {
    let FileTransferRequest::Upload {
        transfer_id,
        destination_path,
        file_name,
        is_archive,
        extract_archive,
        conflict_mode,
        ..
    } = request
    else {
        return Err(TransferError::Message(
            "invalid request for upload".to_string(),
        ));
    };

    let destination_path = normalize_destination_path(destination_path)?;
    if !destination_path.exists() {
        let created_dirs = missing_directory_paths(&destination_path);
        fs::create_dir_all(&destination_path)?;
        apply_uploaded_directory_ownership_paths(&created_dirs)?;
    }

    let temp_input_path = build_temp_path("rmm_upload", if *is_archive { "zip" } else { "bin" });

    Ok(UploadContext {
        transfer_id: transfer_id.clone(),
        temp_input_path,
        destination_path,
        file_name: file_name.clone(),
        is_archive: *is_archive,
        extract_archive: *extract_archive,
        conflict_mode: *conflict_mode,
        bytes_received: 0,
    })
}

pub fn append_upload_chunk(upload: &mut UploadContext, chunk: &[u8]) -> Result<(), TransferError> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&upload.temp_input_path)?;
    file.write_all(chunk)?;
    upload.bytes_received = upload.bytes_received.saturating_add(chunk.len() as u64);
    Ok(())
}

pub fn finalize_upload(upload: UploadContext) -> Result<FileTransferResponse, TransferError> {
    let extracted_entries = if upload.is_archive && upload.extract_archive {
        let extracted_entries = extract_archive(
            &upload.temp_input_path,
            &upload.destination_path,
            upload.conflict_mode,
        )?;
        let _ = fs::remove_file(&upload.temp_input_path);
        extracted_entries
    } else {
        let mut target_path = upload.destination_path.join(upload.file_name);
        if target_path.exists() {
            match upload.conflict_mode {
                FileTransferConflictMode::Prompt => {
                    return Err(TransferError::Conflict {
                        path: target_path.to_string_lossy().to_string(),
                        message: "destination already exists".to_string(),
                    });
                }
                FileTransferConflictMode::Skip => {
                    let _ = fs::remove_file(&upload.temp_input_path);
                    return Ok(FileTransferResponse::TransferComplete {
                        transfer_id: upload.transfer_id.clone(),
                        bytes_transferred: 0,
                        extracted_entries: 0,
                    });
                }
                FileTransferConflictMode::Overwrite => {
                    remove_path_if_exists(&target_path)?;
                }
                FileTransferConflictMode::Rename => {
                    target_path = next_available_path(&target_path);
                }
            }
        }
        fs::rename(&upload.temp_input_path, &target_path)?;
        apply_uploaded_path_ownership(&target_path)?;
        1
    };

    Ok(FileTransferResponse::TransferComplete {
        transfer_id: upload.transfer_id,
        bytes_transferred: upload.bytes_received,
        extracted_entries,
    })
}

fn summarize_paths(paths: &[PathBuf]) -> Result<(usize, u64, bool), TransferError> {
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;
    let mut contains_dir = false;

    for path in paths {
        if path.is_dir() {
            contains_dir = true;
            for entry in WalkDir::new(path) {
                let entry = entry?;
                if entry.file_type().is_file() {
                    file_count = file_count.saturating_add(1);
                    total_bytes = total_bytes.saturating_add(entry.metadata()?.len());
                }
            }
        } else if path.is_file() {
            file_count = file_count.saturating_add(1);
            total_bytes = total_bytes.saturating_add(fs::metadata(path)?.len());
        }
    }

    Ok((file_count, total_bytes, contains_dir))
}

fn create_zip_archive(
    archive_path: &Path,
    selected_paths: &[PathBuf],
    file_count: usize,
    total_bytes: u64,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(ArchivePreparationProgress),
) -> Result<(), TransferError> {
    let file = File::create(archive_path)?;
    let mut zip = ZipWriter::new(file);
    let compression = if should_use_store_mode(file_count, total_bytes) {
        CompressionMethod::Stored
    } else {
        CompressionMethod::Deflated
    };
    let file_options = SimpleFileOptions::default().compression_method(compression);
    let dir_options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let mut files_done = 0usize;
    let mut bytes_done = 0u64;
    let mut last_emit = SystemTime::now();

    for selected_path in selected_paths {
        if cancelled.load(Ordering::Relaxed) {
            return Err(TransferError::Message("cancelled".to_string()));
        }
        let name = selected_path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| "item".to_string());

        if selected_path.is_dir() {
            let folder_header = format!("{}/", to_zip_path(&name));
            zip.add_directory(
                folder_header,
                options_with_path_permissions(dir_options, selected_path, 0o755)?,
            )?;
            for entry in WalkDir::new(selected_path) {
                let entry = entry?;
                if entry.file_type().is_symlink() {
                    continue;
                }
                let entry_path = entry.path();
                let relative = entry_path
                    .strip_prefix(selected_path)
                    .map_err(|error| TransferError::Message(error.to_string()))?;
                if relative.as_os_str().is_empty() {
                    continue;
                }
                let mut archive_name = PathBuf::from(&name);
                archive_name.push(relative);
                let archive_name = to_zip_path(&archive_name.to_string_lossy());
                if entry.file_type().is_dir() {
                    zip.add_directory(
                        format!("{archive_name}/"),
                        options_with_path_permissions(dir_options, entry_path, 0o755)?,
                    )?;
                } else if entry.file_type().is_file() {
                    zip.start_file(
                        archive_name,
                        options_with_path_permissions(file_options, entry_path, 0o644)?,
                    )?;
                    let mut source = File::open(entry_path)?;
                    let mut buffer = [0u8; 262_144];
                    loop {
                        if cancelled.load(Ordering::Relaxed) {
                            return Err(TransferError::Message("cancelled".to_string()));
                        }
                        let read = source.read(&mut buffer)?;
                        if read == 0 {
                            break;
                        }
                        zip.write_all(&buffer[..read])?;
                        bytes_done = bytes_done.saturating_add(read as u64);
                        // Throttle progress updates to avoid excessive chatter.
                        if bytes_done % (2 * 1024 * 1024) < read as u64 {
                            on_progress(ArchivePreparationProgress {
                                files_done,
                                files_total: file_count,
                                bytes_done,
                                bytes_total: total_bytes,
                            });
                        }
                    }
                    files_done = files_done.saturating_add(1);
                    // Emit at least once per file for consistent UX.
                    if last_emit.elapsed().unwrap_or_default()
                        > std::time::Duration::from_millis(200)
                    {
                        last_emit = SystemTime::now();
                        on_progress(ArchivePreparationProgress {
                            files_done,
                            files_total: file_count,
                            bytes_done,
                            bytes_total: total_bytes,
                        });
                    }
                }
            }
        } else {
            zip.start_file(
                to_zip_path(&name),
                options_with_path_permissions(file_options, selected_path, 0o644)?,
            )?;
            let mut source = File::open(selected_path)?;
            let mut buffer = [0u8; 262_144];
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(TransferError::Message("cancelled".to_string()));
                }
                let read = source.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                zip.write_all(&buffer[..read])?;
                bytes_done = bytes_done.saturating_add(read as u64);
                if bytes_done % (2 * 1024 * 1024) < read as u64 {
                    on_progress(ArchivePreparationProgress {
                        files_done,
                        files_total: file_count,
                        bytes_done,
                        bytes_total: total_bytes,
                    });
                }
            }
            files_done = files_done.saturating_add(1);
            on_progress(ArchivePreparationProgress {
                files_done,
                files_total: file_count,
                bytes_done,
                bytes_total: total_bytes,
            });
        }
    }

    zip.finish()?;
    on_progress(ArchivePreparationProgress {
        files_done: file_count,
        files_total: file_count,
        bytes_done,
        bytes_total: total_bytes,
    });
    Ok(())
}

fn should_use_store_mode(file_count: usize, total_bytes: u64) -> bool {
    file_count >= FILE_TRANSFER_STORE_ARCHIVE_THRESHOLD_FILES as usize
        || total_bytes >= FILE_TRANSFER_STORE_ARCHIVE_THRESHOLD_BYTES
}

fn extract_archive(
    archive_path: &Path,
    destination: &Path,
    conflict_mode: FileTransferConflictMode,
) -> Result<u32, TransferError> {
    let file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut extracted_entries = 0u32;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(enclosed_name) = entry.enclosed_name().map(|value| value.to_owned()) else {
            continue;
        };
        if entry.is_symlink() {
            continue;
        }
        let mut target_path = destination.join(enclosed_name);

        if entry.is_dir() {
            fs::create_dir_all(&target_path)?;
            apply_entry_permissions(&target_path, entry.unix_mode())?;
            apply_uploaded_path_ownership(&target_path)?;
            continue;
        }

        if let Some(parent) = target_path.parent() {
            let created_dirs = missing_directory_paths(parent);
            fs::create_dir_all(parent)?;
            apply_uploaded_directory_ownership_paths(&created_dirs)?;
        }

        if target_path.exists() {
            match conflict_mode {
                FileTransferConflictMode::Prompt => {
                    return Err(TransferError::Conflict {
                        path: target_path.to_string_lossy().to_string(),
                        message: "destination already exists".to_string(),
                    });
                }
                FileTransferConflictMode::Skip => {
                    continue;
                }
                FileTransferConflictMode::Overwrite => {
                    remove_path_if_exists(&target_path)?;
                }
                FileTransferConflictMode::Rename => {
                    target_path = next_available_path(&target_path);
                }
            }
        }

        let mut target = File::create(&target_path)?;
        std::io::copy(&mut entry, &mut target)?;
        apply_entry_permissions(&target_path, entry.unix_mode())?;
        apply_uploaded_path_ownership(&target_path)?;
        extracted_entries = extracted_entries.saturating_add(1);
    }

    Ok(extracted_entries)
}

fn next_available_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }

    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();

    let mut index = 1usize;
    loop {
        let mut candidate_name = format!("{stem} ({index})");
        if !extension.is_empty() {
            candidate_name.push('.');
            candidate_name.push_str(&extension);
        }
        let candidate = parent.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
        index = index.saturating_add(1);
    }
}

fn remove_path_if_exists(path: &Path) -> Result<(), TransferError> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn list_roots() -> Result<Vec<FileTransferEntry>, TransferError> {
    #[cfg(windows)]
    {
        let mut roots = Vec::new();
        for drive in b'A'..=b'Z' {
            let path = format!("{}:\\", drive as char);
            let root_path = PathBuf::from(&path);
            if root_path.exists() {
                roots.push(FileTransferEntry {
                    name: path.clone(),
                    path,
                    is_dir: true,
                    size_bytes: 0,
                    modified_unix_ms: None,
                });
            }
        }
        Ok(roots)
    }

    #[cfg(target_os = "macos")]
    {
        let mut roots = Vec::new();
        let home = macos_file_transfer_home();
        let candidates = [
            ("/", "/".to_string()),
            ("Home", home.clone()),
            ("Users", "/Users".to_string()),
            ("Volumes", "/Volumes".to_string()),
            ("Applications", "/Applications".to_string()),
            ("Desktop", format!("{home}/Desktop")),
            ("Documents", format!("{home}/Documents")),
            ("Downloads", format!("{home}/Downloads")),
            ("tmp", "/tmp".to_string()),
        ];
        for (name, path) in candidates {
            if let Some(entry) = root_entry_if_exists(name, &path) {
                roots.push(entry);
            }
        }
        Ok(roots)
    }

    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        Ok(vec![FileTransferEntry {
            name: "/".to_string(),
            path: "/".to_string(),
            is_dir: true,
            size_bytes: 0,
            modified_unix_ms: None,
        }])
    }
}

#[cfg(target_os = "macos")]
fn macos_file_transfer_home() -> String {
    let env_home = std::env::var("HOME").ok();
    macos_effective_file_transfer_home(
        env_home.as_deref(),
        macos_console_user().as_ref().map(|user| user.home.as_str()),
    )
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug)]
struct MacosConsoleUser {
    uid: libc::uid_t,
    gid: libc::gid_t,
    home: String,
}

#[cfg(target_os = "macos")]
fn macos_effective_file_transfer_home(
    env_home: Option<&str>,
    console_home: Option<&str>,
) -> String {
    let env_home = env_home.map(str::trim).filter(|value| !value.is_empty());
    let console_home = console_home
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if env_home.is_none_or(|home| home == "/var/root") {
        if let Some(home) = console_home {
            return home.to_string();
        }
    }
    env_home.unwrap_or("/Users").to_string()
}

#[cfg(target_os = "macos")]
fn macos_console_user() -> Option<MacosConsoleUser> {
    use std::os::unix::fs::MetadataExt;

    let uid = fs::metadata("/dev/console").ok()?.uid();
    if uid == 0 {
        return None;
    }
    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0u8; 16 * 1024];
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            &mut passwd,
            buffer.as_mut_ptr() as *mut libc::c_char,
            buffer.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    let gid = passwd.pw_gid;
    let home = unsafe { std::ffi::CStr::from_ptr(passwd.pw_dir) }
        .to_string_lossy()
        .trim()
        .to_string();
    if home.is_empty() {
        None
    } else {
        Some(MacosConsoleUser { uid, gid, home })
    }
}

#[cfg(target_os = "macos")]
fn root_entry_if_exists(name: &str, path: &str) -> Option<FileTransferEntry> {
    let root_path = PathBuf::from(path);
    if !root_path.is_dir() {
        return None;
    }
    Some(FileTransferEntry {
        name: name.to_string(),
        path: path.to_string(),
        is_dir: true,
        size_bytes: 0,
        modified_unix_ms: fs::metadata(&root_path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(system_time_to_unix_ms),
    })
}

fn normalize_existing_path(path: &str) -> Result<PathBuf, TransferError> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(TransferError::InvalidPath(
            "path must be absolute".to_string(),
        ));
    }
    if let Err(error) = fs::metadata(&path) {
        return Err(TransferError::Io {
            kind: error.kind(),
            message: if error.kind() == std::io::ErrorKind::NotFound {
                "path does not exist".to_string()
            } else {
                error.to_string()
            },
        });
    }
    path.canonicalize().map_err(TransferError::from)
}

fn normalize_destination_path(path: &str) -> Result<PathBuf, TransferError> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(TransferError::InvalidPath(
            "destination path must be absolute".to_string(),
        ));
    }
    if path.exists() {
        return path.canonicalize().map_err(TransferError::from);
    }

    let mut ancestor = path.as_path();
    while !ancestor.exists() {
        let Some(parent) = ancestor.parent() else {
            return Err(TransferError::InvalidPath(
                "unable to resolve destination path".to_string(),
            ));
        };
        ancestor = parent;
    }
    if !ancestor.is_dir() {
        return Err(TransferError::InvalidPath(
            "destination ancestor is not a directory".to_string(),
        ));
    }

    let mut rebuilt = ancestor.canonicalize().map_err(TransferError::from)?;
    if let Ok(remainder) = path.strip_prefix(ancestor) {
        rebuilt.push(remainder);
    }
    Ok(rebuilt)
}

pub fn normalize_upload_destination_path(path: &str) -> Result<PathBuf, TransferError> {
    normalize_destination_path(path)
}

fn to_zip_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn options_with_path_permissions(
    options: SimpleFileOptions,
    path: &Path,
    default_mode: u32,
) -> Result<SimpleFileOptions, TransferError> {
    #[cfg(unix)]
    {
        let mode = fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o777)
            .unwrap_or(default_mode);
        Ok(options.unix_permissions(mode))
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        let _ = default_mode;
        Ok(options)
    }
}

fn apply_entry_permissions(path: &Path, unix_mode: Option<u32>) -> Result<(), TransferError> {
    #[cfg(unix)]
    {
        let mode = unix_mode.unwrap_or(0) & 0o777;
        if mode != 0 {
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        }
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        let _ = unix_mode;
    }

    Ok(())
}

fn apply_uploaded_path_ownership(path: &Path) -> Result<(), TransferError> {
    #[cfg(target_os = "macos")]
    {
        let Some(user) = macos_console_user() else {
            return Ok(());
        };
        if !macos_path_is_under_console_home(path, &user.home) {
            return Ok(());
        }
        apply_uploaded_path_ownership_for_user(path, &user)?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
    }

    Ok(())
}

fn apply_uploaded_directory_ownership_paths(paths: &[PathBuf]) -> Result<(), TransferError> {
    #[cfg(target_os = "macos")]
    {
        let Some(user) = macos_console_user() else {
            return Ok(());
        };
        for path in paths {
            if macos_path_is_under_console_home(path, &user.home) {
                apply_uploaded_path_ownership_for_user(path, &user)?;
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = paths;
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn apply_uploaded_path_ownership_for_user(
    path: &Path,
    user: &MacosConsoleUser,
) -> Result<(), TransferError> {
    let path_cstring =
        std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|error| {
            TransferError::InvalidPath(format!("path contains invalid nul byte: {error}"))
        })?;
    let rc = unsafe { libc::chown(path_cstring.as_ptr(), user.uid, user.gid) };
    if rc != 0 {
        return Err(TransferError::Io {
            kind: std::io::Error::last_os_error().kind(),
            message: std::io::Error::last_os_error().to_string(),
        });
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_path_is_under_console_home(path: &Path, console_home: &str) -> bool {
    let home = Path::new(console_home);
    !console_home.trim().is_empty() && path.starts_with(home)
}

fn missing_directory_paths(path: &Path) -> Vec<PathBuf> {
    let mut missing = Vec::new();
    let mut current = Some(path);
    while let Some(candidate) = current {
        if candidate.exists() {
            break;
        }
        missing.push(candidate.to_path_buf());
        current = candidate.parent();
    }
    missing.reverse();
    missing
}

fn build_temp_path(prefix: &str, extension: &str) -> PathBuf {
    static TEMP_PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let sequence = TEMP_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "{prefix}_{millis}_{}_{}.{extension}",
        std::process::id(),
        sequence
    ));
    path
}

fn system_time_to_unix_ms(value: SystemTime) -> Option<u64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::AtomicBool;

    fn unique_temp_dir(label: &str) -> PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "talos_file_transfer_{label}_{millis}_{}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[test]
    fn unix_root_lists_root_entries() {
        let response = list_dir("/").expect("list unix root");
        let FileTransferResponse::ListDirResult { path, entries } = response else {
            panic!("expected list_dir_result");
        };

        assert_eq!(path, "/");
        assert!(
            entries.iter().any(|entry| entry.path != "/"),
            "unix root listing should include navigable child entries"
        );
    }

    #[test]
    fn path_errors_map_to_specific_operation_codes() {
        let missing = unique_temp_dir("missing_parent").join("missing");
        let missing_error = normalize_existing_path(&missing.to_string_lossy())
            .expect_err("missing path should fail");
        assert_eq!(
            missing_error.operation_error_code(),
            OperationErrorCode::NotFound
        );

        let invalid_error =
            normalize_existing_path("relative/path").expect_err("relative path should fail");
        assert_eq!(
            invalid_error.operation_error_code(),
            OperationErrorCode::InvalidPath
        );

        let permission_error = TransferError::Io {
            kind: std::io::ErrorKind::PermissionDenied,
            message: "permission denied".to_string(),
        };
        assert_eq!(
            permission_error.operation_error_code(),
            OperationErrorCode::PermissionDenied
        );
        let _ = fs::remove_dir_all(missing.parent().expect("missing parent"));
    }

    #[test]
    fn walkdir_permission_errors_map_to_permission_denied() {
        let base = unique_temp_dir("walkdir_permission");
        let protected = base.join("protected");
        fs::create_dir_all(&protected).expect("create protected dir");
        fs::set_permissions(&protected, fs::Permissions::from_mode(0o000))
            .expect("make protected dir unreadable");

        let walk_error = WalkDir::new(&base)
            .into_iter()
            .find_map(Result::err)
            .expect("walkdir should report unreadable child");
        let transfer_error = TransferError::from(walk_error);

        assert_eq!(
            transfer_error.operation_error_code(),
            OperationErrorCode::PermissionDenied
        );

        let _ = fs::set_permissions(&protected, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn list_dir_preserves_entries_when_metadata_is_unavailable() {
        let base = unique_temp_dir("metadata_unavailable");
        let broken = base.join("missing-target-link");
        symlink(base.join("missing-target"), &broken).expect("create broken symlink");

        let response = list_dir(&base.to_string_lossy()).expect("list dir");
        let FileTransferResponse::ListDirResult { entries, .. } = response else {
            panic!("expected list_dir_result");
        };

        let entry = entries
            .iter()
            .find(|entry| entry.name == "missing-target-link")
            .expect("metadata-unavailable entry should remain visible");
        assert!(
            entry.path.ends_with("/missing-target-link"),
            "unexpected entry path: {}",
            entry.path
        );
        assert!(!entry.is_dir);
        assert_eq!(entry.size_bytes, 0);

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn temp_paths_are_unique_within_process() {
        let first = build_temp_path("rmm_upload", "bin");
        let second = build_temp_path("rmm_upload", "bin");

        assert_ne!(first, second);
        assert_eq!(
            first.extension().and_then(|value| value.to_str()),
            Some("bin")
        );
        assert_eq!(
            second.extension().and_then(|value| value.to_str()),
            Some("bin")
        );
    }

    #[test]
    fn missing_directory_paths_include_only_new_chain() {
        let base = unique_temp_dir("missing_dirs");
        let nested = base.join("one").join("two").join("three");

        assert_eq!(
            missing_directory_paths(&nested),
            vec![
                base.join("one"),
                base.join("one").join("two"),
                nested.clone()
            ]
        );

        fs::create_dir_all(base.join("one")).expect("create first nested dir");
        assert_eq!(
            missing_directory_paths(&nested),
            vec![base.join("one").join("two"), nested]
        );

        let _ = fs::remove_dir_all(base);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_empty_path_lists_useful_roots() {
        let response = list_dir("").expect("list macos roots");
        let FileTransferResponse::ListDirResult { path, entries } = response else {
            panic!("expected list_dir_result");
        };

        assert_eq!(path, "/");
        assert!(entries.iter().any(|entry| entry.path == "/"));
        assert!(entries.iter().any(|entry| entry.path == "/Users"));
        assert!(entries.iter().any(|entry| entry.path == "/tmp"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_file_transfer_home_prefers_console_user_when_service_home_is_root() {
        assert_eq!(
            macos_effective_file_transfer_home(Some("/var/root"), Some("/Users/sebastian")),
            "/Users/sebastian"
        );
        assert_eq!(
            macos_effective_file_transfer_home(Some("/Users/custom"), Some("/Users/sebastian")),
            "/Users/custom"
        );
        assert_eq!(
            macos_effective_file_transfer_home(Some("/var/root"), None),
            "/var/root"
        );
        assert_eq!(
            macos_effective_file_transfer_home(None, Some("/Users/sebastian")),
            "/Users/sebastian"
        );
        assert_eq!(macos_effective_file_transfer_home(None, None), "/Users");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_upload_ownership_only_applies_inside_console_home() {
        assert!(macos_path_is_under_console_home(
            Path::new("/Users/sebastian/Desktop/file.txt"),
            "/Users/sebastian"
        ));
        assert!(macos_path_is_under_console_home(
            Path::new("/Users/sebastian"),
            "/Users/sebastian"
        ));
        assert!(!macos_path_is_under_console_home(
            Path::new("/Users/other/Desktop/file.txt"),
            "/Users/sebastian"
        ));
        assert!(!macos_path_is_under_console_home(
            Path::new("/tmp/file.txt"),
            "/Users/sebastian"
        ));
        assert!(!macos_path_is_under_console_home(
            Path::new("/Users/sebastian/Desktop/file.txt"),
            ""
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_root_entry_includes_unreadable_directories() {
        let base = unique_temp_dir("macos_unreadable_root");
        let protected = base.join("protected");
        fs::create_dir_all(&protected).expect("create protected dir");
        fs::set_permissions(&protected, fs::Permissions::from_mode(0o000))
            .expect("make protected dir unreadable");

        let entry = root_entry_if_exists("Protected", &protected.to_string_lossy())
            .expect("protected root should still be visible");

        assert_eq!(entry.name, "Protected");
        assert_eq!(entry.path, protected.to_string_lossy());
        assert!(entry.is_dir);

        let _ = fs::set_permissions(&protected, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn zip_roundtrip_preserves_unix_executable_bits() {
        let base = unique_temp_dir("mode");
        let source = base.join("tool.sh");
        fs::write(&source, "#!/bin/sh\nexit 0\n").expect("write source");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755))
            .expect("set source permissions");
        let archive = base.join("archive.zip");
        let cancelled = AtomicBool::new(false);
        let mut progress = |_: ArchivePreparationProgress| {};

        create_zip_archive(
            &archive,
            std::slice::from_ref(&source),
            1,
            fs::metadata(&source).expect("source metadata").len(),
            &cancelled,
            &mut progress,
        )
        .expect("create archive");

        let destination = base.join("dest");
        fs::create_dir_all(&destination).expect("create dest");
        extract_archive(&archive, &destination, FileTransferConflictMode::Overwrite)
            .expect("extract archive");

        let extracted_mode = fs::metadata(destination.join("tool.sh"))
            .expect("extracted metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(extracted_mode, 0o755);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn folder_archive_skips_symlink_entries() {
        let base = unique_temp_dir("symlink");
        let source_dir = base.join("src");
        fs::create_dir_all(&source_dir).expect("create source dir");
        let real_file = source_dir.join("real.txt");
        let link_file = source_dir.join("link.txt");
        fs::write(&real_file, "real").expect("write real file");
        std::os::unix::fs::symlink(&real_file, &link_file).expect("create symlink");
        let archive = base.join("archive.zip");
        let cancelled = AtomicBool::new(false);
        let mut progress = |_: ArchivePreparationProgress| {};

        let (file_count, total_bytes, _) =
            summarize_paths(std::slice::from_ref(&source_dir)).expect("summarize source");
        create_zip_archive(
            &archive,
            std::slice::from_ref(&source_dir),
            file_count,
            total_bytes,
            &cancelled,
            &mut progress,
        )
        .expect("create archive");

        let file = File::open(&archive).expect("open archive");
        let mut archive_reader = ZipArchive::new(file).expect("read archive");
        let mut names = Vec::new();
        for index in 0..archive_reader.len() {
            names.push(
                archive_reader
                    .by_index(index)
                    .expect("archive entry")
                    .name()
                    .to_string(),
            );
        }

        assert!(names.iter().any(|name| name == "src/real.txt"));
        assert!(!names.iter().any(|name| name == "src/link.txt"));
        let _ = fs::remove_dir_all(base);
    }
}
