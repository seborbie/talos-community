use crate::event_stream::schema::{compute_data_hash, EventEnvelope, EventInput};
use anyhow::{Context, Result};
use chrono::{Datelike, Local, Timelike};
use std::path::{Path, PathBuf};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::Receiver;

#[derive(Debug, Clone)]
pub struct EventWriterConfig {
    pub output_dir: PathBuf,
    pub active_file_name: String,
    pub max_file_size_bytes: u64,
    pub retention_days: i64,
}

impl Default for EventWriterConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from(r"C:\temp"),
            active_file_name: "rmm_events.jsonl".to_string(),
            max_file_size_bytes: 25 * 1024 * 1024,
            retention_days: 7,
        }
    }
}

pub struct EventWriter {
    config: EventWriterConfig,
    receiver: Receiver<EventInput>,
    file: File,
    seq: u64,
    bytes_written: u64,
    current_day: (i32, u32, u32),
}

impl EventWriter {
    pub async fn new(config: EventWriterConfig, receiver: Receiver<EventInput>) -> Result<Self> {
        fs::create_dir_all(&config.output_dir)
            .await
            .with_context(|| format!("failed to create {}", config.output_dir.display()))?;

        let active_path = config.output_dir.join(&config.active_file_name);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active_path)
            .await
            .with_context(|| format!("failed to open {}", active_path.display()))?;

        let size = file.metadata().await.map(|m| m.len()).unwrap_or(0);
        let now = Local::now();
        Ok(Self {
            config,
            receiver,
            file,
            seq: 0,
            bytes_written: size,
            current_day: (now.year(), now.month(), now.day()),
        })
    }

    pub async fn run(mut self) -> Result<()> {
        while let Some(input) = self.receiver.recv().await {
            self.rotate_if_needed().await?;

            self.seq += 1;
            let event = EventEnvelope {
                seq: self.seq,
                ts: chrono::Utc::now(),
                hash: compute_data_hash(&input.data),
                event_type: input.event_type,
                event_kind: input.event_kind,
                scope_key: input.scope_key,
                data: input.data,
            };
            let mut line = serde_json::to_vec(&event).context("serialize event")?;
            line.push(b'\n');

            // TODO(gzip): Optionally compress buffer before append for production.
            self.file
                .write_all(&line)
                .await
                .context("write event line")?;
            self.file.flush().await.context("flush event file")?;
            self.bytes_written += line.len() as u64;
        }

        Ok(())
    }

    async fn rotate_if_needed(&mut self) -> Result<()> {
        let now = Local::now();
        let today = (now.year(), now.month(), now.day());
        let by_day = today != self.current_day;
        let by_size = self.bytes_written >= self.config.max_file_size_bytes;
        if !by_day && !by_size {
            return Ok(());
        }

        self.file.flush().await.context("flush before rotate")?;
        drop(std::mem::replace(
            &mut self.file,
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.config.output_dir.join("rotation.tmp"))
                .await
                .context("open temporary rotation file")?,
        ));

        let active = self.config.output_dir.join(&self.config.active_file_name);
        let rotated = self.config.output_dir.join(format!(
            "rmm_events_{:04}-{:02}-{:02}_{:02}{:02}{:02}.jsonl",
            now.year(),
            now.month(),
            now.day(),
            now.hour(),
            now.minute(),
            now.second(),
        ));

        if active.exists() {
            fs::rename(&active, &rotated)
                .await
                .with_context(|| format!("failed to rotate {}", active.display()))?;
        }

        // TODO(gzip): In production, gzip rotated file to .jsonl.gz and remove .jsonl.
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active)
            .await
            .with_context(|| format!("failed to open {}", active.display()))?;

        self.current_day = today;
        self.bytes_written = self.file.metadata().await.map(|m| m.len()).unwrap_or(0);

        let tmp_path = self.config.output_dir.join("rotation.tmp");
        let _ = fs::remove_file(&tmp_path).await;

        self.enforce_retention().await?;
        Ok(())
    }

    async fn enforce_retention(&self) -> Result<()> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(self.config.retention_days);
        let mut entries = fs::read_dir(&self.config.output_dir)
            .await
            .with_context(|| format!("failed to read {}", self.config.output_dir.display()))?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with("rmm_events_") || !name.ends_with(".jsonl") {
                continue;
            }

            let Some(stamp) = extract_date_prefix(name) else {
                continue;
            };
            if stamp < cutoff.date_naive() {
                let _ = fs::remove_file(&path).await;
            }
        }

        Ok(())
    }
}

fn extract_date_prefix(name: &str) -> Option<chrono::NaiveDate> {
    // rmm_events_YYYY-MM-DD_HHMMSS.jsonl
    let date_part = name.strip_prefix("rmm_events_")?;
    let date_text = date_part.get(..10)?;
    chrono::NaiveDate::parse_from_str(date_text, "%Y-%m-%d").ok()
}

#[allow(dead_code)]
fn _is_path_in_dir(path: &Path, parent: &Path) -> bool {
    path.starts_with(parent)
}
