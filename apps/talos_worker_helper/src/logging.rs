#![cfg(target_os = "windows")]

use std::io::{self, Write};
use std::path::PathBuf;

use tracing_subscriber::fmt::MakeWriter;

/// Writes each formatted tracing line to the daily log file and to **stderr** (terminal when a
/// console is attached), using the same layout as `talos_worker` (`SystemTime`, no ANSI).
pub(crate) fn init_helper_tracing(template: PathBuf) {
    let filter = tracing_subscriber::EnvFilter::new(talos_protocol::rmm_tracing_filter_directive());
    let timer = tracing_subscriber::fmt::time::SystemTime;
    match talos_log_util::DailyFileMakeWriter::try_new(template.clone()) {
        Ok(file_writer) => {
            let tee = StderrTeeMakeWriter { inner: file_writer };
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_timer(timer)
                .with_writer(tee)
                .with_ansi(false)
                .try_init();
        }
        Err(err) => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_timer(timer)
                .with_writer(io::stderr)
                .with_ansi(false)
                .try_init();
            tracing::warn!(
                target: "talos_worker_helper",
                error = %err,
                path = %template.display(),
                "could not open helper log file; logging to stderr only"
            );
        }
    }
}

struct StderrTeeMakeWriter {
    inner: talos_log_util::DailyFileMakeWriter,
}

struct StderrTeeWriter<'a> {
    file: talos_log_util::DailyFileWriterGuard<'a>,
}

impl<'a> MakeWriter<'a> for StderrTeeMakeWriter {
    type Writer = StderrTeeWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        StderrTeeWriter {
            file: self.inner.make_writer(),
        }
    }
}

impl Write for StderrTeeWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.file.write(buf)?;
        io::stderr().write_all(&buf[..n])?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()?;
        io::stderr().flush()
    }
}
