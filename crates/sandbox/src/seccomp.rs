//! seccomp filter construction.
//!
//! An allowlist for the serving loop and nothing else. The list is measured
//! from a traced run, not guessed, and `make sandbox` exercises the real
//! workload under the live filter — a missing entry fails the tests loudly.

use std::fmt;

const PR_SET_NO_NEW_PRIVS: libc::c_int = 38;
const SECCOMP_SET_MODE_FILTER: libc::c_ulong = 1;
const SECCOMP_FILTER_FLAG_TSYNC: libc::c_ulong = 1;

const RET_KILL_PROCESS: u32 = 0x8000_0000;
const RET_ERRNO_EPERM: u32 = 0x0005_0000 | (libc::EPERM as u32 & 0xffff);
const RET_LOG: u32 = 0x7ffc_0000;
const RET_ALLOW: u32 = 0x7fff_0000;

// Classic BPF opcodes.
const LD_W_ABS: u16 = 0x20;
const JMP_JEQ_K: u16 = 0x15;
const JMP_JGE_K: u16 = 0x35;
const RET_K: u16 = 0x06;

// Offsets into struct seccomp_data.
const OFF_NR: u32 = 0;
const OFF_ARCH: u32 = 4;

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xc000_003e;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xc000_00b7;
#[cfg(target_arch = "riscv64")]
const AUDIT_ARCH: u32 = 0xc000_00f3;

#[cfg(target_arch = "x86_64")]
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

/// What the kernel does with a syscall that is not on the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Action {
    /// Kill the whole process. The default.
    #[default]
    Kill,
    /// Fail the call with `EPERM`. For diagnosing a missing entry.
    Errno,
    /// Allow, but log. Development only.
    Log,
}

impl Action {
    fn ret(self) -> u32 {
        match self {
            Action::Kill => RET_KILL_PROCESS,
            Action::Errno => RET_ERRNO_EPERM,
            Action::Log => RET_LOG,
        }
    }

    /// Name used in configuration and in `/v1/status`.
    pub fn name(self) -> &'static str {
        match self {
            Action::Kill => "kill",
            Action::Errno => "errno",
            Action::Log => "log",
        }
    }

    /// Parse the configuration spelling.
    pub fn parse(s: &str) -> Option<Action> {
        match s {
            "kill" => Some(Action::Kill),
            "errno" => Some(Action::Errno),
            "log" => Some(Action::Log),
            _ => None,
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Syscalls the serving loop makes once confinement is in place.
///
/// Notably absent: `openat`, `clone`, `execve`, `socket`, `connect`, `bind`.
/// Archives are already mapped, workers are already spawned, and the listener
/// is already bound before this filter is installed.
pub fn allowed_syscalls() -> Vec<libc::c_long> {
    let mut v: Vec<libc::c_long> = vec![
        // Serving a connection.
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_readv,
        libc::SYS_writev,
        libc::SYS_pread64,
        libc::SYS_close,
        // std checks FD_CLOEXEC on an accepted socket with F_GETFD.
        libc::SYS_fcntl,
        libc::SYS_accept4,
        libc::SYS_shutdown,
        libc::SYS_setsockopt,
        libc::SYS_getsockopt,
        libc::SYS_getsockname,
        libc::SYS_getpeername,
        libc::SYS_recvfrom,
        libc::SYS_sendto,
        libc::SYS_recvmsg,
        libc::SYS_sendmsg,
        libc::SYS_ppoll,
        // Memory: decompression buffers, the cluster cache, page faults.
        libc::SYS_mmap,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_mprotect,
        libc::SYS_madvise,
        libc::SYS_brk,
        // Threads already exist; these are what they use to coordinate.
        libc::SYS_futex,
        libc::SYS_sched_yield,
        libc::SYS_set_robust_list,
        libc::SYS_rseq,
        libc::SYS_membarrier,
        // glibc sizes a new malloc arena the first time a worker allocates.
        libc::SYS_sched_getaffinity,
        // Time and randomness.
        libc::SYS_clock_gettime,
        libc::SYS_clock_nanosleep,
        libc::SYS_nanosleep,
        libc::SYS_getrandom,
        // Signals and exit, including the panic path.
        libc::SYS_rt_sigreturn,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_sigaltstack,
        libc::SYS_restart_syscall,
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_tgkill,
        libc::SYS_exit,
        libc::SYS_exit_group,
        // Stat on descriptors already open, for stderr and for archive files.
        libc::SYS_fstat,
        libc::SYS_statx,
        libc::SYS_lseek,
    ];
    #[cfg(target_arch = "x86_64")]
    {
        v.push(libc::SYS_poll);
        v.push(libc::SYS_newfstatat);
    }
    #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
    {
        v.push(libc::SYS_newfstatat);
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// One classic BPF instruction.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Insn {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
#[derive(Debug)]
struct Prog {
    len: u16,
    filter: *const Insn,
}

/// Build the filter program: check the architecture, then the allowlist.
fn program(allowed: &[libc::c_long], action: Action) -> Vec<Insn> {
    let mut p = Vec::with_capacity(allowed.len() * 2 + 8);
    p.push(Insn {
        code: LD_W_ABS,
        jt: 0,
        jf: 0,
        k: OFF_ARCH,
    });
    p.push(Insn {
        code: JMP_JEQ_K,
        jt: 1,
        jf: 0,
        k: AUDIT_ARCH,
    });
    p.push(Insn {
        code: RET_K,
        jt: 0,
        jf: 0,
        k: RET_KILL_PROCESS,
    });
    p.push(Insn {
        code: LD_W_ABS,
        jt: 0,
        jf: 0,
        k: OFF_NR,
    });
    #[cfg(target_arch = "x86_64")]
    {
        // The x32 ABI reuses syscall numbers with a high bit set.
        p.push(Insn {
            code: JMP_JGE_K,
            jt: 0,
            jf: 1,
            k: X32_SYSCALL_BIT,
        });
        p.push(Insn {
            code: RET_K,
            jt: 0,
            jf: 0,
            k: RET_KILL_PROCESS,
        });
    }
    for &nr in allowed {
        // Two instructions per syscall keeps every jump offset at 0 or 1, so
        // the list can grow without overflowing an 8-bit jump.
        p.push(Insn {
            code: JMP_JEQ_K,
            jt: 0,
            jf: 1,
            k: nr as u32,
        });
        p.push(Insn {
            code: RET_K,
            jt: 0,
            jf: 0,
            k: RET_ALLOW,
        });
    }
    p.push(Insn {
        code: RET_K,
        jt: 0,
        jf: 0,
        k: action.ret(),
    });
    p
}

/// Set `no_new_privs`, which seccomp requires and Landlock relies on.
pub fn set_no_new_privs() -> Result<(), std::io::Error> {
    // SAFETY: prctl with PR_SET_NO_NEW_PRIVS takes scalar arguments only.
    let rc = unsafe { libc::prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if rc != 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Install the filter on every thread in the process.
///
/// `TSYNC` is required, not best-effort: a filter that covered only the
/// calling thread would leave every worker unconfined.
pub fn install(allowed: &[libc::c_long], action: Action) -> Result<usize, std::io::Error> {
    let insns = program(allowed, action);
    let prog = Prog {
        len: insns.len() as u16,
        filter: insns.as_ptr(),
    };
    // SAFETY: `insns` outlives the call, `prog.len` matches its length, and
    // the filter is a well-formed classic BPF program ending in a return.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            SECCOMP_SET_MODE_FILTER,
            SECCOMP_FILTER_FLAG_TSYNC,
            &prog as *const Prog,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(insns.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_shape() {
        let allowed = allowed_syscalls();
        let p = program(&allowed, Action::Kill);
        assert_eq!(p[0].code, LD_W_ABS);
        assert_eq!(p[0].k, OFF_ARCH);
        assert_eq!(p[1].k, AUDIT_ARCH);
        assert_eq!(p[2].k, RET_KILL_PROCESS);
        assert_eq!(p.last().unwrap().code, RET_K);
        assert_eq!(p.last().unwrap().k, RET_KILL_PROCESS);
        // Two instructions per allowed syscall, plus the prologue and default.
        assert!(p.len() >= allowed.len() * 2 + 4);
        assert!(p.len() < u16::MAX as usize);
    }

    #[test]
    fn actions_round_trip() {
        for a in [Action::Kill, Action::Errno, Action::Log] {
            assert_eq!(Action::parse(a.name()), Some(a));
        }
        assert_eq!(Action::parse("allow"), None);
        assert_ne!(Action::Errno.ret(), Action::Kill.ret());
    }

    #[test]
    fn the_list_denies_what_it_must() {
        let allowed = allowed_syscalls();
        for denied in [
            libc::SYS_openat,
            libc::SYS_execve,
            libc::SYS_socket,
            libc::SYS_connect,
        ] {
            assert!(
                !allowed.contains(&denied),
                "syscall {denied} must not be allowed"
            );
        }
        assert!(allowed.contains(&libc::SYS_read));
        assert!(allowed.contains(&libc::SYS_accept4));
        // Sorted and deduplicated, so the filter has no redundant branches.
        let mut sorted = allowed.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(allowed, sorted);
    }
}
