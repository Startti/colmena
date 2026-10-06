# Python sandbox: run mounts (review note for the sandbox owner)

Status: built, local only, not delivered. Dark behind `COLMENA_LARGE_TABULAR`. Nothing in the engine calls the new
paths yet (the protocol, prelude and routing work come later), so with the switch off, and with it on, no existing
call changes. This note is for the person who signs off the jail change; the developer-facing description is in
[53_python_executors.md](../developer_guide/53_python_executors.md#run-staging-directories-linux-dark).

## What changes, in one paragraph

A call that carries prepared data gets two directories made by the trusted side: `data` (the prepared Parquet parts) and
`out` (a tmpfs of its own, size and inode bound). The jail binds them at `/data` (read-only, `nosuid,nodev,noexec`) and
`/out` (writable, `nosuid,nodev,noexec`). The call header carries only a call id and a size bound, never a path.
Everything else the jail does is unchanged.

## Control table

"Off" = `COLMENA_LARGE_TABULAR` off or no staging directory (today's jail). "Plain" = a call without mounts on an
executor with a staging root. "Mounts" = a call that asks for them.

| Control | Before | Off | Plain | Mounts |
|---|---|---|---|---|
| Namespaces (net, mnt, ipc, uts) | all four, empty network | same | same | same |
| Network | empty namespace, `socket()` refused by seccomp | same | same | same |
| Seccomp filter | `seccomp.rs` | file untouched | same | same |
| Identity | slot uid/gid, no groups, no capability | same | same | same (compared from inside) |
| `no_new_privs`, death signal | set | same | same | same |
| `RLIMIT_AS` | template VmSize + `memory_mb` | same | same | same value |
| `RLIMIT_CPU`, `NOFILE` 256, `NPROC` 64, `CORE` 0 | as today | same | same | same |
| `RLIMIT_FSIZE` | `tmp_mb` (64 MiB) | same | same | `max(tmp_mb, out_mb)` |
| `/tmp` | private tmpfs 64 MiB, `nosuid,nodev` | same | same | same |
| `/proc`, hidden paths | fresh proc `hidepid=invisible`; `DEFAULT_HIDDEN` + configured | same | + staging root covered | + staging root covered |
| `/data` | absent | absent | absent (an empty dir persists after the first mounted call, see Q1) | bind of the call's `data`, ro,nosuid,nodev,noexec, flags read back |
| `/out` | absent | absent | same as `/data` | bind of the call's `out` tmpfs, rw,nosuid,nodev,noexec |
| Environment | `PATH`, 3 thread variables, host locale | same | + `ARROW_DEFAULT_MEMORY_POOL=system`, `ARROW_IO_THREADS=1` | same as Plain |
| Call header | 4 fields | same bytes | same bytes | + `mounts {stage_id, out_mb}` |
| Jail spec | 3 fields | same bytes | + `staging_root` | same |
| Template start | single thread check | same | + pyarrow pool must be `system` if loaded | same |
| Self-test layers | 26 | 26 | 29 | 29 |
| New executor setting | - | - | `COLMENA_PYTHON_EXECUTOR_STAGING_DIR` (read only while the switch is on), `--staging-root` | same |

## Evidence that the unchanged things are unchanged

- `git diff <base>..HEAD -- python_exec/seccomp.rs`: empty. `DEFAULT_HIDDEN`, the `set_limit` calls except
  `RLIMIT_FSIZE`, uid/gid/capability code, namespace flags: not in the diff (`jail.rs` removes three lines in all: the
  `mount` visibility, the end of the hidden-path chain, the `RLIMIT_FSIZE` value).
- No line of any existing suite is changed (`python_executor_isolation`, `_subprocess`, `_golden`, `_serve*`, `_remote`,
  C0 characterisation): they pass unmodified. The one existing unit test of the template environment is untouched.
- Byte tests: a header without mounts and a jail spec without a staging root serialise to exactly today's bytes; the
  `COLMENA_PYTHON_EXECUTOR_STAGING_DIR` variable is not even looked at while the switch is off; the self-test of an
  executor without a staging root reports the 26 layers.
- From inside the sandbox, a call with mounts and a call without compare equal in: uid/gid/groups, all five capability
  sets except the bounding set (which the jail has never touched, see Q3), `NoNewPrivs`, seccomp mode, environment,
  every `RLIMIT_*`, `socket()` error, network interfaces, cwd and `/tmp` size.

## Threat cases and the test that covers each

All tests are in `tests/python_executor_mounts.rs` unless noted. "Docker" = privileged Debian bookworm container (see
Verification). Every one of these ran green in Docker and was shown to fail when its control was removed.

| Threat | Control | Test | Mutation that makes it fail |
|---|---|---|---|
| Program writes to `/data` | read-only mount, not file mode | `nothing_can_be_changed_under_data` (world-writable file and dir: write, append, create, mkdir, remove, rename, symlink, link, chmod, utime, truncate all `EROFS`; source unchanged afterwards) | drop `MS_RDONLY` |
| `/data` suid/dev/exec | mount flags | `data_is_mounted_nosuid_nodev_noexec_and_read_only` (`statvfs` from inside) | drop the flags |
| Remount, unmount, mount over, move | no capability, seccomp | `data_cannot_be_remounted_unmounted_or_replaced`, `out_cannot_be_remounted_or_unmounted` (`mount`, `umount2`, tmpfs over, `unshare` mount/user ns, `chroot`, `fsopen`, `open_tree`, `mount_setattr`, `move_mount`: `EPERM`; capability sets empty) | none directly: this is the existing capability drop |
| Reach anything but this call's data | bind of the opened directory only; staging root covered | `nothing_but_this_calls_data_is_reachable` (walk of `/`, `/data/..` is the jail root, another call's file nowhere), `the_staging_root_is_hidden_from_a_call_without_mounts` | widen the bind to the call directory; do not cover the root |
| Symlink or `..` in the path | call id charset; `O_NOFOLLOW` on every component; owner/mode check | `staging::tests` (macOS and Linux), `a_call_the_jail_cannot_trust_ends_before_any_code_runs` (`../etc`, a link as `data`, a world-writable `data`: the call ends before any code runs) | allow links; skip the owner check; skip the id check |
| TOCTOU between check and bind | the bound thing is the descriptor, not the path | `staging::tests::the_descriptors_keep_the_checked_directories_after_a_swap` | (property of the design) |
| Output unbounded | tmpfs `size=`, `nr_inodes=`, jail refuses any other volume | `out_is_bounded_and_mounted_nosuid_nodev_noexec` (3 MiB into 2: `ENOSPC`; 2000 files: `ENOSPC`; mknod `EPERM`; hard link from `/data` `EXDEV`), `an_output_that_is_not_a_bound_volume_ends_the_call`, `the_output_is_a_bound_tmpfs_of_its_own_and_data_is_not_a_mount`, `an_output_on_the_volume_of_the_call_directory_is_refused` | no size; no inode bound; skip the volume check |
| Output shared between calls | own tmpfs per call | `out_is_writable_and_private_to_each_call` | - |
| Output cleaned afterwards | unmount, then remove | `dropping_the_call_unmounts_and_removes_everything`, `release_twice_is_fine_...` | skip cleanup; never unmount |
| Trusted side follows a link the program left | cleanup unmounts first, never walks `out` | `a_link_left_in_out_is_never_followed_by_the_cleanup` | cleanup that resolves and removes entries |
| Larger file limit leaks to other calls | only for mounts calls; `/tmp` stays 64 MiB | `only_the_file_size_limit_follows_the_output_volume` | raise it for all calls; ignore `out_mb` |
| Feature visible with the switch off | gate on switch and staging dir | `config::staging_tests` (macOS + Linux), byte tests in `child.rs`, `jail.rs` | ignore the switch |
| Mounts asked for without a staging root | refused before a child starts | `mounts_without_a_staging_root_are_refused` | remove the check |
| Self-test would pass with a broken mount | three new layers proven in the probe call | `the_self_test_reports_the_mount_layers_only_with_a_staging_root`, unit tests of each probe on good and bad mounts in `selftest.rs` | read-write mount; probes that always hold |
| Default pyarrow pool reserves ~1 GiB per call | system pool env + template check | `a_staged_executor_adds_exactly_the_arrow_variables`, `zygote::tests::a_loaded_pyarrow_must_use_the_system_pool`, `pandas_reads_a_parquet_part_through_data_when_pyarrow_is_installed` | drop the env; accept any pool |

## Verification, and where each part ran

| Where | What | Result |
|---|---|---|
| macOS (aarch64, the author's machine) | `cargo fmt --check`, `cargo clippy --lib --tests -D warnings`, `cargo test --lib` (3,638 passed), the C0 suites, `scripts/check_doc_links.py`. Portable unit tests: the no-follow walk (9), the config gate (2), the header byte test. | green |
| macOS, compile only | NOTHING of the jail compiles on macOS: `jail`, `selftest`, `subprocess`, `zygote` are `cfg(target_os = "linux")`. | not verified there |
| Docker Desktop, `linux/aarch64`, Debian bookworm + the CI package set, `--cap-add SYS_ADMIN --security-opt seccomp=unconfined --security-opt apparmor=unconfined`, root | Linux clippy; `cargo test --lib python_exec` (130); the new suite (26); `python_executor_subprocess`, `_isolation`, `_serve`, `_serve_process`, `_remote`, `_golden` unchanged; `python_executor self-test` (26 layers, and 29 with `--staging-root`); every mutation in this note; the suite with real pyarrow 21.0.0 installed (pip, `--no-deps`) | green |
| The Linux job "Python executors (Linux, isolated)" | the new suite runs there only after the one-word workflow change (slice c5c) merges; the job itself was not run | NOT run |
| The real dev executor on amd64, Cloud Run gen2 | nothing | NOT verified: kernel, `/var/lib` and rootfs writability, seccomp interplay with the new mount API, amd64 allocator behaviour with pyarrow |

Caveats about the Docker evidence: it is the Docker Desktop VM kernel, not Cloud Run's; aarch64, not amd64; the container
runs as root with seccomp unconfined for the container itself (the jail applies its own filter inside).

One mutation is detected only probabilistically: the self-test's mount-leak check with and without the staging-root
exclusion (`the_self_test_ignores_mounts_of_calls_in_flight` caught it in about half of the runs; the churn is a thread
in the same process). One mutation pair is equivalent and was not distinguished: the output size check and the
`ENOSPC` probe in the self-test overlap on every volume that can be built here.

## CI change, stated exactly

`.github/workflows/ci-develop.yml`, step "Isolation and subprocess suites": `--test python_executor_mounts` appended to
the existing `cargo test` line. Strictly needed because that step lists its suites by name, and the new suite is a jail
test (root, `CAP_SYS_ADMIN`) that only this job can run. It sits alone in its own commit (c5c).

## Not done here (belongs to later slices)

Filling `data` with Parquet parts and the `/v2/run` protocol (C6); the prelude, routing and `tables` API (C7);
collecting and scanning `out` on the trusted side (symlinks, hard links, device nodes, name charset, file and byte caps:
C8); the executor image with pyarrow and the staging volume (A4); the re-run of spike items 1 and 2 in the real jail on
amd64.

## Open questions for the sandbox owner

1. Mount points. `/data` and `/out` are created by the jail in the root filesystem when absent (a link or file there is
   refused; callers racing to create them are handled). They then persist, empty and not writable by the program, and
   later small calls see them. Alternatives: pre-create them in the executor image and fail if absent (needs the A4
   Dockerfile), or mount over a directory that already exists. Which?
2. Information leak. A call can read the mount table, which names the other calls' output volumes (random ids; no
   content is reachable, the staging root is covered). Acceptable, or should the jail detach the other mounts under the
   root before covering it?
3. Hardening not done because it is outside the brief. The new mount API system calls (`fsopen`, `fsconfig`, `fsmount`,
   `fspick`, `open_tree`, `move_mount`, `mount_setattr`) are not in the seccomp denylist; today only the capability
   drop stops them (tested: `EPERM`). The capability bounding set is not cleared. Both are pre-existing; do you want
   them added to the denylist or the jail?
4. The trusted side (the executor process, root with `CAP_SYS_ADMIN`) now mounts and unmounts a tmpfs per heavy call in
   the host mount namespace. A call that fails to unmount falls back to `MNT_DETACH`. Is a root process doing per-call
   mounts acceptable, or should the volume be made another way (a pre-sized pool of volumes)?
5. The output volume is memory (tmpfs) and counts against the instance memory budget (`V = D_max + OUT_MAX` in the
   design). Spike item 5 has not measured it.
6. `RLIMIT_FSIZE` becomes the output size for mounts calls, so one file may fill the volume. Intended; say if you want a
   per-file cap below the volume.
7. The staging root is covered for every call of an executor that has one, including calls without mounts.
8. The Arrow variables are added to the environment of every call of a staged executor, not only mounts calls (the
   template is shared).
9. Real-jail measurements (spike items 1, 2 on amd64, `RLIMIT_NPROC/NOFILE` and the 64 MiB `/tmp` with pyarrow loaded)
   are still to be done; only the aarch64 Docker approximation exists.
