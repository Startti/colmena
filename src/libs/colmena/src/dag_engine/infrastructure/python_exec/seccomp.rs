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
    libc::SYS_kexec_file_load,
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
/// by other means (`open_tree`, `move_mount`, `fsopen`, `fsconfig`, `fsmount`,
/// `fspick`, `mount_setattr`, `open_tree_attr` from Linux 6.15), and the two
/// calls that read the mount table of a namespace (`statmount`, `listmount`,
/// Linux 6.8). The libc crate names the first seven only: the rest are listed by
/// number. Every number is the same on x86_64 and aarch64 (both use the generic
/// table from 424 on), so there is nothing to skip on either. Denied for EVERY
/// jail, with the action and errno of `mount`; the one observable change is that a
/// kernel lacking one of them now answers EPERM instead of ENOSYS.
pub(crate) const MOUNT_API: &[i64] = &[
    libc::SYS_open_tree,
    libc::SYS_move_mount,
    libc::SYS_fsopen,
    libc::SYS_fsconfig,
    libc::SYS_fsmount,
    libc::SYS_fspick,
    libc::SYS_mount_setattr,
    457, // statmount
    458, // listmount
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

fn deny_filter(arch: TargetArch) -> io::Result<BpfProgram> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = DENIED_COMMON
        .iter()
        .chain(DENIED_ARCH)
        .chain(MOUNT_API)
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

/// x86_64 kernels also accept, under the native arch value, the x32 numbering
/// of the same calls (the number with bit 30 set) and the x32-only entries of the
/// native table, numbered 512 to 547 (among them an `execve` and an `execveat`).
/// The programs above compare exact native numbers, so this one refuses every
/// number of 512 or more under that arch value. The native table ends in the
/// 470s, so nothing the template or Python needs is refused.
/// EPERM, not ENOSYS: a kernel without x32 already answers ENOSYS, so EPERM
/// shows that the filter answered. Other arch values are left to the
/// programs above.
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
fn x32_filter() -> BpfProgram {
    use seccompiler::sock_filter;
    /// `AUDIT_ARCH_X86_64` of `linux/audit.h`.
    const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
    /// The first number of the x32 ABI; also covers the x32 bit (bit 30).
    const FIRST_X32_NUMBER: u32 = 512;
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
        insn(jump(libc::BPF_JGE), FIRST_X32_NUMBER, 0, 1),
        ret(libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
        ret(libc::SECCOMP_RET_ALLOW),
    ]
}

/// Installs every program on the calling thread; threads it starts later
/// inherit them. Nothing here can be undone. The denylist includes [`MOUNT_API`]
/// with the same action and errno (EPERM) as `mount`, so that the read-only
/// guarantee of `/data` does not rest on the empty capability set alone. The
/// filter matches numbers, never the kernel's table, so it loads on a kernel that
/// lacks a call and answers EPERM for it where the kernel alone would say ENOSYS.
pub fn apply() -> io::Result<()> {
    apply_filter(&deny_filter(ARCH)?).map_err(err)?;
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
            let deny = deny_filter(arch).unwrap();
            let mut listed = DENIED_COMMON.iter().chain(DENIED_ARCH);
            assert!(listed.all(|&nr| compares(&deny, nr)), "{arch:?}");
            assert!(compares(&deny, libc::SYS_clone), "{arch:?}");
            assert!(answers(&deny, libc::EPERM), "{arch:?}");
            let clone3 = clone3_filter(arch).unwrap();
            assert!(compares(&clone3, libc::SYS_clone3), "{arch:?}");
            assert!(answers(&clone3, libc::ENOSYS), "{arch:?}");
        }
    }

    /// The new mount API, by number: the same on x86_64 and aarch64 (they share the
    /// generic table from 424 on). `open_tree_attr` (467), `statmount` (457) and
    /// `listmount` (458) are not named by the libc crate.
    #[test]
    fn the_mount_api_numbers_are_the_documented_ones() {
        assert_eq!(
            MOUNT_API,
            [428, 429, 430, 431, 432, 433, 442, 457, 458, 467]
        );
    }

    /// Denied for EVERY jail, with the same action and errno as `mount`; and
    /// `kexec_file_load` next to `kexec_load`.
    #[test]
    fn the_mount_api_and_kexec_file_load_are_always_denied_like_mount() {
        for arch in [TargetArch::x86_64, TargetArch::aarch64] {
            let deny = deny_filter(arch).unwrap();
            for &nr in MOUNT_API {
                assert!(compares(&deny, nr), "{arch:?} {nr}");
            }
            assert!(compares(&deny, libc::SYS_kexec_file_load), "{arch:?}");
            assert!(compares(&deny, libc::SYS_mount) && answers(&deny, libc::EPERM));
        }
    }

    /// Run as root, with every capability, in a throwaway child: the calls get
    /// past the kernel (an answer other than EPERM: a descriptor, EFAULT, EINVAL,
    /// ENOSYS on a kernel without the call), then the filter is installed and each
    /// of them answers EPERM. The unfiltered answers of `open_tree`, `move_mount`,
    /// `fsconfig`, `mount_setattr`, `open_tree_attr` and the two info calls come
    /// from argument validation (the zero arguments are rejected), which runs
    /// before the permission check: for those the proof is "the filter answers
    /// EPERM where the kernel answered something else". `fsopen`, `fsmount` and
    /// `fspick` reach the capability check, so for them this also proves that it is
    /// the FILTER, not a missing capability, that refuses them.
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
        // Exit status: 0 held; 10 + index for a call refused before the filter;
        // 50 + index for a call the filter did not refuse.
        let code = match unsafe { libc::fork() } {
            0 => {
                unsafe { libc::alarm(30) };
                let code = std::panic::catch_unwind(|| {
                    for (i, &nr) in MOUNT_API.iter().enumerate() {
                        if answer(nr) == libc::EPERM {
                            return 10 + i as i32;
                        }
                    }
                    apply().unwrap();
                    for (i, &nr) in MOUNT_API.iter().enumerate() {
                        if answer(nr) != libc::EPERM {
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
        };
        assert_eq!(code, 0, "every call refused by the filter, none before it");
    }

    /// Runs the x32 program on a `seccomp_data`-like (arch, nr), as the kernel
    /// would: `ld`, `jeq`/`jge` with their offsets and the two returns.
    fn run_x32(program: &BpfProgram, arch: u32, nr: u32) -> u32 {
        let (mut acc, mut pc) = (0u32, 0usize);
        loop {
            let i = &program[pc];
            pc += 1;
            match i.code {
                0x20 => acc = if i.k == 4 { arch } else { nr }, // ld [4] arch, ld [0] nr
                0x15 => pc += usize::from(if acc == i.k { i.jt } else { i.jf }), // jeq
                0x35 => pc += usize::from(if acc >= i.k { i.jt } else { i.jf }), // jge
                0x06 => return i.k,                             // ret
                other => panic!("unexpected instruction {other:#x}"),
            }
        }
    }

    /// Under the x86_64 arch value every number from 512 up is refused with
    /// EPERM: the x32-only entries of the native table (512 to 547, among them an
    /// `execve` and an `execveat`) and the numbers with the x32 bit alike; every
    /// native number below 512 and every other arch value is allowed by it. No
    /// native x86_64 call the template or Python needs is numbered 512 or more
    /// (the native table ends in the 470s; futex_waitv 449, faccessat2 439, clone3
    /// 435, rseq 334 are among the highest in use).
    #[test]
    fn the_x32_guard_refuses_every_number_from_512_up_under_the_x86_64_arch() {
        const X86_64: u32 = 0xC000_003E;
        const AARCH64: u32 = 0xC000_00B7;
        let allow = libc::SECCOMP_RET_ALLOW;
        let eperm = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
        let program = x32_filter();
        for nr in [0, 1, 59, 334, 435, 439, 449, 467, 470, 511] {
            assert_eq!(run_x32(&program, X86_64, nr), allow, "native {nr}");
        }
        for nr in [512, 520, 545, 547, 548, 1000, 0x4000_0000, 0x4000_0000 + 59] {
            assert_eq!(run_x32(&program, X86_64, nr), eperm, "x32 table {nr}");
        }
        for nr in [0, 59, 512, 520, 0x4000_0000] {
            assert_eq!(run_x32(&program, AARCH64, nr), allow, "other arch {nr}");
        }
    }

    /// The x32 program, instruction by instruction: EPERM for a number of 512 or
    /// more under the x86_64 arch value, allow for anything else. The jump offsets
    /// are those of the six-instruction layout (arch load, arch test skipping three
    /// to the allow, number load, the number test skipping one to the allow, the
    /// two returns): only the constant of the number test changed.
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
            (0x35, 0, 1, 512),         // jge 512, else to allow
            (0x06, 0, 0, 0x0005_0001), // ret SECCOMP_RET_ERRNO | EPERM
            (0x06, 0, 0, 0x7FFF_0000), // ret SECCOMP_RET_ALLOW
        ];
        assert_eq!(listing, expected);
    }
}
