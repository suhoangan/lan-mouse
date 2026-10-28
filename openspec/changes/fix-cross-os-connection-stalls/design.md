# Design

## Context

See proposal.md — Why. Constraints that shape the approach:

- The service runs on a **current-thread** Tokio runtime (`src/main.rs` `run_async`), so any synchronous call inside an event loop blocks *every* task on that thread — including the ones the macOS event-tap thread waits on. Clipboard calls (`arboard`) and `blocking_lock` are therefore systemic, not local, stalls.
- `InputCapture` (`input-capture/src/lib.rs`) implements `Stream` manually and fans one backend event out to every handle registered at that position. Its `poll_next` is the single choke point between every backend and `src/capture.rs`.
- The macOS tap callback runs on its own thread and communicates with the runtime through two bounded `mpsc::channel(32)` queues (`input-capture/src/macos.rs`). Channel sends there are the only backpressure point between the OS and the runtime.
- DTLS identity is a self-signed certificate generated per machine (`src/crypto.rs`); `authorized_fingerprints` in the config is the sole trust store. The listen side verifies it; the connect side passes `insecure_skip_verify: true` (`src/connect.rs:60`).
- The service shutdown path (`src/service.rs:151`) races only `signal::ctrl_c()`; capture/emulation terminate without peer notification.
- Release builds use `panic = "abort"`, so an `expect` on a disconnect path kills the process rather than unwinding.

## Goals / Non-Goals

**Goals:**
- Remove every path where the capture stream can park without a waker.
- Make OS callback duration independent of runtime load.
- Preserve the existing protocol and config format; one approval authorizes both directions.
- Ensure a normal quit is indistinguishable to the peer from a normal leave.

**Non-Goals:**
- Any change to the wire format, clipboard chunking, or DTLS parameters.
- Changing the Wayland/X11/libei capture or emulation backends.
- Fingerprint pinning in config (a stricter alternative to address-based outbound trust; deferred, see Open Questions).
- Making `panic = "abort"` builds resilient (out of scope; noted as a risk).

## Decisions

### 1. Fix the stream stall at the choke point, not at each caller
`InputCapture::poll_next` must not return `Poll::Pending` after consuming a backend event. When the position has no registered handle, the event is dropped and the loop re-polls the backend within the same `poll_next` call.

- *Why*: every caller (`src/capture.rs`, `src/emulation.rs`) assumes a pending stream means "backend has nothing", not "backend value consumed and discarded". Returning `Pending` without a waker registration violates the `Stream` contract and is unrecoverable from outside.
- *Alternative considered*: register a waker that is notified on the next `create` request. Rejected — it needs a second wake source for every backend and still loses events in the same poll.

### 2. Lossy, coalescing sends in the macOS tap callback
Replace `blocking_send` on both channels with `try_send`, and drop `client_state.blocking_lock()` from the callback path in favor of state that does not need the async mutex.

- *Why*: a callback that waits is a callback macOS kills after ~1s, and the kill leaves the cursor hidden. Losing a motion sample is invisible; losing the tap is a system-wide input freeze.
- *Alternative considered*: keep blocking sends but enlarge the channel. Rejected — the queue fills exactly when the runtime is stalled, so capacity only moves the deadline.
- The callback still coalesces motion through the existing accumulated-delta mechanism, so dropped samples are replayed as one delta instead of vanishing.

### 3. Move clipboard off both event loops
Run clipboard read/write and the "send changed clipboard" step on a dedicated task fed by a request channel; capture/listen loops only enqueue.

- *Why*: `arboard` performs synchronous window-system calls, and both loops are single-threaded hot paths. Clipboard is inherently slow and optional; input is neither.
- *Alternative considered*: `spawn_blocking` per call. Rejected — it still allocates a blocking task per clipboard event and serializes nothing, so ordering guarantees (the `LAST_SENT`/`LAST_RECEIVED` dedup state) would need a lock anyway.

### 4. Distinguish "not connected yet" from "connection failed"
`LanMouseConnection::send` already returns a typed error. The capture session releases only on the error variant that means a definite failure, and keeps the ack deadline running for the not-yet-connected case.

- *Why*: the current unconditional release makes the first cold crossing always bounce, which reads to users as "connects one way, then the other".
- *Alternative considered*: eagerly connect when a client is activated. Rejected — it connects to every configured client whether used or not, and duplicates the existing lazy-connect path.

### 5. Shutdown reuses the release path
Add termination signal handling (SIGTERM on Unix; console-ctrl and session-end on Windows) that triggers the same `release_capture` sequence used by the release-bind chord before terminating capture and emulation.

- *Why*: one code path means key-up synthesis, modifier clearing, and the leave event cannot drift apart between "user pressed the chord" and "user quit the app".
- *Alternative considered*: a separate shutdown-only notify path. Rejected — duplicate logic is exactly what produced the current gap.

### 6. Listener keeps its workaround without cancelling handshakes
Keep the periodic tick that works around `webrtc-rs#614`, but ensure an in-progress `accept()` future is preserved across ticks instead of being dropped and recreated.

- *Why*: `DTLSListener::accept` performs the handshake inline, so dropping the future aborts it mid-flight; a peer waking from sleep then needs several attempts.
- *Alternative considered*: remove the workaround. Rejected — it exists to avoid a known busy-accept bug; removing it risks the 100% CPU failure mode documented in the earlier change.

### 7. Record the outbound peer fingerprint as trust
After a successful outbound handshake, read the peer certificate from the connection state, compute the same fingerprint the listener uses, and store it in `authorized_keys` (persisting with the existing config write).

- *Why*: the machine was chosen by the operator, and the reverse direction then works from the same approval. This is the smallest change that satisfies "connect should connect two ways".
- *Alternative considered*: pin per client in config. Strictly stronger, but it adds config surface and silently breaks existing configs; deferred to Open Questions.
- The fingerprint is read from the *completed* connection state, never from `insecure_skip_verify`'s bypass, so the stored value is the peer's real certificate.

### 8. Console verification, not console code
`windows_subsystem = "windows"` plus `AttachConsole(ATTACH_PARENT_PROCESS)` already exist in `src/main.rs`. No code change; the deliverable is confirming the released artifact contains them and that both launch modes behave.

## Risks / Trade-offs

- **[Trust model weakened]** Address-based outbound trust accepts whoever answers at the configured address on a LAN → Mitigation: only for explicitly configured clients; record and surface the fingerprint in the UI so a surprising peer is visible; documented as breaking in the proposal; pinning available as a follow-up.
- **[Dropped motion samples]** Lossy tap sends can drop a motion event under sustained backpressure → Mitigation: coalescing replays the accumulated delta on the next send; only the intermediate sample is lost, not the displacement.
- **[macOS behavior unverified locally]** The tap and cursor changes cannot be executed on the development host → Mitigation: `cargo check`/`clippy` pass for `x86_64-apple-darwin`, then this manual check on a Mac before release:
  1. Cross into a remote client; the local cursor hides and input reaches the peer.
  2. Press the release-bind chord; the local cursor reappears immediately and the peer holds no modifiers.
  3. Quit the app on the peer; the local cursor reappears and the peer's held keys are released.
  4. Log out or shut down the peer; the local side releases input without waiting for its liveness timeout.
  5. Under a heavy local load (e.g. a busy build), confirm input keeps flowing and the tap is not disabled.
- **[`panic = "abort"` on disconnect paths]** An `expect` reached during teardown kills the process → Mitigation: teardown code touched by this change uses total functions; the remaining `expect`s are out of scope and listed as a follow-up.
- **[Signal handling differs per platform]** SIGTERM, console control events, and session-end notifications have different delivery semantics → Mitigation: register each platform's mechanism behind the same handler, and keep the peer's liveness timeout as the backstop.

## Migration Plan

1. No config or protocol migration. Existing `authorized_fingerprints` entries keep working; the outbound fingerprint is added on the first successful connection to a configured client.
2. Rollback: revert the change; the extra fingerprints in the config are inert entries that the previous version simply ignores.
3. Ship order: capture-stream and shutdown fixes first (they remove hangs), authorization last (it changes the trust model) so a regression can be bisected cleanly.

## Open Questions

- Should outbound trust be upgraded to a per-client pinned fingerprint instead of address-based trust? It is strictly safer, but changes config surface and would need a decision on how existing configs migrate. Deferrable: the current decision is forward-compatible with adding a field later.
- Should the clipboard path also handle non-text formats? Out of scope here; text sync keeps its current behavior.
