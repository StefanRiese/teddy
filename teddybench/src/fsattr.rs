//! The Toniebox marks "live" content with the FAT hidden attribute of the Tonie file.
//!
//! - Windows: the hidden file attribute
//! - macOS: the `UF_HIDDEN` flag, which the msdos file system maps to the FAT attribute
//! - Linux: the FAT attribute ioctls of the vfat driver (only works on FAT file systems)

use std::io;
use std::path::Path;

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, INVALID_FILE_ATTRIBUTES};

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    fn attributes(path: &Path) -> io::Result<u32> {
        let attrs = unsafe { GetFileAttributesW(wide(path).as_ptr()) };
        if attrs == INVALID_FILE_ATTRIBUTES {
            return Err(io::Error::last_os_error());
        }
        Ok(attrs)
    }

    pub fn is_hidden(path: &Path) -> io::Result<bool> {
        Ok(attributes(path)? & FILE_ATTRIBUTE_HIDDEN != 0)
    }

    pub fn set_hidden(path: &Path, hidden: bool) -> io::Result<()> {
        let attrs = attributes(path)?;
        let attrs = if hidden {
            attrs | FILE_ATTRIBUTE_HIDDEN
        } else {
            attrs & !FILE_ATTRIBUTE_HIDDEN
        };
        if unsafe { SetFileAttributesW(wide(path).as_ptr(), attrs) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn c_path(path: &Path) -> io::Result<CString> {
        CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
    }

    fn flags(path: &Path) -> io::Result<u32> {
        let p = c_path(path)?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::stat(p.as_ptr(), &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(st.st_flags)
    }

    pub fn is_hidden(path: &Path) -> io::Result<bool> {
        Ok(flags(path)? & libc::UF_HIDDEN != 0)
    }

    pub fn set_hidden(path: &Path, hidden: bool) -> io::Result<()> {
        let f = flags(path)?;
        let f = if hidden { f | libc::UF_HIDDEN } else { f & !libc::UF_HIDDEN };
        let p = c_path(path)?;
        if unsafe { libc::chflags(p.as_ptr(), f) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::fs::File;
    use std::io;
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    /* from linux/msdos_fs.h */
    const FAT_IOCTL_GET_ATTRIBUTES: libc::c_ulong = 0x8004_7210;
    const FAT_IOCTL_SET_ATTRIBUTES: libc::c_ulong = 0x4004_7211;
    const ATTR_HIDDEN: u32 = 2;

    fn not_fat(e: io::Error) -> io::Error {
        if e.raw_os_error() == Some(libc::ENOTTY) {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "the live flag can only be read and set on FAT file systems (the SD card)",
            )
        } else {
            e
        }
    }

    fn attributes(file: &File) -> io::Result<u32> {
        let mut attrs: u32 = 0;
        if unsafe { libc::ioctl(file.as_raw_fd(), FAT_IOCTL_GET_ATTRIBUTES as _, &mut attrs) } != 0 {
            return Err(not_fat(io::Error::last_os_error()));
        }
        Ok(attrs)
    }

    pub fn is_hidden(path: &Path) -> io::Result<bool> {
        Ok(attributes(&File::open(path)?)? & ATTR_HIDDEN != 0)
    }

    pub fn set_hidden(path: &Path, hidden: bool) -> io::Result<()> {
        let file = File::open(path)?;
        let attrs = attributes(&file)?;
        let attrs = if hidden { attrs | ATTR_HIDDEN } else { attrs & !ATTR_HIDDEN };
        if unsafe { libc::ioctl(file.as_raw_fd(), FAT_IOCTL_SET_ATTRIBUTES as _, &attrs) } != 0 {
            return Err(not_fat(io::Error::last_os_error()));
        }
        Ok(())
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    use std::io;
    use std::path::Path;

    pub fn is_hidden(_path: &Path) -> io::Result<bool> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub fn set_hidden(_path: &Path, _hidden: bool) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

/// Whether the Tonie file is marked as live content.
pub fn is_live(path: &Path) -> io::Result<bool> {
    imp::is_hidden(path)
}

pub fn set_live(path: &Path, live: bool) -> io::Result<()> {
    imp::set_hidden(path, live)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles_or_reports_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("500304E0");
        std::fs::write(&file, b"x").unwrap();
        match set_live(&file, true) {
            Ok(()) => {
                assert!(is_live(&file).unwrap());
                set_live(&file, false).unwrap();
                assert!(!is_live(&file).unwrap());
            }
            /* temp directories on Linux are not FAT */
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::Unsupported),
        }
    }
}
