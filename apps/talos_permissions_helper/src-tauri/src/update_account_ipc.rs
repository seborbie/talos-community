#[cfg(unix)]
use anyhow::Context;
use anyhow::{anyhow, Result};
#[cfg(unix)]
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
};
use std::{thread, time::Duration};
use talos_protocol::{
    MacosUpdateAccountIpcRequest, MacosUpdateAccountIpcResponse, MACOS_UPDATE_ACCOUNT_SOCKET_PATH,
};
use tracing::debug;

pub(super) fn macos_update_account_ipc_with_retry(
    request: &MacosUpdateAccountIpcRequest,
    attempts: usize,
    delay: Duration,
) -> Result<MacosUpdateAccountIpcResponse> {
    let attempts = attempts.max(1);
    let mut last_error = None;
    for attempt in 1..=attempts {
        match macos_update_account_ipc_once(request) {
            Ok(response) => return Ok(response),
            Err(err) if is_transient_macos_update_account_error(&err) && attempt < attempts => {
                debug!(
                    attempt,
                    attempts,
                    error = %err,
                    "macOS update account IPC unavailable; retrying"
                );
                last_error = Some(err);
                thread::sleep(delay);
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("macOS update account IPC retry exhausted")))
}

#[cfg(unix)]
fn macos_update_account_ipc_once(
    request: &MacosUpdateAccountIpcRequest,
) -> Result<MacosUpdateAccountIpcResponse> {
    let mut stream = UnixStream::connect(MACOS_UPDATE_ACCOUNT_SOCKET_PATH)
        .with_context(|| format!("connect {MACOS_UPDATE_ACCOUNT_SOCKET_PATH}"))?;
    let mut request_bytes =
        serde_json::to_vec(request).context("serialize macOS update account IPC request")?;
    request_bytes.push(b'\n');
    stream
        .write_all(&request_bytes)
        .context("write macOS update account IPC request")?;
    stream
        .flush()
        .context("flush macOS update account IPC request")?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("read macOS update account IPC response")?;
    if line.trim().is_empty() {
        return Err(anyhow!("empty macOS update account IPC response"));
    }
    serde_json::from_str(line.trim()).context("parse macOS update account IPC response")
}

#[cfg(not(unix))]
fn macos_update_account_ipc_once(
    _request: &MacosUpdateAccountIpcRequest,
) -> Result<MacosUpdateAccountIpcResponse> {
    Err(anyhow!(
        "macOS update account IPC is unsupported on this platform"
    ))
}

fn is_transient_macos_update_account_error(err: &anyhow::Error) -> bool {
    let chain = err
        .chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    chain.contains(&format!("connect {MACOS_UPDATE_ACCOUNT_SOCKET_PATH}"))
        || chain.contains("Connection refused")
        || chain.contains("empty macOS update account IPC response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_platform_errors_are_not_retried() {
        assert!(!is_transient_macos_update_account_error(&anyhow!(
            "macOS update account IPC is unsupported on this platform"
        )));
        assert!(is_transient_macos_update_account_error(&anyhow!(
            "connect {MACOS_UPDATE_ACCOUNT_SOCKET_PATH}"
        )));
    }

    #[cfg(not(unix))]
    #[test]
    fn unsupported_platform_rejects_account_ipc() {
        let result = macos_update_account_ipc_once(&MacosUpdateAccountIpcRequest::GetStatus);
        assert!(result.unwrap_err().to_string().contains("unsupported"));
    }
}
