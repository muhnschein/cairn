//! Landlock ruleset construction.
//!
//! Read access to the archive directory, nothing else, and no filesystem write
//! capability anywhere. Built directly on the three syscalls so the crate has
//! no dependency beyond `libc`.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

// Syscall numbers are architecture-independent for Landlock (Linux 5.13).
const SYS_CREATE_RULESET: libc::c_long = 444;
const SYS_ADD_RULE: libc::c_long = 445;
const SYS_RESTRICT_SELF: libc::c_long = 446;

const CREATE_RULESET_VERSION: u32 = 1;
const RULE_PATH_BENEATH: libc::c_int = 1;

const ACCESS_FS_READ_FILE: u64 = 1 << 2;
const ACCESS_FS_READ_DIR: u64 = 1 << 3;

const ACCESS_NET_BIND_TCP: u64 = 1 << 0;
const ACCESS_NET_CONNECT_TCP: u64 = 1 << 1;

#[repr(C)]
#[derive(Debug, Default)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
}

#[repr(C, packed)]
#[derive(Debug)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

/// Landlock ABI version supported by the running kernel.
pub fn abi_version() -> Result<i32, std::io::Error> {
    // SAFETY: the version query passes a null attribute with size 0, which is
    // what LANDLOCK_CREATE_RULESET_VERSION requires; it returns a version
    // number and creates nothing.
    let rc = unsafe {
        libc::syscall(SYS_CREATE_RULESET, std::ptr::null::<RulesetAttr>(), 0usize, CREATE_RULESET_VERSION)
    };
    if rc < 0 { Err(std::io::Error::last_os_error()) } else { Ok(rc as i32) }
}

/// Every filesystem right this ABI knows about, so all of them are denied by
/// default and only reads are granted back.
fn handled_fs(abi: i32) -> u64 {
    match abi {
        1 => 0x1fff,          // through MAKE_SYM
        2 => 0x3fff,          // + REFER
        3 | 4 => 0x7fff,      // + TRUNCATE
        _ => 0xffff,          // + IOCTL_DEV (ABI 5 and later)
    }
}

fn handled_net(abi: i32) -> u64 {
    if abi >= 4 { ACCESS_NET_BIND_TCP | ACCESS_NET_CONNECT_TCP } else { 0 }
}

/// Apply a read-only ruleset over `read_only`, denying everything else.
///
/// `no_new_privs` must already be set. Returns the ABI version in force.
pub fn restrict(read_only: &[&Path], abi: i32) -> Result<i32, std::io::Error> {
    let attr = RulesetAttr { handled_access_fs: handled_fs(abi), handled_access_net: handled_net(abi) };
    // Kernels below ABI 4 do not know the net field and reject the larger size.
    let size = if abi >= 4 { size_of::<RulesetAttr>() } else { size_of::<u64>() };

    // SAFETY: `attr` outlives the call and `size` matches what this ABI accepts.
    let ruleset = unsafe { libc::syscall(SYS_CREATE_RULESET, &attr as *const RulesetAttr, size, 0u32) };
    if ruleset < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let ruleset = ruleset as libc::c_int;
    let close_ruleset = OwnedFd(ruleset);

    for path in read_only {
        let c = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let parent = OwnedFd(fd);
        let rule = PathBeneathAttr {
            allowed_access: ACCESS_FS_READ_FILE | ACCESS_FS_READ_DIR,
            parent_fd: fd,
        };
        // SAFETY: `rule` describes the fd held open by `parent` and outlives the call.
        let rc = unsafe {
            libc::syscall(SYS_ADD_RULE, ruleset, RULE_PATH_BENEATH, &rule as *const PathBeneathAttr, 0u32)
        };
        drop(parent);
        if rc < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    // SAFETY: restricting self takes only the ruleset fd and a flag word.
    let rc = unsafe { libc::syscall(SYS_RESTRICT_SELF, ruleset, 0u32) };
    drop(close_ruleset);
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(abi)
}

/// Closes a descriptor on drop. Local to this module; nothing else needs it.
struct OwnedFd(libc::c_int);

impl Drop for OwnedFd {
    fn drop(&mut self) {
        // SAFETY: the descriptor is owned here and closed exactly once.
        unsafe { libc::close(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handled_masks_grow_with_the_abi() {
        assert_eq!(handled_fs(1), 0x1fff);
        assert!(handled_fs(3) > handled_fs(2));
        assert_eq!(handled_net(3), 0);
        assert_ne!(handled_net(4), 0);
        // An unknown future ABI is treated as the newest one we understand.
        assert_eq!(handled_fs(99), handled_fs(5));
    }

    #[test]
    fn path_beneath_attr_is_packed() {
        assert_eq!(size_of::<PathBeneathAttr>(), 12);
        assert_eq!(size_of::<RulesetAttr>(), 16);
    }
}
