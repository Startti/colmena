//! Last jail layer: syscalls a data-processing child never needs return EPERM.
//! Threads stay allowed (`clone` with CLONE_THREAD); `clone3` returns ENOSYS so
//! libc falls back to `clone`, where the flag can be inspected.

use seccompiler::{
    apply_filter, BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition,
    SeccompFilter, SeccompRule, TargetArch,
};
use std::collections::BTreeMap;
use std::io;

/// The architecture the programs are built for: the one this is compiled for.
#[cfg(target_arch = "x86_64")]
const ARCH: TargetArch = TargetArch::x86_64;
#[cfg(target_arch = "aarch64")]
const ARCH: TargetArch = TargetArch::aarch64;

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

/// The new mount API, which can do what `mount`, `umount2` and `pivot_root` do
/// by other means: `open_tree`, `move_mount`, `fsopen`, `fsconfig`, `fsmount`,
/// `fspick`, `mount_setattr`, and `open_tree_attr` (Linux 6.15, which the libc
/// crate does not name). Every number is the same on x86_64 and aarch64 (both
/// use the generic table from 424 on), so there is nothing to skip on either.
/// Denied only for a jail that asks for the extra layer (see [`apply`]).
pub(crate) const MOUNT_API: &[i64] = &[
    libc::SYS_open_tree,
    libc::SYS_move_mount,
    libc::SYS_fsopen,
    libc::SYS_fsconfig,
    libc::SYS_fsmount,
    libc::SYS_fspick,
    libc::SYS_mount_setattr,
    467, // open_tree_attr
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

fn deny_filter(arch: TargetArch, mount_api: bool) -> io::Result<BpfProgram> {
    let extra: &[i64] = if mount_api { MOUNT_API } else { &[] };
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = DENIED_COMMON
        .iter()
        .chain(DENIED_ARCH)
        .chain(extra)
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

/// x86_64 kernels may also accept the x32 numbering of the same calls under
/// the same arch value: the number with bit 30 set. The programs above
/// compare exact numbers, so this one refuses every number with that bit.
/// EPERM, not ENOSYS: a kernel without x32 already answers ENOSYS, so EPERM
/// shows that the filter answered. Other arch values are left to the
/// programs above.
#[cfg(target_arch = "x86_64")]
fn x32_filter() -> BpfProgram {
    use seccompiler::sock_filter;
    /// `AUDIT_ARCH_X86_64` of `linux/audit.h`.
    const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
    const X32_SYSCALL_BIT: u32 = 0x4000_0000;
    let insn = |code: u32, k: u32, jt: u8, jf: u8| sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    };
    let load = |offset| insn(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, offset, 0, 0);
    let ret = |action| insn(libc::BPF_RET | libc::BPF_K, action, 0, 0);
    let jump = |op| libc::BPF_JMP | op | libc::BPF_K;
    vec![
        load(4), // seccomp_data.arch
        insn(jump(libc::BPF_JEQ), AUDIT_ARCH_X86_64, 0, 3),
        load(0), // seccomp_data.nr
        insn(jump(libc::BPF_JGE), X32_SYSCALL_BIT, 0, 1),
        ret(libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
        ret(libc::SECCOMP_RET_ALLOW),
    ]
}

/// Installs every program on the calling thread; threads it starts later
/// inherit them. Nothing here can be undone. `mount_api` adds [`MOUNT_API`] to
/// the denylist, with the same action and errno (EPERM) as `mount`: it is for
/// jails that stage run mounts, so that the read-only guarantee of `/data` does
/// not rest on the empty capability set alone. The filter matches numbers, never
/// the kernel's table, so it loads on a kernel that lacks a call and answers EPERM
/// for it where the kernel alone would answer ENOSYS.
pub fn apply(mount_api: bool) -> io::Result<()> {
    apply_filter(&deny_filter(ARCH, mount_api)?).map_err(err)?;
    apply_filter(&clone3_filter(ARCH)?).map_err(err)?;
    #[cfg(target_arch = "x86_64")]
    apply_filter(&x32_filter()).map_err(err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `BPF_JMP | BPF_JEQ | BPF_K`: how a program compares the syscall number.
    const JEQ: u16 = 0x15;
    /// `BPF_RET | BPF_K`: a return with a fixed action.
    const RET: u16 = 0x06;

    fn compares(program: &BpfProgram, nr: i64) -> bool {
        program
            .iter()
            .any(|i| i.code == JEQ && i64::from(i.k) == nr)
    }

    /// Whether the program returns `errno` somewhere and ends allowing the
    /// call, which is what a number it does not list reaches.
    fn answers(program: &BpfProgram, errno: i32) -> bool {
        let refused = libc::SECCOMP_RET_ERRNO | errno as u32;
        let returns = |i: &seccompiler::sock_filter, k| i.code == RET && i.k == k;
        program.iter().any(|i| returns(i, refused))
            && program
                .last()
                .is_some_and(|i| returns(i, libc::SECCOMP_RET_ALLOW))
    }

    /// The numbers are those of the target this test is built for; for the
    /// other architecture the programs only prove that they build.
    #[test]
    fn both_programs_build_for_each_supported_architecture() {
        for arch in [TargetArch::x86_64, TargetArch::aarch64] {
            let deny = deny_filter(arch, false).unwrap();
            let mut listed = DENIED_COMMON.iter().chain(DENIED_ARCH);
            assert!(listed.all(|&nr| compares(&deny, nr)), "{arch:?}");
            assert!(compares(&deny, libc::SYS_clone), "{arch:?}");
            assert!(answers(&deny, libc::EPERM), "{arch:?}");
            let clone3 = clone3_filter(arch).unwrap();
            assert!(compares(&clone3, libc::SYS_clone3), "{arch:?}");
            assert!(answers(&clone3, libc::ENOSYS), "{arch:?}");
        }
    }

    /// The seven calls of the new mount API and its newest sibling, by number:
    /// the same on x86_64 and aarch64 (they share the generic table from 424 on).
    /// `open_tree_attr` (467) is not named by the libc crate.
    #[test]
    fn the_mount_api_numbers_are_the_documented_ones() {
        assert_eq!(MOUNT_API, [428, 429, 430, 431, 432, 433, 442, 467]);
    }

    /// Listed only for a jail that asks for the extra layer; when listed, with
    /// the same action and errno as `mount` and `umount2`.
    #[test]
    fn the_mount_api_is_denied_only_when_asked_for_and_like_mount() {
        for arch in [TargetArch::x86_64, TargetArch::aarch64] {
            let without = deny_filter(arch, false).unwrap();
            assert!(
                MOUNT_API.iter().all(|&nr| !compares(&without, nr)),
                "{arch:?}"
            );
            let with = deny_filter(arch, true).unwrap();
            for &nr in MOUNT_API {
                assert!(compares(&with, nr), "{arch:?} {nr}");
            }
            assert!(compares(&with, libc::SYS_mount) && answers(&with, libc::EPERM));
        }
    }

    /// Run as root, with every capability, in a throwaway child: the calls get
    /// past the kernel (an answer other than EPERM: a descriptor, EFAULT, EINVAL,
    /// ENOSYS on a kernel without the call), then the filter is installed and each
    /// of them answers EPERM. That it is the FILTER, not a missing capability,
    /// that refuses them. A filter built without the extra layer leaves them alone.
    #[test]
    fn the_filter_refuses_each_mount_api_call_where_the_capability_is_present() {
        if std::env::var("COLMENA_PYEXEC_JAIL_TESTS").as_deref() != Ok("1") {
            return;
        }
        let answer = |nr: i64| -> i32 {
            let rc = unsafe { libc::syscall(nr, 0, 0, 0, 0, 0, 0) };
            let e = if rc < 0 {
                io::Error::last_os_error().raw_os_error().unwrap_or(-1)
            } else {
                0
            };
            if rc >= 3 {
                unsafe { libc::close(rc as libc::c_int) };
            }
            e
        };
        // Exit status: 0 held; otherwise the index of the call that did not
        // behave, plus 10 for "refused before the filter" and 50 for "not refused
        // by the filter".
        let child = |extra: bool| -> i32 {
            match unsafe { libc::fork() } {
                0 => {
                    unsafe { libc::alarm(30) };
                    let code = std::panic::catch_unwind(|| {
                        for (i, &nr) in MOUNT_API.iter().enumerate() {
                            if answer(nr) == libc::EPERM {
                                return 10 + i as i32;
                            }
                        }
                        apply(extra).unwrap();
                        for (i, &nr) in MOUNT_API.iter().enumerate() {
                            let refused = answer(nr) == libc::EPERM;
                            if refused != extra {
                                return 50 + i as i32;
                            }
                        }
                        0
                    });
                    unsafe { libc::_exit(code.unwrap_or(99)) }
                }
                pid => {
                    let mut status = 0;
                    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
                    if libc::WIFEXITED(status) {
                        libc::WEXITSTATUS(status)
                    } else {
                        98
                    }
                }
            }
        };
        assert_eq!(
            child(true),
            0,
            "with the extra layer: every call refused by the filter"
        );
        assert_eq!(child(false), 0, "without it: none refused (today's filter)");
    }

    /// The x86_64 x32 program, instruction by instruction: EPERM for a
    /// number with the x32 bit under the x86_64 arch value, allow for
    /// anything else.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn the_x32_program_refuses_x32_numbers_with_eperm() {
        let listing: Vec<_> = x32_filter()
            .iter()
            .map(|i| (i.code, i.jt, i.jf, i.k))
            .collect();
        let expected: [(u16, u8, u8, u32); 6] = [
            (0x20, 0, 0, 4),           // ld [4]: arch
            (0x15, 0, 3, 0xC000_003E), // jeq AUDIT_ARCH_X86_64, else to allow
            (0x20, 0, 0, 0),           // ld [0]: nr
            (0x35, 0, 1, 0x4000_0000), // jge x32 bit, else to allow
            (0x06, 0, 0, 0x0005_0001), // ret SECCOMP_RET_ERRNO | EPERM
            (0x06, 0, 0, 0x7FFF_0000), // ret SECCOMP_RET_ALLOW
        ];
        assert_eq!(listing, expected);
    }
}
