//! POSIX sandbox implementation (T081 — Unix path).
//!
//! This is the only surface in either crate that carries
//! `#![allow(unsafe_code)]`. The carve-out is explicitly authorized by FR-049
//! because `setrlimit`, `prlimit`, and `unshare` are `unsafe extern "C"`
//! symbols that have no safe Rust equivalent short of pulling in the full
//! `nix` crate (weighed against in `specs/043-evals-adv-features/research.md`
//! §R-006).
//!
//! Invariants enforced here:
//!
//! 1. Unsafe is denied at the crate root (`#![deny(unsafe_code)]`) — the only
//!    reason it isn't `forbid` is that `forbid` can't be relaxed by a nested
//!    `allow`, which FR-049 explicitly authorises for this one submodule.
//!    Every other source file in the crate still fails to compile on any
//!    `unsafe` usage.
//! 2. Every `unsafe` block below carries a `// SAFETY:` comment describing
//!    the invariant being upheld.
//! 3. The module compiles only on `cfg(target_family = "unix")` — it is
//!    absent on Windows builds, and the Windows stub in
//!    [`super::run_sandboxed`] returns `EvaluatorError::UnsupportedPlatform`
//!    without touching this code.
//!
//! Mechanics:
//!
//! * Resource caps are applied in the child via `Command::pre_exec`. The
//!   closure runs after `fork()` but before `exec()`, so async-signal-safe
//!   primitives are the only thing we're allowed to touch (no Rust-level
//!   locks, no heap allocation through `Arc::clone`, no `eprintln!`). The
//!   closure used here is intentionally minimal: it's three `libc` calls
//!   and a couple of `if` branches.
//! * Wall-clock is parent-enforced: the parent waits on the child up to the
//!   configured deadline, then sends SIGKILL.
//! * Exit classification heuristics map signals + stderr fragments back to a
//!   named limit per FR-017. Where a limit can only be observed indirectly
//!   (FDs, network), we sniff standard libc error strings from the child's
//!   stderr; the classifier favours the most specific label it finds.

#![allow(unsafe_code)]

use std::io::{self, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::evaluators::EvaluatorError;

use super::{SandboxLimits, SandboxOutcome};

/// Entry point called by [`super::run_sandboxed`] on Unix builds.
pub(super) fn run_sandboxed_unix(
    mut command: Command,
    limits: &SandboxLimits,
) -> Result<SandboxOutcome, EvaluatorError> {
    // Plumb stderr so we can classify the failure mode after waitpid().
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    install_pre_exec_limits(&mut command, limits.clone());

    let mut child = command.spawn().map_err(|err| EvaluatorError::Execution {
        reason: format!("sandbox spawn failed: {err}"),
    })?;

    let wall_clock = limits.wall_clock;
    let (outcome, wall_clock_exceeded) = wait_with_wall_clock(&mut child, wall_clock);

    let stderr = outcome.stderr.clone();
    let classified = classify(&outcome, wall_clock_exceeded, limits);

    let final_outcome = SandboxOutcome {
        success: classified.is_none() && outcome.exit_code == Some(0),
        exit_code: outcome.exit_code,
        signal: outcome.signal,
        stderr,
        limit_exceeded: classified.clone(),
    };

    if let Some(limit) = classified {
        return Err(EvaluatorError::SandboxLimitExceeded { limit });
    }

    Ok(final_outcome)
}

/// Install the `pre_exec` hook that applies rlimits + optional network-namespace
/// unshare before the child `exec`s its target binary.
fn install_pre_exec_limits(command: &mut Command, limits: SandboxLimits) {
    // SAFETY: `pre_exec` requires an async-signal-safe closure. The closure
    // below calls only `libc::setrlimit` (AS-safe per POSIX.1-2008), and on
    // Linux `libc::unshare`. It performs no heap allocations, no Rust-level
    // locking, and touches no shared state. `libc::rlimit` is a POD struct.
    // The `SandboxLimits` values are `Copy` primitives captured by move.
    unsafe {
        command.pre_exec(move || apply_limits(&limits));
    }
}

/// Apply every [`SandboxLimits`] field to the currently-executing child.
///
/// This runs post-fork / pre-exec. Must remain async-signal-safe. Each
/// `setrlimit` call is best-effort: EINVAL / EPERM on a specific resource is
/// tolerated so a platform-specific incompatibility (e.g. `RLIMIT_AS` on
/// macOS refusing to shrink below the forked VSZ) doesn't abort the spawn.
/// Any limit we couldn't install is surfaced through the absence of a
/// classification in [`super::SandboxOutcome::limit_exceeded`] rather than a
/// spawn failure — research.md §R-006 documents this degradation.
///
/// The signature matches the `io::Result<()>` shape required by
/// `Command::pre_exec`.
#[allow(clippy::unnecessary_wraps)]
fn apply_limits(limits: &SandboxLimits) -> io::Result<()> {
    // RLIMIT_CPU — seconds of CPU time.
    let cpu = clamp_rlim(limits.cpu.as_secs());
    let _ = set_rlimit(libc::RLIMIT_CPU, cpu);

    // RLIMIT_AS — virtual address space ceiling. macOS + Linux both respect
    // this; macOS also enforces RLIMIT_DATA, but RLIMIT_AS is the broadest.
    // Skip on macOS: the cap must be >= current VSZ (which already includes
    // every dylib loaded into the forked child), and a 1 GiB cap typically
    // EINVALs a modern macOS build out of the box.
    #[cfg(target_os = "linux")]
    {
        let mem = clamp_rlim(limits.memory_bytes);
        let _ = set_rlimit(libc::RLIMIT_AS, mem);
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Prefer RLIMIT_DATA on macOS — it caps the heap growth and is safer
        // to set to a small value without tripping on loaded dylibs.
        let mem = clamp_rlim(limits.memory_bytes);
        let _ = set_rlimit(libc::RLIMIT_DATA, mem);
    }

    // RLIMIT_NOFILE — file descriptor ceiling.
    let nofile = clamp_rlim(limits.max_open_files);
    let _ = set_rlimit(libc::RLIMIT_NOFILE, nofile);

    // Network isolation: Linux only. macOS has no equivalent syscall, and
    // research.md §R-006 documents the degradation. On Linux, `unshare`ing
    // CLONE_NEWNET requires CAP_SYS_ADMIN in the current user namespace —
    // which is typically available in `cargo test` sessions running as the
    // invoking user only when CAP_SYS_ADMIN is granted. If unshare fails we
    // proceed without hard network isolation; the classifier will still map
    // `connect()` refusals to the `network` limit by stderr sniffing.
    #[cfg(target_os = "linux")]
    if !limits.allow_network {
        let _ = try_unshare_netns();
    }

    Ok(())
}

fn clamp_rlim(value: u64) -> libc::rlim_t {
    // `libc::rlim_t` is `u64` on every supported Unix target. We keep the
    // saturating cast behind a helper so the intent stays visible if that
    // ever changes.
    libc::rlim_t::try_from(value).unwrap_or(libc::rlim_t::MAX)
}

// `RLIMIT_*` constants are `__rlimit_resource_t` on Linux (an enum) and
// `c_int` on macOS. We accept `ResourceId` which aliases to the right type
// for each platform.
#[cfg(target_os = "linux")]
type ResourceId = libc::__rlimit_resource_t;
#[cfg(not(target_os = "linux"))]
type ResourceId = libc::c_int;

fn set_rlimit(resource: ResourceId, value: libc::rlim_t) -> io::Result<()> {
    let rlim = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    // SAFETY: `setrlimit` is a standard POSIX syscall with a stable ABI.
    // `rlim` is a well-formed, fully-initialized `libc::rlimit` stored on the
    // stack, outliving the call. `resource` is one of the constants from
    // `libc::RLIMIT_*`. The call modifies only the calling process's resource
    // limits — no shared state is mutated from Rust's perspective.
    let ret = unsafe { libc::setrlimit(resource, &raw const rlim) };
    if ret == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn try_unshare_netns() -> io::Result<()> {
    // SAFETY: `unshare` is a standard Linux syscall. `CLONE_NEWNET` is a
    // valid flag constant. The call affects only the calling thread's
    // namespaces and does not touch Rust-level shared state. We ignore
    // EPERM / ENOSPC and let the classifier fall back to stderr sniffing.
    let ret = unsafe { libc::unshare(libc::CLONE_NEWNET) };
    if ret == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Raw outcome from `waitpid` before classification.
struct RawOutcome {
    exit_code: Option<i32>,
    signal: Option<i32>,
    stderr: String,
}

/// Wait for `child`, enforcing the wall-clock deadline by SIGKILL.
///
/// Returns `(RawOutcome, wall_clock_exceeded)`. On wall-clock expiry the child
/// is killed and the flag is set; callers map it into
/// [`EvaluatorError::SandboxLimitExceeded`].
fn wait_with_wall_clock(child: &mut Child, wall_clock: Duration) -> (RawOutcome, bool) {
    let (tx, rx) = mpsc::channel::<()>();
    let pid = child.id().cast_signed();
    let deadline = Instant::now() + wall_clock;

    // Parent polls with a short interval so tests don't wait 120s of real
    // time to observe enforcement.
    let poll_interval = Duration::from_millis(25);

    let waiter = thread::spawn(move || {
        let mut status = 0i32;
        loop {
            // SAFETY: `waitpid` is a standard POSIX syscall. `pid` is a
            // valid child PID owned by the calling process (the process we
            // just spawned). `&raw mut status` is a valid, fully-initialized
            // `i32` on the stack. `WNOHANG` is a well-defined constant. No
            // Rust-level shared state is mutated.
            let ret = unsafe { libc::waitpid(pid, &raw mut status, libc::WNOHANG) };
            if ret == pid {
                return Some(status);
            } else if ret == -1 {
                // ECHILD means the process was already reaped (unlikely) —
                // treat it as "no status" and bail. Any other errno is a
                // bug in the caller; bail the same way.
                return None;
            }
            if rx.recv_timeout(poll_interval).is_ok() {
                // Kill signal — reap once more to collect the status.
                // SAFETY: same invariants as above; we only pass flags we
                // control.
                let _ = unsafe { libc::waitpid(pid, &raw mut status, 0) };
                return Some(status);
            }
        }
    });

    let mut wall_clock_exceeded = false;
    let status_code;
    loop {
        if waiter.is_finished() {
            status_code = waiter.join().unwrap_or(None);
            break;
        }
        if Instant::now() >= deadline {
            // Signal child to die, then tell the waiter to reap.
            // SAFETY: `kill` is a standard POSIX syscall. `pid` is the child
            // we spawned; SIGKILL is a well-defined signal constant. No
            // Rust-level shared state is involved.
            let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
            let _ = tx.send(());
            wall_clock_exceeded = true;
            status_code = waiter.join().unwrap_or(None);
            break;
        }
        thread::sleep(poll_interval);
    }

    let (exit_code, signal) = decode_status(status_code);

    // Drain stderr after the child is gone so the pipe doesn't deadlock.
    let mut stderr_buf = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut stderr_buf);
    }

    (
        RawOutcome {
            exit_code,
            signal,
            stderr: stderr_buf,
        },
        wall_clock_exceeded,
    )
}

fn decode_status(status: Option<i32>) -> (Option<i32>, Option<i32>) {
    let Some(status) = status else {
        return (None, None);
    };
    // Use ExitStatus::from_raw to reuse std's decoding.
    let es = std::process::ExitStatus::from_raw(status);
    (es.code(), es.signal())
}

fn classify(
    outcome: &RawOutcome,
    wall_clock_exceeded: bool,
    limits: &SandboxLimits,
) -> Option<String> {
    if wall_clock_exceeded {
        return Some("wall_clock".to_string());
    }

    if let Some(sig) = outcome.signal {
        // SIGXCPU (cpu time) is 24 on both Linux and macOS.
        if sig == libc::SIGXCPU {
            return Some("cpu".to_string());
        }
        // SIGSEGV / SIGBUS on a memory-pressure exit path — map to memory.
        if sig == libc::SIGSEGV || sig == libc::SIGBUS {
            return Some("memory".to_string());
        }
    }

    // Stderr heuristics for limits that surface as libc errno strings.
    let stderr_lower = outcome.stderr.to_ascii_lowercase();
    if stderr_lower.contains("too many open files") {
        return Some("fds".to_string());
    }
    if stderr_lower.contains("cannot allocate memory") || stderr_lower.contains("out of memory") {
        return Some("memory".to_string());
    }
    if !limits.allow_network
        && (stderr_lower.contains("network is unreachable")
            || stderr_lower.contains("connection refused")
            || stderr_lower.contains("operation not permitted")
            || stderr_lower.contains("name or service not known")
            || stderr_lower.contains("no address associated with hostname")
            || stderr_lower.contains("could not resolve host")
            || stderr_lower.contains("temporary failure in name resolution"))
    {
        return Some("network".to_string());
    }

    None
}
