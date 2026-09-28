//! Last jail layer: syscalls a data-processing child never needs return EPERM.
//! Threads stay allowed (`clone` with CLONE_THREAD); `clone3` returns ENOSYS so
//! libc falls back to `clone`, where the flag can be inspected.

use seccompiler::{
    apply_filter, BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition,
    SeccompFilter, SeccompRule, TargetArch,
};
use std::collections::BTreeMap;
use std::io;

const DENIED_COMMON: &[i64] = &[
    libc::SYS_socket,
    libc::SYS_socketpair,
    libc::SYS_connect,
    libc::SYS_bind,
    libc::SYS_listen,
    libc::SYS_accept,
    libc::SYS_accept4,
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
    libc::SYS_userfaultfd,
    libc::SYS_kexec_load,
    libc::SYS_init_module,
    libc::SYS_finit_module,
    libc::SYS_delete_module,
    libc::SYS_reboot,
    libc::SYS_swapon,
    libc::SYS_swapoff,
    libc::SYS_acct,
    libc::SYS_personality,
    libc::SYS_name_to_handle_at,
    libc::SYS_open_by_handle_at,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
];

#[cfg(target_arch = "x86_64")]
const DENIED_ARCH: &[i64] = &[
    libc::SYS_fork,
    libc::SYS_vfork,
    libc::SYS_iopl,
    libc::SYS_ioperm,
];
#[cfg(not(target_arch = "x86_64"))]
const DENIED_ARCH: &[i64] = &[];

/// Keeps the OS error of a filter the kernel refused, for the jail's report.
fn err(e: impl Into<seccompiler::Error>) -> io::Error {
    match e.into() {
        seccompiler::Error::Prctl(e) | seccompiler::Error::Seccomp(e) => e,
        other => io::Error::other(other.to_string()),
    }
}

fn deny_filter(arch: TargetArch) -> io::Result<BpfProgram> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = DENIED_COMMON
        .iter()
        .chain(DENIED_ARCH)
        .map(|&nr| (nr, vec![]))
        .collect();
    // `clone` without CLONE_THREAD creates a process: refused. Threads pass.
    let no_thread = SeccompRule::new(vec![SeccompCondition::new(
        0,
        SeccompCmpArgLen::Qword,
        SeccompCmpOp::MaskedEq(libc::CLONE_THREAD as u64),
        0,
    )
    .map_err(err)?])
    .map_err(err)?;
    rules.insert(libc::SYS_clone, vec![no_thread]);
    let refused = SeccompAction::Errno(libc::EPERM as u32);
    SeccompFilter::new(rules, SeccompAction::Allow, refused, arch)
        .map_err(err)?
        .try_into()
        .map_err(err)
}

fn clone3_filter(arch: TargetArch) -> io::Result<BpfProgram> {
    let rules: BTreeMap<i64, Vec<SeccompRule>> = [(libc::SYS_clone3, vec![])].into_iter().collect();
    let unknown = SeccompAction::Errno(libc::ENOSYS as u32);
    SeccompFilter::new(rules, SeccompAction::Allow, unknown, arch)
        .map_err(err)?
        .try_into()
        .map_err(err)
}

/// Installs both filters on the calling thread; threads it starts later
/// inherit them. Nothing here can be undone.
pub fn apply() -> io::Result<()> {
    let arch = TargetArch::try_from(std::env::consts::ARCH).map_err(err)?;
    apply_filter(&deny_filter(arch)?).map_err(err)?;
    apply_filter(&clone3_filter(arch)?).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `BPF_JMP | BPF_JEQ | BPF_K`: how a program compares the syscall number.
    const JEQ: u16 = 0x15;

    fn compares(program: &BpfProgram, nr: i64) -> bool {
        program
            .iter()
            .any(|i| i.code == JEQ && i64::from(i.k) == nr)
    }

    /// The numbers are those of the target this test is built for; for the
    /// other architecture the programs only prove that they build.
    #[test]
    fn both_programs_build_for_each_supported_architecture() {
        for arch in [TargetArch::x86_64, TargetArch::aarch64] {
            let deny = deny_filter(arch).unwrap();
            let mut listed = DENIED_COMMON.iter().chain(DENIED_ARCH);
            assert!(listed.all(|&nr| compares(&deny, nr)), "{arch:?}");
            assert!(compares(&deny, libc::SYS_clone), "{arch:?}");
            assert!(compares(&clone3_filter(arch).unwrap(), libc::SYS_clone3));
        }
    }
}
