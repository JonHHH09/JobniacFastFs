//! Linux confinement of a worker child.
//!
//! The child confines itself, after it has loaded everything it needs from the file system and
//! before it parses any untrusted input. Three layers apply, in this order:
//!
//! 1. `PR_SET_NO_NEW_PRIVS`, read back with `PR_GET_NO_NEW_PRIVS`;
//! 2. a Landlock ruleset (best-effort ABI negotiation up to ABI v6) that handles every file system
//!    access right and grants none, so the child can use only descriptors it already holds. The PDF
//!    profile also handles TCP bind and connect (ABI v4), and both profiles scope abstract UNIX
//!    sockets and signals to the child (ABI v6);
//! 3. a seccomp-BPF filter, applied to every thread and read back with `PR_GET_SECCOMP`, that
//!    answers `EPERM` to the calls an exploited parser would use to get out: sockets (all of them
//!    for the PDF profile, anything but IPv4 and IPv6 for the fetch profile), `execve`, `ptrace`,
//!    cross-process memory access, mounts, namespaces, keyrings, `bpf`, `perf_event_open`,
//!    `io_uring`, and the module, reboot and swap calls. `clone3` answers `ENOSYS` (so the C library
//!    falls back to `clone`, whose namespace flags are filtered).
//!
//! Failure policy: a failure to set `no_new_privs` or to install and read back the seccomp filter
//! is an error, and the child refuses to run. A kernel without Landlock (or with an older ABI) is
//! not an error: the child runs with what the kernel offers and reports the level it reached, so the
//! caller never claims more than was enforced.
//!
//! Landlock restricts the calling thread only (and its later children), so the child must still be
//! single-threaded here; a second thread is an error. The seccomp filter covers all threads.

#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::io;

use landlock::{
    ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset,
    RulesetAttr, RulesetCreatedAttr, RulesetStatus, Scope, make_bitflags,
};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

use crate::workers::sandbox::{SandboxLevel, SandboxProfile};

/// The Landlock ABI the rulesets ask for. Older kernels are served by best-effort negotiation.
const REQUESTED_ABI: ABI = ABI::V6;

/// The `clone` flags that create a namespace. A call that sets any of them is refused.
const NAMESPACE_CLONE_FLAGS: [u64; 8] = [
    libc::CLONE_NEWNS as u64,
    libc::CLONE_NEWCGROUP as u64,
    libc::CLONE_NEWUTS as u64,
    libc::CLONE_NEWIPC as u64,
    libc::CLONE_NEWUSER as u64,
    libc::CLONE_NEWPID as u64,
    libc::CLONE_NEWNET as u64,
    // `CLONE_NEWTIME`, which the `libc` crate does not name.
    0x0000_0080,
];

/// The system font directories that PDFium searches on Linux (`CFX_LinuxFontInfo`). The render
/// profile may read them; a directory that does not exist is skipped.
const PDFIUM_FONT_DIRECTORIES: &[&str] = &[
    "/usr/share/fonts",
    "/usr/local/share/fonts",
    "/usr/share/X11/fonts/Type1",
    "/usr/share/X11/fonts/TTF",
];

/// The syscalls that every profile refuses, whatever their arguments.
const DENIED_EVERYWHERE: &[libc::c_long] = &[
    libc::SYS_execve,
    libc::SYS_execveat,
    libc::SYS_ptrace,
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
    libc::SYS_mount,
    libc::SYS_umount2,
    libc::SYS_pivot_root,
    libc::SYS_chroot,
    libc::SYS_unshare,
    libc::SYS_setns,
    libc::SYS_keyctl,
    libc::SYS_add_key,
    libc::SYS_request_key,
    libc::SYS_bpf,
    libc::SYS_perf_event_open,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    libc::SYS_kexec_load,
    libc::SYS_init_module,
    libc::SYS_finit_module,
    libc::SYS_delete_module,
    libc::SYS_reboot,
    libc::SYS_swapon,
    libc::SYS_swapoff,
    libc::SYS_userfaultfd,
    libc::SYS_fanotify_init,
    libc::SYS_open_by_handle_at,
    libc::SYS_move_mount,
    libc::SYS_open_tree,
    libc::SYS_fsopen,
    libc::SYS_fsconfig,
    libc::SYS_fsmount,
    libc::SYS_fspick,
    libc::SYS_mount_setattr,
    libc::SYS_kexec_file_load,
    libc::SYS_pidfd_getfd,
    libc::SYS_pidfd_send_signal,
    libc::SYS_tkill,
    libc::SYS_kcmp,
    libc::SYS_quotactl,
    libc::SYS_acct,
    libc::SYS_settimeofday,
    libc::SYS_clock_settime,
    libc::SYS_clock_adjtime,
    libc::SYS_adjtimex,
    libc::SYS_sethostname,
    libc::SYS_setdomainname,
    libc::SYS_syslog,
    libc::SYS_shmget,
    libc::SYS_shmat,
    libc::SYS_shmctl,
    libc::SYS_shmdt,
    libc::SYS_msgget,
    libc::SYS_msgsnd,
    libc::SYS_msgrcv,
    libc::SYS_msgctl,
    libc::SYS_semget,
    libc::SYS_semop,
    libc::SYS_semtimedop,
    libc::SYS_semctl,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_iopl,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_ioperm,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_modify_ldt,
];

/// The signal-sending syscalls whose first argument is a process id: refused unless it is the
/// child's own pid. This keeps `kill(0, ..)`, `kill(-1, ..)`, the parent and every other process out
/// of reach on every Landlock ABI (the signal scope only exists from ABI v6, and does not stop
/// `kill(-1, ..)`).
const OWN_PID_ONLY: &[libc::c_long] = &[
    libc::SYS_kill,
    libc::SYS_tgkill,
    libc::SYS_rt_sigqueueinfo,
    libc::SYS_rt_tgsigqueueinfo,
];

/// The first system call number of the x32 ABI (`__X32_SYSCALL_BIT`). A filter that matches numbers
/// by equality would let `nr | 0x4000_0000` through to the kernel's x32 table.
#[cfg(target_arch = "x86_64")]
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

/// The socket-family syscalls that the PDF profile refuses outright.
const DENIED_SOCKET_CALLS: &[libc::c_long] = &[
    libc::SYS_socket,
    libc::SYS_socketpair,
    libc::SYS_connect,
    libc::SYS_bind,
    libc::SYS_listen,
    libc::SYS_accept,
    libc::SYS_accept4,
];

/// The seccomp architecture of this build, or `None` where no filter can be built (the child then
/// refuses to run).
const fn target_arch() -> Option<TargetArch> {
    if cfg!(target_arch = "x86_64") {
        Some(TargetArch::x86_64)
    } else if cfg!(target_arch = "aarch64") {
        Some(TargetArch::aarch64)
    } else {
        None
    }
}

fn failure(message: &'static str) -> io::Error {
    io::Error::other(message)
}

/// Confines the calling process for `profile` and returns the level that was reached.
///
/// # Errors
///
/// Returns an error when `no_new_privs` or the seccomp filter cannot be installed and read back,
/// when the process has more than one thread, or when a Landlock call fails for a reason other than
/// the kernel lacking the feature.
pub fn apply(profile: SandboxProfile) -> io::Result<SandboxLevel> {
    ensure_single_thread()?;
    set_no_new_privs()?;
    drop_capabilities()?;
    let landlock = apply_landlock(profile)?;
    install_seccomp(profile)?;
    Ok(match landlock {
        RulesetStatus::FullyEnforced => SandboxLevel::Full,
        RulesetStatus::PartiallyEnforced => SandboxLevel::Degraded,
        RulesetStatus::NotEnforced => SandboxLevel::SeccompOnly,
    })
}

/// The highest Landlock ABI version of the running kernel, or `None` without Landlock (no kernel
/// support, a disabled LSM, or a policy that refuses the call). Used by the parent to report a
/// degraded host once.
#[must_use]
pub fn host_landlock_abi() -> Option<u32> {
    // `LANDLOCK_CREATE_RULESET_VERSION` is `1 << 0`; with a null attribute it returns the version.
    const CREATE_RULESET_VERSION: libc::c_ulong = 1;
    // SAFETY: `landlock_create_ruleset` with a null attribute, size 0 and the VERSION flag only
    // returns the ABI version number; it reads no memory and creates no object.
    let version = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0 as libc::size_t,
            CREATE_RULESET_VERSION,
        )
    };
    u32::try_from(version).ok().filter(|version| *version > 0)
}

/// Refuses to confine a process that already has other threads, because Landlock would leave them
/// outside the ruleset.
fn ensure_single_thread() -> io::Result<()> {
    let threads = std::fs::read_dir("/proc/self/task")?.count();
    if threads == 1 {
        Ok(())
    } else {
        Err(failure("the worker has more than one thread"))
    }
}

fn set_no_new_privs() -> io::Result<()> {
    // SAFETY: `prctl` with `PR_SET_NO_NEW_PRIVS` takes plain integers, reads no memory and only
    // sets a one-way flag on this process.
    let set = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if set != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `PR_GET_NO_NEW_PRIVS` takes plain integers and returns the flag.
    let read_back = unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) };
    if read_back == 1 {
        Ok(())
    } else {
        Err(failure("no_new_privs does not read back"))
    }
}

/// Whether `no_new_privs` is set and a seccomp filter is installed: the tripwire that a child
/// checks before it returns a response.
#[must_use]
pub fn confined() -> bool {
    // SAFETY: both `prctl` queries take plain integers, read no memory and only return a value.
    unsafe {
        libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) == 1
            && libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) == 2
    }
}

/// Closes every descriptor above stderr, so nothing the parent leaked survives into the child.
///
/// Called at the start of the child, before it opens anything of its own. Uses `close_range`, and
/// falls back to closing the entries of `/proc/self/fd` on a kernel without it.
///
/// # Errors
///
/// Returns an error when neither way works.
pub fn close_inherited_descriptors() -> io::Result<()> {
    // SAFETY: `close_range` takes plain integers and closes descriptors of this process only.
    let closed = unsafe { libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, 0_u32) };
    if closed == 0 {
        return Ok(());
    }
    let numbers: Vec<libc::c_int> = std::fs::read_dir("/proc/self/fd")?
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
        .filter(|number| *number > 2)
        .collect();
    for number in numbers {
        // SAFETY: closing a descriptor number of this process; the directory's own descriptor is
        // already closed by the time the iterator is dropped, so an `EBADF` is ignored.
        unsafe {
            libc::close(number);
        }
    }
    Ok(())
}

/// The layout of `__user_cap_header_struct`.
#[repr(C)]
struct CapHeader {
    version: u32,
    pid: libc::c_int,
}

/// The layout of `__user_cap_data_struct`.
#[repr(C)]
#[derive(Clone, Copy)]
struct CapData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// `_LINUX_CAPABILITY_VERSION_3`.
const CAPABILITY_VERSION_3: u32 = 0x2008_0522;

/// Drops every capability when the process runs as root: the bounding set (so none can come back
/// through an exec), the ambient set, and the effective, permitted and inheritable sets. A no-op
/// for an unprivileged process, which has none to drop.
fn drop_capabilities() -> io::Result<()> {
    // SAFETY: `geteuid` takes no arguments and cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        return Ok(());
    }
    for capability in 0..64_u64 {
        // SAFETY: `PR_CAPBSET_DROP` takes plain integers; `EINVAL` marks the first number past the
        // last capability of the kernel.
        let dropped = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, capability, 0, 0, 0) };
        if dropped != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINVAL) {
                break;
            }
            return Err(error);
        }
    }
    // SAFETY: `PR_CAP_AMBIENT_CLEAR_ALL` takes plain integers. A kernel without ambient
    // capabilities answers `EINVAL`, which leaves nothing to clear.
    unsafe {
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL,
            0,
            0,
            0,
        );
    }
    let mut header = CapHeader {
        version: CAPABILITY_VERSION_3,
        pid: 0,
    };
    let data = [CapData {
        effective: 0,
        permitted: 0,
        inheritable: 0,
    }; 2];
    // SAFETY: `capset` reads one header and two data structs (version 3) of the layouts above,
    // which stay alive for the call.
    let set = unsafe { libc::syscall(libc::SYS_capset, &mut header, data.as_ptr()) };
    if set == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Applies the Landlock ruleset of `profile`: every file system access right is handled and none
/// is granted, so no path can be opened, created, removed or changed any more. Descriptors that
/// are already open keep working.
fn apply_landlock(profile: SandboxProfile) -> io::Result<RulesetStatus> {
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(REQUESTED_ABI))
        .map_err(io::Error::other)?;
    if profile != SandboxProfile::Fetch {
        ruleset = ruleset
            .handle_access(AccessNet::from_all(REQUESTED_ABI))
            .map_err(io::Error::other)?;
    }
    let mut ruleset = ruleset
        .scope(Scope::from_all(REQUESTED_ABI))
        .map_err(io::Error::other)?
        .create()
        .map_err(io::Error::other)?;
    if profile == SandboxProfile::PdfRender {
        for directory in PDFIUM_FONT_DIRECTORIES {
            let fd = match PathFd::new(directory) {
                Ok(fd) => fd,
                Err(_) if !std::path::Path::new(directory).exists() => continue,
                Err(error) => return Err(io::Error::other(error)),
            };
            ruleset = ruleset
                .add_rule(PathBeneath::new(
                    fd,
                    make_bitflags!(AccessFs::{ReadFile | ReadDir}),
                ))
                .map_err(io::Error::other)?;
        }
    }
    let status = ruleset.restrict_self().map_err(io::Error::other)?;
    Ok(status.ruleset)
}

/// The syscall number as the key type of a seccomp rule map.
fn number(syscall: libc::c_long) -> i64 {
    syscall
}

/// The main filter: the denied calls of `profile` answer `EPERM`, everything else is allowed.
fn main_filter(profile: SandboxProfile, arch: TargetArch, own_pid: u32) -> io::Result<BpfProgram> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    for syscall in DENIED_EVERYWHERE {
        rules.insert(number(*syscall), Vec::new());
    }
    match profile {
        SandboxProfile::PdfExtract | SandboxProfile::PdfRender => {
            for syscall in DENIED_SOCKET_CALLS {
                rules.insert(number(*syscall), Vec::new());
            }
        }
        SandboxProfile::Fetch => {
            // Only IPv4 and IPv6 sockets (TCP, and UDP for DNS). `socket` is refused when its
            // domain is neither.
            let not_inet = SeccompCondition::new(
                0,
                SeccompCmpArgLen::Dword,
                SeccompCmpOp::Ne,
                u64::from(libc::AF_INET as u32),
            )
            .map_err(io::Error::other)?;
            let not_inet6 = SeccompCondition::new(
                0,
                SeccompCmpArgLen::Dword,
                SeccompCmpOp::Ne,
                u64::from(libc::AF_INET6 as u32),
            )
            .map_err(io::Error::other)?;
            let rule = SeccompRule::new(vec![not_inet, not_inet6]).map_err(io::Error::other)?;
            rules.insert(number(libc::SYS_socket), vec![rule]);
            // Local socket pairs, and listening, are never part of a fetch.
            for syscall in [libc::SYS_socketpair, libc::SYS_listen, libc::SYS_accept] {
                rules.insert(number(syscall), Vec::new());
            }
            rules.insert(number(libc::SYS_accept4), Vec::new());
        }
    }
    // `clone` with any namespace flag. One rule per flag: the rules of a syscall are alternatives.
    let mut clone_rules = Vec::new();
    for flag in NAMESPACE_CLONE_FLAGS {
        let condition = SeccompCondition::new(
            0,
            SeccompCmpArgLen::Qword,
            SeccompCmpOp::MaskedEq(flag),
            flag,
        )
        .map_err(io::Error::other)?;
        clone_rules.push(SeccompRule::new(vec![condition]).map_err(io::Error::other)?);
    }
    rules.insert(number(libc::SYS_clone), clone_rules);
    // Signals only to the child's own process.
    for syscall in OWN_PID_ONLY {
        let other_pid = SeccompCondition::new(
            0,
            SeccompCmpArgLen::Dword,
            SeccompCmpOp::Ne,
            u64::from(own_pid),
        )
        .map_err(io::Error::other)?;
        rules.insert(
            number(*syscall),
            vec![SeccompRule::new(vec![other_pid]).map_err(io::Error::other)?],
        );
    }
    // `personality` may query (`0xffff_ffff`), never change.
    let change_persona = SeccompCondition::new(
        0,
        SeccompCmpArgLen::Dword,
        SeccompCmpOp::Ne,
        u64::from(u32::MAX),
    )
    .map_err(io::Error::other)?;
    rules.insert(
        number(libc::SYS_personality),
        vec![SeccompRule::new(vec![change_persona]).map_err(io::Error::other)?],
    );
    let mut program = compile(rules, SeccompAction::Errno(libc::EPERM as u32), arch)?;
    reject_x32(&mut program);
    Ok(program)
}

/// Puts a check in front of the program that refuses every x32 system call number, so that a
/// number with the x32 bit set cannot slip past the per-number rules.
#[cfg(target_arch = "x86_64")]
fn reject_x32(program: &mut BpfProgram) {
    use seccompiler::sock_filter;
    // `BPF_LD | BPF_W | BPF_ABS` of `seccomp_data.nr` (offset 0), `BPF_JMP | BPF_JGE | BPF_K`
    // (skip the return when the number is below the bit), and `BPF_RET | BPF_K` with `EPERM`.
    let denied = 0x0005_0000 | libc::EPERM as u32;
    let guard = [
        sock_filter {
            code: 0x20,
            jt: 0,
            jf: 0,
            k: 0,
        },
        sock_filter {
            code: 0x35,
            jt: 0,
            jf: 1,
            k: X32_SYSCALL_BIT,
        },
        sock_filter {
            code: 0x06,
            jt: 0,
            jf: 0,
            k: denied,
        },
    ];
    program.splice(0..0, guard);
}

/// Other CPUs have no second system call table to guard.
#[cfg(not(target_arch = "x86_64"))]
fn reject_x32(_program: &mut BpfProgram) {}

/// The `clone3` filter: `ENOSYS`, so that glibc falls back to `clone` (whose flags are checked by
/// the main filter), because `clone3` passes its flags in memory that a filter cannot inspect.
fn clone3_filter(arch: TargetArch) -> io::Result<BpfProgram> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    rules.insert(number(libc::SYS_clone3), Vec::new());
    compile(rules, SeccompAction::Errno(libc::ENOSYS as u32), arch)
}

fn compile(
    rules: BTreeMap<i64, Vec<SeccompRule>>,
    on_match: SeccompAction,
    arch: TargetArch,
) -> io::Result<BpfProgram> {
    SeccompFilter::new(rules, SeccompAction::Allow, on_match, arch)
        .map_err(io::Error::other)?
        .try_into()
        .map_err(io::Error::other)
}

/// Installs both filters on every thread and reads the seccomp mode back.
fn install_seccomp(profile: SandboxProfile) -> io::Result<()> {
    let arch = target_arch().ok_or_else(|| failure("no seccomp filter exists for this CPU"))?;
    seccompiler::apply_filter_all_threads(&clone3_filter(arch)?).map_err(io::Error::other)?;
    // SAFETY: `getpid` takes no arguments and cannot fail. The child is single-threaded here, so the
    // process id is also the only thread id.
    let own_pid = u32::try_from(unsafe { libc::getpid() }).map_err(io::Error::other)?;
    seccompiler::apply_filter_all_threads(&main_filter(profile, arch, own_pid)?)
        .map_err(io::Error::other)?;
    // SAFETY: `PR_GET_SECCOMP` takes plain integers and returns the mode of this thread.
    let mode = unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) };
    if mode == 2 {
        Ok(())
    } else {
        Err(failure("the seccomp filter does not read back"))
    }
}

/// Makes one raw system call that the filters refuse (or, for the `*_self` and `*_query` names, must
/// allow), for the test-only probe. Returns the error of the call, or `Ok` when it succeeded.
///
/// The names are fixed; an unknown name is `InvalidInput`.
///
/// # Errors
///
/// The OS error of the call, which is what the probe reports.
#[cfg(any(test, feature = "worker-probe"))]
pub fn probe_syscall(name: &str) -> io::Result<()> {
    // SAFETY (all arms): each call passes plain integers or null pointers that the kernel does not
    // dereference before the filter (or its own argument check) answers; `kill`, `tgkill` and
    // `pidfd_send_signal` use signal 0, which only checks permission, and the `clone` call that a
    // filter lets through carries no `CLONE_VM`, so no memory is shared.
    let result = unsafe {
        match name {
            "kill_parent" => libc::syscall(libc::SYS_kill, libc::getppid(), 0),
            "kill_all" => libc::syscall(libc::SYS_kill, -1, 0),
            "kill_group" => libc::syscall(libc::SYS_kill, 0, 0),
            "kill_self" => libc::syscall(libc::SYS_kill, libc::getpid(), 0),
            "tgkill_self" => libc::syscall(
                libc::SYS_tgkill,
                libc::getpid(),
                libc::syscall(libc::SYS_gettid),
                0,
            ),
            "tgkill_parent" => libc::syscall(libc::SYS_tgkill, libc::getppid(), libc::getppid(), 0),
            "pidfd_send_signal" => libc::syscall(libc::SYS_pidfd_send_signal, 0, 0, 0, 0),
            "clone_newns" => libc::syscall(libc::SYS_clone, libc::CLONE_NEWNS, 0, 0, 0, 0),
            "clone_newuser" => libc::syscall(libc::SYS_clone, libc::CLONE_NEWUSER, 0, 0, 0, 0),
            "shmget" => libc::syscall(libc::SYS_shmget, 0, 4096, 0),
            "personality_query" => libc::syscall(libc::SYS_personality, u32::MAX),
            "personality_set" => libc::syscall(libc::SYS_personality, 0x0004_0000),
            #[cfg(target_arch = "x86_64")]
            "x32_execve" => libc::syscall(
                libc::SYS_execve | X32_SYSCALL_BIT as libc::c_long,
                std::ptr::null::<libc::c_char>(),
                std::ptr::null::<libc::c_char>(),
                std::ptr::null::<libc::c_char>(),
            ),
            #[cfg(not(target_arch = "x86_64"))]
            "x32_execve" => {
                // No x32 table here: the guard does not exist, so report the refusal it stands for.
                *libc::__errno_location() = libc::EPERM;
                -1
            }
            _ => return Err(io::Error::from(io::ErrorKind::InvalidInput)),
        }
    };
    if result >= 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A process with a second thread is refused before any irreversible step is taken, so this
    /// test process stays unconfined.
    #[test]
    fn a_second_thread_makes_the_confinement_fail() {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let helper = std::thread::spawn(move || {
            let _ = wait.recv();
        });
        let error = ensure_single_thread().expect_err("two threads");
        assert!(error.to_string().contains("more than one thread"));
        release.send(()).expect("release");
        helper.join().expect("join");
    }

    #[test]
    fn dropping_capabilities_is_a_no_op_for_an_unprivileged_process() {
        // SAFETY: `geteuid` takes no arguments and cannot fail.
        if unsafe { libc::geteuid() } != 0 {
            drop_capabilities().expect("nothing to drop");
        }
    }

    #[test]
    fn the_filters_compile_for_every_profile() {
        let arch = target_arch().expect("a supported test architecture");
        for profile in [
            SandboxProfile::PdfExtract,
            SandboxProfile::PdfRender,
            SandboxProfile::Fetch,
        ] {
            assert!(
                !main_filter(profile, arch, 1)
                    .expect("main filter")
                    .is_empty()
            );
        }
        assert!(!clone3_filter(arch).expect("clone3 filter").is_empty());
    }
}
