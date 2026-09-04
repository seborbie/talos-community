use std::fs::File;
use std::os::fd::{FromRawFd, RawFd};

use anyhow::{Context, Result};

use super::{DEFAULT_COLS, DEFAULT_ROWS};

pub(super) fn open_unix_pty() -> Result<(File, File)> {
    let mut master_fd: libc::c_int = -1;
    let mut slave_fd: libc::c_int = -1;
    let mut winsize = libc::winsize {
        ws_row: DEFAULT_ROWS as libc::c_ushort,
        ws_col: DEFAULT_COLS as libc::c_ushort,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    // SAFETY: all output pointers refer to live writable values. The optional name and
    // termios pointers are null. winsize is mutable on macOS and const on Linux; a raw
    // mutable pointer supports both libc signatures without creating an unused &mut.
    let result = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(winsize),
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("open Unix PTY");
    }

    // SAFETY: successful openpty transferred two distinct owned descriptors to us.
    let master = unsafe { File::from_raw_fd(master_fd as RawFd) };
    // SAFETY: the slave descriptor is valid, distinct from master, and owned here.
    let slave = unsafe { File::from_raw_fd(slave_fd as RawFd) };
    Ok((master, slave))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn pty_has_distinct_owned_descriptors_and_default_dimensions() -> Result<()> {
        let (master, slave) = open_unix_pty()?;
        assert_ne!(master.as_raw_fd(), slave.as_raw_fd());
        let mut size = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: slave remains open and size is writable for the complete winsize result.
        let result = unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCGWINSZ, &mut size) };
        assert_eq!(result, 0);
        assert_eq!(size.ws_row, DEFAULT_ROWS as libc::c_ushort);
        assert_eq!(size.ws_col, DEFAULT_COLS as libc::c_ushort);
        Ok(())
    }
}
