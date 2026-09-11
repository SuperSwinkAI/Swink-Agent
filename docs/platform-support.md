# Platform Support

Swink Agent builds and runs on Linux, macOS and Windows. A few features depend
on POSIX-only primitives and are compiled out on Windows. This page records
which ones, why, and what happens if you call them anyway.

## Support matrix

| Feature | Linux | macOS | Windows | Notes |
| --- | --- | --- | --- | --- |
| Core agent loop (`swink-agent`) | ✅ | ✅ | ✅ | |
| Providers, memory, tools, plugins | ✅ | ✅ | ✅ | |
| Local TUI (`swink-agent-tui`) | ✅ | ✅ | ✅ | External editor defaults to `notepad` on Windows |
| Script tools (`ScriptTool`) | ✅ | ✅ | ✅ | `sh -c` on Unix, `cmd /C` on Windows |
| Web browse plugin | ✅ | ✅ | ✅ | Node lookup uses `which` on Unix, `where` on Windows |
| RPC server and client (`swink-agent-rpc`) | ✅ | ✅ | ❌ | Unix domain sockets only |
| Remote TUI (`swink-agent-tui-remote`) | ✅ | ✅ | ❌ | Rides the RPC transport |
| Sandboxed execution evaluator | ✅ | ✅ | ❌ | `/bin/sh` plus POSIX `setrlimit` |

Everything marked ❌ still compiles on Windows. Only the entry points are
unavailable, so a Windows build of the workspace is a normal, warning-free
build.

## RPC and remote TUI

The RPC transport is a Unix domain socket. Two POSIX mechanisms carry the
security model, and neither has a drop-in Windows equivalent:

- The socket file is created with mode `0600`, so only the owning user can open
  it.
- Every accepted connection is checked against the peer's user id, read through
  `SO_PEERCRED` on Linux and `getpeereid` on the BSDs and macOS. A connection
  from any other user is closed before the handshake.

On Windows, `AgentServer::serve` returns an `io::Error` with
`ErrorKind::Unsupported`, and `AgentClient::connect` returns the matching
`RpcError`. Both carry the message "Unix socket transport requires a Unix
host". The integration tests in `rpc/tests` and `tui-remote/tests` are gated
behind `#![cfg(unix)]`.

Porting this would mean a named-pipe transport with a Windows security
descriptor in place of the file mode, and a token-based caller check in place
of the peer-credential read. That is a separate transport implementation, not a
conditional branch, so it is deliberately out of scope. Run the agent locally on
Windows, or host the RPC server on a Unix machine.

## Sandboxed execution evaluator

`SandboxedExecutionEvaluator` runs an extracted code snippet under resource
limits. The default runner writes a shell script and invokes `/bin/sh`, and the
limits (CPU time, address space, file size) are applied with POSIX `setrlimit`
in the child between `fork` and `exec`. Windows job objects express similar
limits, but through an unrelated API and with different semantics.

The evaluator is gated behind `#[cfg(target_family = "unix")]`, as is its test
module in `eval/tests/suite/main.rs`. On Windows the type is not compiled, so
the failure is a name-resolution error at build time rather than a runtime
surprise. Every other evaluator is cross-platform.

## Windows CI coverage

Pull requests target `integration`. The Windows job that runs on pull requests
is compile-only; the full Windows test suite runs on push to `main`. Windows
test regressions can therefore reach `integration` without a red check. Treat a
green pull request as evidence that Windows compiles, not that it passes.
