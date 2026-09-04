use crate::collectors::Collector;
use crate::models::{AppCrashEntry, BsodEntry, EventCounts, EventEntry, EventsSummary};
use crate::windows_utils::event_log;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use tracing::debug;

pub struct EventsCollector;

#[async_trait]
impl Collector for EventsCollector {
    fn name(&self) -> &'static str {
        "Events"
    }

    fn data_type(&self) -> &'static str {
        "events"
    }

    fn estimated_duration_ms(&self) -> u64 {
        5000 // Event log queries can be slow
    }

    fn requires_admin(&self) -> bool {
        true // Some event logs require admin
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting Events collection");

        let mut events = EventsSummary::default();

        // Get summary counts
        events.summary = self.collect_event_counts().await?;

        // Get critical errors
        events.critical_errors = self.collect_critical_errors().await?;

        // Get warnings
        events.warnings = self.collect_warnings().await?;

        // Get BSOD history
        events.bsod_history = self.collect_bsod_history().await?;

        // Get application crashes
        events.application_crashes = self.collect_app_crashes().await?;

        // Get Windows Update errors
        events.windows_update_errors = self.collect_wu_errors().await?;

        debug!("Events collection completed");

        Ok(json!(events))
    }
}

impl EventsCollector {
    async fn collect_event_counts(&self) -> Result<EventCounts> {
        let mut counts = EventCounts::default();
        let now = Utc::now();
        let cutoff_24h = now - Duration::hours(24);
        let cutoff_7d = now - Duration::days(7);

        // Query with a 2-day window so we don't miss events at timezone boundaries, then filter by 24h
        let mut logs24h =
            event_log::query_events("System", event_log::now_minus_days(2), 2000, None)
                .await
                .unwrap_or_default();
        logs24h.extend(
            event_log::query_events("Application", event_log::now_minus_days(2), 2000, None)
                .await
                .unwrap_or_default(),
        );

        for e in logs24h.into_iter().filter(|e| e.time >= cutoff_24h) {
            let lvl = e.level.to_ascii_lowercase();
            if lvl.contains("critical") {
                counts.critical_last_24h += 1;
            } else if lvl.contains("error") {
                counts.error_last_24h += 1;
            } else if lvl.contains("warning") {
                counts.warning_last_24h += 1;
            } else {
                counts.information_last_24h += 1;
            }
        }

        // Query 8 days for 7d counts, then filter by 7d
        let logs7d = event_log::query_events("System", event_log::now_minus_days(8), 5000, None)
            .await
            .unwrap_or_default();
        let logs7d_app =
            event_log::query_events("Application", event_log::now_minus_days(8), 5000, None)
                .await
                .unwrap_or_default();

        for e in logs7d
            .into_iter()
            .chain(logs7d_app)
            .filter(|e| e.time >= cutoff_7d)
        {
            let lvl = e.level.to_ascii_lowercase();
            if lvl.contains("critical") {
                counts.critical_last_7d += 1;
            } else if lvl.contains("error") {
                counts.error_last_7d += 1;
            }
        }

        Ok(counts)
    }

    async fn collect_critical_errors(&self) -> Result<Vec<EventEntry>> {
        let mut errors = Vec::new();

        let mut candidates =
            event_log::query_events("System", event_log::now_minus_days(1), 100, None)
                .await
                .unwrap_or_default();
        candidates.extend(
            event_log::query_events("Application", event_log::now_minus_days(1), 100, None)
                .await
                .unwrap_or_default(),
        );
        candidates.sort_by(|a, b| b.time.cmp(&a.time));

        for err in candidates
            .into_iter()
            .filter(|e| {
                let lvl = e.level.to_ascii_lowercase();
                lvl.contains("critical") || lvl.contains("error")
            })
            .take(10)
        {
            errors.push(EventEntry {
                time: err.time,
                source: err.source,
                event_id: err.event_id,
                level: err.level,
                message: err.message,
                computer: err.computer,
                user: err.user,
                raw_data: None,
            });
        }

        Ok(errors)
    }

    async fn collect_warnings(&self) -> Result<Vec<EventEntry>> {
        let mut warnings = Vec::new();

        let mut candidates =
            event_log::query_events("System", event_log::now_minus_days(1), 100, None)
                .await
                .unwrap_or_default();
        candidates.extend(
            event_log::query_events("Application", event_log::now_minus_days(1), 100, None)
                .await
                .unwrap_or_default(),
        );
        candidates.sort_by(|a, b| b.time.cmp(&a.time));

        for ev in candidates
            .into_iter()
            .filter(|e| e.level.to_ascii_lowercase().contains("warning"))
            .take(10)
        {
            warnings.push(EventEntry {
                time: ev.time,
                source: ev.source,
                event_id: ev.event_id,
                level: ev.level,
                message: ev.message,
                computer: ev.computer,
                user: ev.user,
                raw_data: None,
            });
        }

        Ok(warnings)
    }

    async fn collect_bsod_history(&self) -> Result<Vec<BsodEntry>> {
        let mut bsods = Vec::new();

        let events = event_log::query_events(
            "System",
            event_log::now_minus_days(30),
            20,
            Some("EventCode=1001"),
        )
        .await
        .unwrap_or_default();

        for bsod in events.into_iter().take(5) {
            let message = bsod.message;
            let bugcheck_code = self.extract_from_message(&message, "Bugcheck code: ");
            let parameter1 = self.extract_from_message(&message, "Parameter 1: ");
            let caused_by = self.extract_from_message(&message, "Caused By Driver: ");

            bsods.push(BsodEntry {
                time: bsod.time,
                bugcheck_code,
                parameter1,
                parameter2: self.extract_from_message(&message, "Parameter 2: "),
                parameter3: self.extract_from_message(&message, "Parameter 3: "),
                parameter4: self.extract_from_message(&message, "Parameter 4: "),
                caused_by_driver: caused_by,
                crash_address: self.extract_from_message(&message, "Crash Address: "),
                dump_file: self.extract_from_message(&message, "Dump file: "),
            });
        }

        Ok(bsods)
    }

    async fn collect_app_crashes(&self) -> Result<Vec<AppCrashEntry>> {
        let mut crashes = Vec::new();

        let crash_events = event_log::query_events(
            "Application",
            event_log::now_minus_days(7),
            20,
            Some("EventCode=1000"),
        )
        .await
        .unwrap_or_default();

        for crash in crash_events.into_iter().take(10) {
            crashes.push(AppCrashEntry {
                time: crash.time,
                app_name: self
                    .extract_from_message(&crash.message, "Faulting application name:")
                    .if_empty("Unknown"),
                app_version: self
                    .extract_from_message(&crash.message, "Faulting application version:"),
                exception_code: self.extract_from_message(&crash.message, "Exception code:"),
                faulting_module: self.extract_from_message(&crash.message, "Faulting module name:"),
                fault_offset: self.extract_from_message(&crash.message, "Fault offset:"),
            });
        }

        Ok(crashes)
    }

    async fn collect_wu_errors(&self) -> Result<Vec<EventEntry>> {
        let mut errors = Vec::new();

        let where_clause =
            "SourceName LIKE '%WindowsUpdate%' OR SourceName LIKE '%WindowsUpdateClient%'";
        let records = event_log::query_events(
            "Setup",
            event_log::now_minus_days(7),
            50,
            Some(where_clause),
        )
        .await
        .unwrap_or_default();

        for err in records
            .into_iter()
            .filter(|e| {
                let lvl = e.level.to_ascii_lowercase();
                lvl.contains("error") || lvl.contains("warning")
            })
            .take(10)
        {
            errors.push(EventEntry {
                time: err.time,
                source: if err.source.is_empty() {
                    "Windows Update".to_string()
                } else {
                    err.source
                },
                event_id: err.event_id,
                level: err.level,
                message: err.message,
                computer: err.computer,
                user: err.user,
                raw_data: None,
            });
        }

        Ok(errors)
    }

    fn extract_from_message(&self, message: &str, prefix: &str) -> String {
        if let Some(start) = message.find(prefix) {
            let after_prefix = &message[start + prefix.len()..];
            let end = after_prefix
                .find(['\n', '\r', ','])
                .unwrap_or(after_prefix.len());
            after_prefix[..end].trim().to_string()
        } else {
            String::new()
        }
    }
}

trait IfEmptyExt {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmptyExt for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_events_collector_name() {
        let collector = EventsCollector;
        assert_eq!(collector.name(), "Events");
    }

    #[test]
    fn test_extract_from_message() {
        let collector = EventsCollector;
        let message = "Bugcheck code: 0x0000001E\nParameter 1: 0x00000000";
        assert_eq!(
            collector.extract_from_message(message, "Bugcheck code: "),
            "0x0000001E"
        );
        assert_eq!(
            collector.extract_from_message(message, "Parameter 1: "),
            "0x00000000"
        );
    }
}
