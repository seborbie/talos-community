pub mod file_transfer;
#[cfg(target_family = "unix")]
pub mod linux_account;
#[cfg(target_os = "macos")]
pub mod macos_update_account;

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub mod capture;
#[cfg(target_os = "windows")]
pub mod control;
#[cfg(target_os = "windows")]
pub mod display;
#[cfg(target_os = "windows")]
pub(crate) mod display_processing;
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub mod encode;
#[cfg(target_os = "windows")]
mod monitor;
#[cfg(target_os = "windows")]
pub mod registry;
#[cfg(any(target_os = "windows", target_family = "unix"))]
pub mod shell;

#[cfg(target_os = "windows")]
pub(crate) use monitor::get_primary_monitor_hz;
