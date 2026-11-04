# Workbench local patch: process-wrap 10.0.1

Source: the published crates.io `process-wrap` 10.0.1 archive, with package SHA-256
`1f21b97672d2dc848e7b25701ab4535618b92f4861c13cc3f7f7bed52ad3c8da`, matching the
repository's original locked checksum. Published VCS metadata identifies upstream
commit `ca45003a831ac125e6673b8759430e5ef33cc1db` in
[watchexec/process-wrap](https://github.com/watchexec/process-wrap).

Copied only released `src/`, `tests/`, normalized `Cargo.toml`, `README.md`,
`COPYRIGHT`, `LICENSE-APACHE`, and `LICENSE-MIT`. No registry cache markers,
upstream lockfile, generated binaries, or build directories are included. The
global Cargo cache remains unchanged. The upstream package name/version and
Apache-2.0 OR MIT license remain intact; this is a local source patch, not a new
upstream release.

## Scope

- `src/windows.rs`: only `JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO` means the entire
  Job has exited. Other completion packets remain pending. Ignore only a real
  `WAIT_TIMEOUT` with no packet; propagate closed-port and other native errors.
  Initialize the completion output pointer to null as required for timeout
  inspection. Each call dequeues at most one message.
- `src/tokio/job_object.rs`: track parent exit status separately from Job-empty
  state. `wait` is complete only after both. Poll the completion port without
  blocking, with cancellable Tokio sleeps while waiting for cleanup; no detached
  `spawn_blocking` consumer retains a borrowed/closed port. Cancelling `wait` and
  calling it again preserves both facts. `try_wait` retains a consumed empty
  notification and reports `None` while descendants remain.
- `src/std/job_object.rs`: apply the same parent/Job separation, real error
  propagation, and retained empty notification to blocking and nonblocking waits.
- `Cargo.toml`: explicitly enable Tokio `time` and the Windows SystemServices
  constants used by the patch. No dependency version change or new package.

No generic process framework, non-Windows behavior change, public API change, or
unsafe application code was added. Existing platform FFI remains confined to this
licensed vendor dependency.

## Platform contract and focused validation

Microsoft defines
[ACTIVE_PROCESS_ZERO](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_associate_completion_port)
as the Job's active process count reaching zero. Its
[GetQueuedCompletionStatus contract](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-getqueuedcompletionstatus)
distinguishes a timeout from a closed port and failed packets. Notifications may
be lost; missing proof therefore remains pending or produces a caller timeout,
never a fabricated successful release.

The workbench's existing Runtime example target `job_cleanup_probe` creates a
native helper parent that exits while its inherited-Job descendant remains alive.
It verifies that Job wait does not return early, cancels that wait, terminates the
owned Job, consumes the empty notification via `try_wait`, then successfully
resumes `wait` and observes descendant stdout EOF. It invokes no Agent/model,
network, npm installation, or credentials. Run from the root workspace:

```powershell
cargo run --locked -p agent-runtime --example job_cleanup_probe
```

No independent vendor lock/build or full upstream test suite is required for this
focused regression. Root patch/exclusion/lock and third-party notices are owned
by the integration lead.

Verified on Windows on 2026-11-04: the probe exited 0 with all five assertions
reported true (parent exited, no early Job completion, cancelled wait resumed,
empty notification retained, descendant pipe EOF); both tracked helper PIDs were
absent afterwards. The std path also compiled using
`cargo check --locked -p agent-runtime --features process-wrap/std --lib`.
The existing Runtime/Catalog library checks passed (6 + 2 tests), and both
crates' final all-targets build passed against this patched dependency. No model
calls or real installations were repeated.
