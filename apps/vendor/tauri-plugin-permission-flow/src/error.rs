use std::ffi::NulError;

use serde::{Serialize, ser::Serializer};

/// Result type used by the Tauri plugin surface.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("permission-flow is only available on macOS")]
    UnsupportedPlatform,
    #[error("appPath contains an embedded NUL byte")]
    InvalidAppPath(#[from] NulError),
    #[error("failed to receive a result from the main thread")]
    MainThreadChannelClosed,
    #[error("permission flow handle has already been closed")]
    DriverClosed,
    #[error(transparent)]
    Tauri(#[from] tauri::Error),
    #[error(transparent)]
    NewController(#[from] permission_flow::NewControllerError),
    #[error(transparent)]
    StartFlow(#[from] permission_flow::StartPermissionFlowError),
    #[error(transparent)]
    StopFlow(#[from] permission_flow::StopPermissionFlowError),
    #[error(transparent)]
    PermissionStatus(#[from] permission_flow::PermissionStatusError),
    #[cfg(mobile)]
    #[error(transparent)]
    PluginInvoke(#[from] tauri::plugin::mobile::PluginInvokeError),
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.to_string().as_ref())
    }
}
