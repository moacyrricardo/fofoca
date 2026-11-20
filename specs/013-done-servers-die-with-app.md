Status: done
Branch: moacyrricardo/spec-013-servers-die-with-app

# 013 — Model servers and captures die with the app

## Context

Found while measuring 005: a `whisper-server` started by a recording the day before was still
running, re-parented to systemd, holding **813 MiB of the MX230's 2 GB**. llama-server then failed
with `ErrorOutOfDeviceMemory`. fo-foca stops its children on `Drop`, but when the app itself dies
(crash, `kill`, closing a `cargo tauri dev` terminal) nothing runs and the children live on — model
servers keep VRAM, `pw-record` keeps the microphone open.

## Decision

- Every child process fo-foca spawns (whisper-server, llama-server, both `pw-record` captures) is
  started with **`PR_SET_PDEATHSIG = SIGKILL`**, so the kernel kills it when its parent goes away,
  however that happens.
- Race guard: if the parent already exited between `fork` and `exec`, the child exits at once.
- Caveat (Linux semantics): the signal fires when the **thread** that spawned the child exits.
  Children must be spawned from threads that outlive them — true today (session worker, Tauri's
  main thread for sync commands, blocking command threads that stop their server before
  returning, the CLI's main thread). Documented next to the helper.

## Implementation

- `fofoca-core/src/proc.rs`: `die_with_parent(&mut Command)` — `pre_exec` with
  `libc::prctl(PR_SET_PDEATHSIG, SIGKILL)` and the `getppid()` check; no-op off Linux.
- Applied in `servers.rs` (both servers) and `capture.rs` (`pw-record`).
- Test: a child spawned with the helper from a short-lived thread is killed when that thread ends.

## Known Gaps

- Servers orphaned by builds from before this change still need killing once by hand.

## Implementation Notes

- Built as specified (`proc::die_with_parent`, `libc` added as a direct dependency).
- Verified by hand: started `fofoca-cli transcribe`, `kill -9` the CLI while whisper-server was
  loaded → the server was gone within a second and VRAM use dropped to 2 MiB.
- Unit test spawns `sleep 30` from a short-lived thread and checks it is killed — which also
  demonstrates the per-thread caveat.
