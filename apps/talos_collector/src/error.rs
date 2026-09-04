use serde::{Deserialize, Serialize};
use std::fmt;

/// Errors that can occur during data collection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollectorError {
    /// WMI query failed
    WmiError { collector: String, message: String },
    /// Registry access failed
    RegistryError {
        collector: String,
        key: String,
        message: String,
    },
    /// PowerShell execution failed
    PowerShellError {
        collector: String,
        command: String,
        message: String,
    },
    /// Collection timed out
    Timeout { collector: String, duration_ms: u64 },
    /// Permission denied (needs admin)
    PermissionDenied { collector: String, message: String },
    /// Generic IO error
    IoError { collector: String, message: String },
    /// Serialization error
    SerializationError { collector: String, message: String },
    /// Collection not supported on this platform
    UnsupportedPlatform { collector: String },
    /// Unknown/Other error
    Other { collector: String, message: String },
}

impl CollectorError {
    /// Get the name of the collector that failed
    pub fn collector_name(&self) -> &str {
        match self {
            CollectorError::WmiError { collector, .. } => collector,
            CollectorError::RegistryError { collector, .. } => collector,
            CollectorError::PowerShellError { collector, .. } => collector,
            CollectorError::Timeout { collector, .. } => collector,
            CollectorError::PermissionDenied { collector, .. } => collector,
            CollectorError::IoError { collector, .. } => collector,
            CollectorError::SerializationError { collector, .. } => collector,
            CollectorError::UnsupportedPlatform { collector } => collector,
            CollectorError::Other { collector, .. } => collector,
        }
    }

    /// Get a human-readable error message
    pub fn message(&self) -> String {
        match self {
            CollectorError::WmiError { message, .. } => format!("WMI error: {}", message),
            CollectorError::RegistryError { key, message, .. } => {
                format!("Registry error accessing '{}': {}", key, message)
            }
            CollectorError::PowerShellError {
                command, message, ..
            } => {
                format!("PowerShell error running '{}': {}", command, message)
            }
            CollectorError::Timeout { duration_ms, .. } => {
                format!("Collection timed out after {}ms", duration_ms)
            }
            CollectorError::PermissionDenied { message, .. } => {
                format!("Permission denied: {}", message)
            }
            CollectorError::IoError { message, .. } => format!("IO error: {}", message),
            CollectorError::SerializationError { message, .. } => {
                format!("Serialization error: {}", message)
            }
            CollectorError::UnsupportedPlatform { .. } => "Platform not supported".to_string(),
            CollectorError::Other { message, .. } => message.clone(),
        }
    }

    /// Check if this error is due to missing admin privileges
    pub fn is_permission_error(&self) -> bool {
        matches!(self, CollectorError::PermissionDenied { .. })
    }

    /// Create a WMI error for a specific collector
    pub fn wmi<C: AsRef<str>, M: AsRef<str>>(collector: C, message: M) -> Self {
        CollectorError::WmiError {
            collector: collector.as_ref().to_string(),
            message: message.as_ref().to_string(),
        }
    }

    /// Create a registry error for a specific collector
    pub fn registry<C: AsRef<str>, K: AsRef<str>, M: AsRef<str>>(
        collector: C,
        key: K,
        message: M,
    ) -> Self {
        CollectorError::RegistryError {
            collector: collector.as_ref().to_string(),
            key: key.as_ref().to_string(),
            message: message.as_ref().to_string(),
        }
    }

    /// Create a PowerShell error for a specific collector
    pub fn powershell<C: AsRef<str>, CMD: AsRef<str>, M: AsRef<str>>(
        collector: C,
        command: CMD,
        message: M,
    ) -> Self {
        CollectorError::PowerShellError {
            collector: collector.as_ref().to_string(),
            command: command.as_ref().to_string(),
            message: message.as_ref().to_string(),
        }
    }

    /// Create a timeout error for a specific collector
    pub fn timeout<C: AsRef<str>>(collector: C, duration_ms: u64) -> Self {
        CollectorError::Timeout {
            collector: collector.as_ref().to_string(),
            duration_ms,
        }
    }

    /// Create a permission denied error for a specific collector
    pub fn permission_denied<C: AsRef<str>, M: AsRef<str>>(collector: C, message: M) -> Self {
        CollectorError::PermissionDenied {
            collector: collector.as_ref().to_string(),
            message: message.as_ref().to_string(),
        }
    }

    /// Create an IO error for a specific collector
    pub fn io<C: AsRef<str>, M: AsRef<str>>(collector: C, message: M) -> Self {
        CollectorError::IoError {
            collector: collector.as_ref().to_string(),
            message: message.as_ref().to_string(),
        }
    }

    /// Create a serialization error for a specific collector
    pub fn serialization<C: AsRef<str>, M: AsRef<str>>(collector: C, message: M) -> Self {
        CollectorError::SerializationError {
            collector: collector.as_ref().to_string(),
            message: message.as_ref().to_string(),
        }
    }

    /// Create an unsupported platform error for a specific collector
    pub fn unsupported_platform<C: AsRef<str>>(collector: C) -> Self {
        CollectorError::UnsupportedPlatform {
            collector: collector.as_ref().to_string(),
        }
    }

    /// Create a generic error for a specific collector
    pub fn other<C: AsRef<str>, M: AsRef<str>>(collector: C, message: M) -> Self {
        CollectorError::Other {
            collector: collector.as_ref().to_string(),
            message: message.as_ref().to_string(),
        }
    }
}

impl fmt::Display for CollectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.collector_name(), self.message())
    }
}

impl std::error::Error for CollectorError {}

// Convert from anyhow::Error
impl From<anyhow::Error> for CollectorError {
    fn from(err: anyhow::Error) -> Self {
        CollectorError::Other {
            collector: "unknown".to_string(),
            message: err.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_messages() {
        let err = CollectorError::wmi("TestCollector", "Query failed");
        assert_eq!(err.collector_name(), "TestCollector");
        assert!(err.message().contains("WMI error"));

        let err = CollectorError::registry("TestCollector", "HKLM\\Software", "Key not found");
        assert!(err.message().contains("Registry error"));
        assert!(err.message().contains("HKLM\\Software"));

        let err = CollectorError::timeout("TestCollector", 5000);
        assert!(err.message().contains("timed out"));
        assert!(err.message().contains("5000ms"));
    }

    #[test]
    fn test_is_permission_error() {
        let err = CollectorError::permission_denied("Test", "Need admin");
        assert!(err.is_permission_error());

        let err = CollectorError::wmi("Test", "Query failed");
        assert!(!err.is_permission_error());
    }
}
