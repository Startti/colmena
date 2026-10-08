# Python sandbox: run mounts (review note for the sandbox owner)

Status: built, local only, not delivered. Dark behind `COLMENA_LARGE_TABULAR`. The engine's own calls do not use the
new paths yet (the protocol, prelude and routing work come later), so no existing call changes; BUT the host
(`Dispatcher::build`) and `python_executor serve` now build their subprocess executor with `new_for_serving`, which, when
a staging root is configured (switch on AND the directory set), sweeps that root before serving: it unmounts and removes
call directories left by a killed predecessor. With the switch off nothing is swept. This note is for the person
who signs off the jail change; the developer-facing description is in
[53_python_executors.md](../developer_guide/53_python_executors.md#run-staging-directories-linux-dark).

## Open question first: seccomp

Today the read-only guarantee of `/data` rests on ONE thing: the program holds an empty capability set (all of
inheritable, permitted, effective and ambient are zero), so every mount operation fails with `EPERM`. The seccomp
filter denies `mount`, `umount2`, `unshare`, `setns`, `pivot_root`, `chroot` but NOT the new mount API system calls
(`fsopen`, `fsconfig`, `fsmount`, `fspick`, `open_tree`, `move_mount`, `mount_setattr`); the capability bounding set is
not cleared either. Both are as they were before this change. Two blind reviewers independently asked for the seven
calls to be added to the denylist as defence in depth. That edit touches `seccomp.rs`, which this unit was told not to
change, so it was NOT made and no claim is made that it would not alter anything else. Tests prove today's behaviour
only: four of the seven (`fsopen`, `open_tree`, `mount_setattr`, `move_mount`) are exercised and fail with `EPERM` from
inside; the other three (`fsconfig`, `fsmount`, `fspick`) exist and are NOT exercised (`ENOSYS` would be accepted only for a kernel without the API, and only because the capability sets are asserted
empty in the same test). Decision for the sandbox owner: add the seven calls to the denylist (and its test), clear the
bounding set, or accept the capability drop as the only barrier.

## What changes, in one paragraph

A call that carries prepared data gets two directories made by the trusted side: `data` (the prepared Parquet parts) and
`out` (a tmpfs of its own, size and inode bound). The jail binds them at `/data` (read-only, `nosuid,nodev,noexec`) and
`/out` (writable, `nosuid,nodev,noexec`). The call header carries only a call id and a size bound, never a path.
Everything else the jail does is unchanged, except that a configured staging root is covered for every call.

## Control table

"Off" = `COLMENA_LARGE_TABULAR` off or no staging directory (today's jail). "Plain" = a call without mounts on an
executor with a staging root. "Mounts" = a call that asks for them.

| Control | Before | Off | Plain | Mounts |
|---|---|---|---|---|
| Namespaces (net, mnt, ipc, uts) | all four, empty network | same | same | same |
| Network | empty namespace, `socket()` refused by seccomp | same | same | same |
| Seccomp filter | `seccomp.rs` | file untouched | same | same |
| Identity, capabilities | slot uid/gid, no groups, empty effective/permitted | same | same | same (compared from inside) |
| `no_new_privs`, death signal | set | same | same | same |
| `RLIMIT_AS` | template VmSize + `memory_mb` | same | same | same value |
| `RLIMIT_CPU`, `NOFILE` 256, `NPROC` 64, `CORE` 0 | as today | same | same | same |
| `RLIMIT_FSIZE` | `tmp_mb` (64 MiB) | same | same | `max(tmp_mb, verified volume size)`, from the volume the jail checked, not from the header |
| `/tmp` | private tmpfs 64 MiB, `nosuid,nodev` | same | same | same |
| `/proc`, hidden paths | fresh proc `hidepid=invisible`; `DEFAULT_HIDDEN` + configured | same | + staging root covered | + staging root covered |
| Mounts below the staging root in the call's namespace | n/a | n/a | all detached | all detached, this call's own volume at its staging path included (only the `/data` and `/out` binds survive) |
| `/data` | absent | absent | absent (an empty directory persists after the first mounted call, see Q1) | bind of the call's `data`, ro,nosuid,nodev,noexec, flags read back |
| `/out` | absent | absent | same as `/data` | bind of the call's own tmpfs, rw,nosuid,nodev,noexec |
| Output size | n/a | n/a | n/a | 1 to 1,024 MiB (`OUT_MB_MAX`), checked at creation and again in the jail, before any mount |
| Output volume checks in the jail | n/a | n/a | n/a | tmpfs, size <= declared, inodes <= 1,024, mount root, not the call directory's volume |
| Staged volumes in flight | n/a | n/a | n/a | at most 2 volumes and 2,048 MiB in total per executor, enforced ONLY through `SubprocessExecutor::stage_call`; `StagedCall::create` and `run_staged(CallMounts)` are public and unbudgeted (see Budget) |
| Environment | `PATH`, 3 thread variables, host locale | same | + `ARROW_DEFAULT_MEMORY_POOL=system`, `ARROW_IO_THREADS=1` | same as Plain |
| Call header | 4 fields | same bytes | same bytes | + `mounts {stage_id, out_mb}` |
| Jail spec | 3 fields | same bytes | + `staging_root` | same |
| Template start | single-thread check, 26-layer self-test | same | + a second probe whose failure disables mounts only | same |
| Self-test layers | 26 | 26 | 26 + 3 (the mount probe) | 26 + 3 |
| New executor setting | n/a | `COLMENA_PYTHON_EXECUTOR_STAGING_DIR` is read and ignored | `COLMENA_PYTHON_EXECUTOR_STAGING_DIR`, `--staging-root` | same |

## Budget numbers (S1)

| Number | Value | Where it comes from | Status |
|---|---|---|---|
| `OUT_MB_MAX` | 1,024 MiB | the design's output cap, which is at most the 1 GiB upload cap (design D6/D9) | ceiling; final value needs the memory measurement |
| `STAGED_VOLUMES_MAX` | 2 | design D11 `PYEXEC_SLOTS` = 2 (one heavy slot, `H` = 1) | estimate, not measured |
| `STAGED_OUT_MIB_MAX` | 2,048 MiB | design D11 `V = D_max + OUT_MAX` = 2,048 MiB | estimate, not measured |
| `OUT_MAX_INODES` | 1,024 | chosen so a program cannot fill memory with empty files; the design's cap is 8 files | not derived from a measurement |

All four are estimates. The tmpfs is memory and counts against the instance budget; spike item 5 (instance memory,
concurrency) has not been measured, so the final values of the budget and the ceiling still need it. A request over the
budget gets a typed `StageError::OverBudget` and mounts nothing. The budget is NOT enforced by visibility: the tests and any
caller can still call `StagedCall::create` or `run_staged` directly, so the unit that wires the protocol must use
`stage_call`. A share is not given back when the volume could not be unmounted (it still holds its memory).

## Evidence that the unchanged things are unchanged

- `git diff <base>..HEAD -- python_exec/seccomp.rs` is empty. `DEFAULT_HIDDEN`, the `set_limit` calls except
  `RLIMIT_FSIZE`, the uid/gid/capability code and the namespace flags are not in the diff.
- No line of any existing suite is changed (`python_executor_isolation`, `_subprocess`, `_golden`, `_serve*`,
  `_remote`, the C0 characterisation suites): they pass unmodified.
- A call header without mounts and a jail spec without a staging root serialise to exactly today's bytes (unit tests).
  With the switch off, `COLMENA_PYTHON_EXECUTOR_STAGING_DIR` is READ and then ignored (the value is not validated and
  nothing is created): the executor config is today's. The self-test of an executor without a staging root reports the
  26 layers.
- From inside the sandbox a call with mounts and a call without compare equal in: uid/gid/groups, all five capability
  sets (the bounding set is nonzero and unchanged: the jail has never cleared it), `NoNewPrivs`, seccomp mode,
  environment, every `RLIMIT_*` except `RLIMIT_FSIZE` (compared separately: 64 MiB against the volume's 100 MiB),
  `socket()` error, network interfaces, cwd and `/tmp` size.

## What the two security reviewers found, and what was done

Both found no sandbox escape, no write outside a call's own `/out`, no read of another call's or the host's data and no
change with the feature off, and judged the core construction sound (private propagation before the binds, bind from the
held descriptor after a no-follow walk, flags read back, empty capability set, ordering preserved, no staging
descriptor reaching the sandbox). They agreed on ten items. The fixes are stacked slices after the original chain
(not folded into the slice that introduced each piece: see the slice table in the report).

| # | Finding | Resolution | Proof |
|---|---|---|---|
| S1 | `out_mb` unvalidated: 0 mounts an unlimited tmpfs; `RLIMIT_FSIZE` from the header; no bound on concurrent volumes | 1 to 1,024 MiB checked at creation and in the jail before any mount; file limit from the verified volume; per-executor budget with a typed error | `an_invalid_output_size_makes_nothing`, `a_header_with_an_invalid_output_size_ends_the_call`, `the_file_limit_follows_the_verified_volume_not_the_header`, `the_executor_refuses_staged_calls_over_its_budget`, budget unit tests. The ordering "refused before any path is opened or anything mounted" is proven by `an_invalid_output_size_is_refused_before_any_path_is_opened` and `creation_refuses_an_invalid_size_before_touching_the_root` (against a root that does not exist: `InvalidInput`, not `NotFound`), not by the end-to-end header test, which `judge_out_volume` would also satisfy |
| S2 | every call's namespace copies every other live call's `/out` | the jail detaches EVERY mount below the staging root (this call's own volume at its staging path too; deepest first) before covering it; a mount whose path vanished meanwhile (ENOENT/EINVAL) is ignored: that branch IS reached (56 ENOENT hits in one full run of the suite, counted once with a temporary probe), though no test asserts it on purpose | `a_call_does_not_see_other_calls_mounts`, `calls_start_correctly_while_other_calls_come_and_go`. The claim that pages stay pinned could NOT be reproduced here (see below) |
| S3 | `check_out_volume` checked only size and device | tmpfs type, size, inode bound, mount root, not the call directory's volume; self-test probes the inode limit | `the_output_volume_is_judged_fact_by_fact`, `real_volumes_that_miss_one_fact_are_refused`, self-test unit tests |
| S4 | `staging_root_hidden` passed when the listing failed | the layer proves the cover (empty read-only tmpfs, listed successfully); a listing failure fails it | `the_staging_probe_proves_the_cover_and_does_not_read_a_failure_as_one`, and the self-test fails when the jail does not cover the root |
| S5 | tests that did not prove what they said | swap test now runs the real `open_staged` and `bind_dir` and swaps the path between them; mount API helper clears errno and treats `rc >= 0` as success; same-sandbox test order independent and compares a 100 MiB file limit | `a_path_swapped_between_the_walk_and_the_bind_binds_the_original`, `data_cannot_be_remounted_unmounted_or_replaced`, `a_call_with_mounts_is_otherwise_the_same_sandbox` |
| S6 | a broken staging setup or pyarrow import could take the whole executor down | the mount probe is separate: a failure disables the mounts capability (logged, `MOUNTS_DISABLED <reason>` before `READY`, typed error); the pool check never imports pyarrow | `a_broken_staging_root_disables_mounts_and_not_plain_calls`, `pyarrow_is_never_imported_just_to_be_checked`, `a_template_with_mounts_disabled_refuses_only_mounts_calls` |
| S7 | cleanup errors swallowed; nothing reclaims leftovers after a crash | failures logged with the call id; a startup sweep unmounts and removes leftover call directories without following anything inside | `a_failed_cleanup_is_logged_with_the_call_id`, `the_sweep_reclaims_leftover_calls_and_follows_nothing`, `an_executor_about_to_serve_sweeps_its_root` |
| S8 | the pyarrow test never runs in CI | a missing pyarrow prints a note; with `COLMENA_PYEXEC_EXPECT_PYARROW=1` it prints the skip line the job's step greps for and fails. NOT added to the workflow | see "CI does not install pyarrow" below |
| S9 | add the new mount API calls to seccomp | NOT done (outside the brief); first open question | n/a |
| S10 | the note overclaimed and omitted | this rewrite | n/a |

Honest note on S2. The reviewers' claim was that the memory of a released output volume stays pinned while any call that
started earlier is alive. In the Docker container used here it was not: after the host unmounted, a running overlapping
call lost the mount from its own namespace and the memory was freed, with or without the detach
(`a_released_volume_is_freed_while_an_overlapping_call_runs` passes both ways, so it guards the property but does not
prove the fix). What does fail without the detach is the visibility test: before the fix a call's `/proc/self/mountinfo`
named other calls' volumes. Whether the kernel on Cloud Run propagates the unmount the same way is not known.

## Threat cases and the test that covers each, with its honest mutation status

All tests are in `tests/python_executor_mounts.rs` unless noted. "Mutated" = the control was removed or weakened in the
Docker container and a named test failed; "not mutated" = no mutation was run.

| Threat | Control | Test | Status |
|---|---|---|---|
| Program writes to `/data` | read-only mount, not file mode | `nothing_can_be_changed_under_data` (world-writable file and directory; 11 operations all `EROFS`; source unchanged) | mutated: drop `MS_RDONLY`, also with the read-back neutralised |
| `/data` suid/dev/exec | mount flags | `data_is_mounted_nosuid_nodev_noexec_and_read_only` | mutated through the read-back (the call ends); not shown independently of the read-back |
| Remount, unmount, mount over, new namespaces, new mount API, chroot | empty capability set; seccomp for the old calls | `data_cannot_be_remounted_unmounted_or_replaced`, `out_cannot_be_remounted_or_unmounted` | not mutated: this is the existing capability drop. The helper was fixed after review; its reporting of a success was shown on a call that succeeds as root |
| Reach another call's data or the host's | bind of the opened directory only; root covered; other calls' mounts detached | `nothing_but_this_calls_data_is_reachable`, `the_staging_root_is_hidden_from_a_call_without_mounts`, `a_call_does_not_see_other_calls_mounts` | mutated: widen the bind, do not cover the root, do not detach |
| Symlink or `..` in the path | id charset; `O_NOFOLLOW` per component; owner/mode check | `staging::tests` (macOS and Linux), `a_call_the_jail_cannot_trust_ends_before_any_code_runs` | mutated: allow links, skip the owner check, skip the id check |
| Swap between check and bind | the bound thing is the descriptor | `a_path_swapped_between_the_walk_and_the_bind_binds_the_original` (real `open_staged` and `bind_dir`, in a child) | mutated: bind re-resolves by name |
| Output unbounded | size, inode bound, tmpfs, mount root, own volume, 1 to 1,024 MiB | `out_is_bounded_and_mounted_nosuid_nodev_noexec`, `the_output_volume_is_judged_fact_by_fact`, `real_volumes_that_miss_one_fact_are_refused`, `an_output_that_is_not_a_bound_volume_ends_the_call`, `an_invalid_output_size_makes_nothing` | mutated: no size, no inode bound, any type, no mount-root check, no volume check, size 0 accepted, ceiling removed |
| Output shared between calls | own tmpfs per call | `out_is_writable_and_private_to_each_call` | not mutated |
| Output cleaned afterwards | unmount, then remove; failures logged | `dropping_the_call_unmounts_and_removes_everything`, `a_failed_cleanup_is_logged_with_the_call_id` | mutated: skip cleanup, never unmount, swallow the error |
| Trusted side follows a link the program left | cleanup unmounts first | `a_link_left_in_out_is_never_followed_by_the_cleanup` | mutated |
| Leftovers after a crash | startup sweep, no-follow | `the_sweep_reclaims_leftover_calls_and_follows_nothing`, `an_executor_about_to_serve_sweeps_its_root`; `out_is_still_mounted` unit test | mutated: no unmount, follow a link inside, no sweep at start, still-mounted judgement. An `out` that cannot be opened for any reason but ENOENT is left in place (`the_sweep_leaves_a_call_whose_out_it_cannot_open`, `only_a_missing_out_means_nothing_to_unmount`). The "volume that cannot be detached is left in place" branch is covered by the unit test of its judgement only: a mount that `MNT_DETACH` refuses could not be built |
| Larger file limit leaks to other calls | only for mounts calls, from the verified volume | `only_the_file_size_limit_follows_the_output_volume`, `the_file_limit_follows_the_verified_volume_not_the_header` | mutated |
| Feature visible with the switch off | gate on switch and staging dir | `config::staging_tests` (macOS and Linux), byte tests in `child.rs`, `jail.rs` | mutated: ignore the switch, serialise always |
| Mounts asked for without a staging root | refused before a child starts | `mounts_without_a_staging_root_are_refused` | mutated |
| Broken staging takes the executor down | the mounts capability is disabled alone | `a_broken_staging_root_disables_mounts_and_not_plain_calls` | mutated: not disabled; made fatal |
| Self-test passes with a broken mount | the mount probe, three layers | `the_self_test_reports_the_mount_layers_only_with_a_staging_root`, unit tests of each probe on good and bad mounts | mutated: probes always holding, read-write mount, no inode probe, vacuous cover. `the_self_test_ignores_mounts_of_calls_in_flight` caught its mutation in about half of the runs |
| Default pyarrow pool reserves about 1 GiB | system pool env; the template check | `a_staged_executor_adds_exactly_the_arrow_variables`, `zygote::tests::a_loaded_pyarrow_must_use_the_system_pool`, `pyarrow_is_never_imported_just_to_be_checked`, `pandas_reads_a_parquet_part_through_data_when_pyarrow_is_installed` | mutated: drop the env, accept any pool, import pyarrow |

## Verification, and where each part ran

| Where | What | Result |
|---|---|---|
| macOS (aarch64) | `cargo fmt --check`, `cargo clippy --lib --tests -D warnings`, `cargo test --lib`, the C0 suites, `scripts/check_doc_links.py`. Portable unit tests: the no-follow walk, the budget, the volume judgement, the mount-table parsing, the config gate, the header byte test | green |
| macOS, compile only | NOTHING of the jail compiles on macOS: `jail`, `selftest`, `subprocess`, `zygote` are `cfg(target_os = "linux")` | not verified there |
| Docker Desktop, `linux/aarch64`, Debian bookworm + the CI package set, `--cap-add SYS_ADMIN --security-opt seccomp=unconfined --security-opt apparmor=unconfined`, root | Linux clippy; `cargo test --lib python_exec`; the new suite (38 tests); `python_executor_subprocess`, `_isolation`, `_serve`, `_serve_process`, `_remote`, `_golden` unchanged; `python_executor self-test` (26 layers, 29 with `--staging-root`); every mutation in this note; the suite also with pyarrow 21.0.0 installed in the image (the test that reads Parquet through `/data` ran there) | green |
| The Linux job "Python executors (Linux, isolated)" | the new suite runs there only after the one-line workflow change (its own commit) merges; the job itself was never run | NOT run |
| The real dev executor on amd64, Cloud Run gen2 | nothing | NOT verified |

Caveats about the Docker evidence: Docker Desktop's VM kernel, not Cloud Run's; aarch64, not amd64; the container runs
as root with seccomp unconfined for the container itself (the jail applies its own filter inside).

## CI change, stated exactly

`.github/workflows/ci-develop.yml`, step "Isolation and subprocess suites": `--test python_executor_mounts` appended to
the existing `cargo test` line (two changed lines). Strictly needed because that step lists its suites by name and the new
suite is a jail test (root, `CAP_SYS_ADMIN`) that only this job can run. It sits alone in its own commit.

Decision for the sandbox owner: the job installs `python3-pandas` and NOT pyarrow, so CI does not exercise a template
that has pyarrow loaded, nor the test that reads a Parquet part through `/data`. Adding `pip install pyarrow` (or a
package) is a workflow change and was not made. Until it is, those two things are covered only by the Docker run.

## Requirements for the next unit, which reads `/out` as root

The trusted side will read what the program wrote. Everything in `/out` is attacker-controlled. The next unit MUST:

1. wait until the child is reaped (the program can still be writing before that) and read BEFORE the volume is unmounted;
2. walk from one directory descriptor with `openat(O_NOFOLLOW | O_NONBLOCK | O_NOCTTY | O_CLOEXEC)`, never by path;
3. `fstat` every entry and accept only regular files with a single link; never open FIFOs, sockets, devices or
   directories it did not expect; refuse symlinks and hard links;
4. cap the number of entries, the depth, the name length and the LOGICAL size (files can be sparse: the allocated size
   says nothing about how much a read returns);
5. treat names as raw bytes, not as UTF-8 text, and validate them against the allowed charset before use;
6. reserve the output budget (`StagingBudget`) before creating the volume, not after.

Not done in this unit: nothing here reads `/out` on the trusted side.

## Deployment requirements

- A staging root BELOW a path the jail covers (`/mnt`, `/srv`, `/run`, `/home`, `/root`, `/var/tmp`, `/app`, `/dev/shm`, a configured hidden path or `/tmp`: a Cloud Run volume is normally under `/mnt`) is supported: inside the jail it does not exist, and that counts as hidden only when the nearest covered ancestor is proven to be the cover (`a_staging_root_below_a_covered_path_enables_mounts` runs a self-test and a real mounts call with roots under `/mnt` and `/tmp`). Before this was fixed such a root disabled mounts with a misleading `mount_layer_failed`.
- The staging root must be the staging volume's OWN mount point (a tmpfs or a volume mounted for this executor), so that
  its contents cannot be shared with anything else and the cover the jail puts over it hides only staged data.
- Its owner must be the executor's user (root) and its mode `0700` (no group or other access). The call directories the
  trusted side creates are `0700` (call) and `0755` (`data`, readable by the slot user through the bind); the jail refuses
  a call directory or `data` directory that is not owned by the executor or is writable by group or others.
- One executor per staging root: the startup sweep cannot tell a leftover from a call in flight. This is documented, NOT enforced (see the open questions).
- If the volume is persistent, the prepared customer data of a call that was killed stays on it until the next start of
  an executor with that root, which sweeps it; a tmpfs volume vanishes with the instance. Mounted output volumes do not
  survive a restart of the container (their namespace is gone).
- The root filesystem must allow `mkdir /data` and `mkdir /out`, or the image must contain them (Q1); otherwise the
  mounts capability is disabled at start (`staging_unusable`) and plain calls are unaffected.

## Not run / not verified

Linux CI job; amd64; Cloud Run gen2 or the real dev executor (kernel behaviour of mount propagation, rootfs writability,
the new mount API under that kernel, allocator behaviour with pyarrow on amd64); the real jail with pyarrow loaded for
`RLIMIT_NPROC`/`NOFILE` and the 64 MiB `/tmp`; instance memory with several volumes (spike item 5); real anonymised files.
Not built here (later slices): filling `data` and the `/v2/run` protocol (C6), the prelude and routing (C7), collecting
`out` on the trusted side (C8), the executor image and staging volume (A4).

## Other open questions for the sandbox owner

- CI installs no pyarrow (S8): a template with pyarrow loaded and a Parquet read through `/data` never run in CI, only in the Docker run. Install it in the job (a workflow change, not made) or accept that?
- One executor per staging root is documented but NOT enforced: a second root process started with the same staging directory would sweep the first one's in-flight calls (and unmount their volumes) at its start. A lock file or a per-process subdirectory would enforce it; not done.

1. Mount points. `/data` and `/out` are created by the jail in the root filesystem when absent (a link or file there is
   refused; calls racing to create them are handled). They then persist, empty and not writable by the program, and later
   small calls see them. Alternative: pre-create them in the executor image and fail if absent. Which?
2. A root process now mounts and unmounts a tmpfs per heavy call in the host mount namespace (with `MNT_DETACH` as a
   fallback). Acceptable, or should volumes come from a pre-sized pool?
3. The output volume is memory (tmpfs) and counts against the instance budget (see Budget). Spike item 5 is pending.
4. `RLIMIT_FSIZE` becomes the verified volume size for mounts calls, so one file may fill the volume. Intended; say if
   you want a per-file cap below the volume.
5. The staging root is covered for every call of an executor that has one, including calls without mounts, and the Arrow
   variables are added to the environment of every call of that executor (the template is shared).
6. Real-jail measurements (spike items 1 and 2 on amd64) are still to be done; only the aarch64 Docker approximation
   exists.
