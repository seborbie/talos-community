use crate::event_stream::{
    monitors::{spawn_monitors, MonitorConfig},
    writer::{EventWriter, EventWriterConfig},
};
use anyhow::Result;
use std::path::PathBuf;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct EventStreamConfig {
    pub writer: EventWriterConfig,
    pub monitors: MonitorConfig,
    pub channel_capacity: usize,
    pub boot_session_id: String,
}

impl Default for EventStreamConfig {
    fn default() -> Self {
        Self {
            writer: EventWriterConfig::default(),
            monitors: MonitorConfig::default(),
            channel_capacity: 2048,
            boot_session_id: "unknown".to_string(),
        }
    }
}

impl EventStreamConfig {
    pub fn with_boot_session_id(mut self, boot_session_id: impl Into<String>) -> Self {
        self.boot_session_id = boot_session_id.into();
        self
    }

    #[allow(dead_code)]
    pub fn with_output_dir(mut self, output_dir: PathBuf) -> Self {
        self.writer.output_dir = output_dir;
        self
    }
}

pub async fn run_event_stream(config: EventStreamConfig) -> Result<()> {
    let (tx, rx) = mpsc::channel(config.channel_capacity);
    let writer = EventWriter::new(config.writer, rx).await?;

    spawn_monitors(tx, config.monitors, config.boot_session_id);
    writer.run().await
}
