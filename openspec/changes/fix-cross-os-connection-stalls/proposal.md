# Proposal

## Why

Cross-OS sessions (macOS host, Windows/macOS peer) wedge instead of failing cleanly: disabling or disconnecting the app parks the capture pipeline forever, holding system input while the cursor is hidden, and a peer that is turned off or shut down keeps stuck modifiers and a trapped pointer until an internal watchdog fires. The same pipeline also loses the first barrier crossing on a cold connection, and the inbound handshake is torn down every two seconds.

## What Changes

- **Capture stream never parks without a waker.** `InputCapture::poll_next` currently returns `Poll::Pending` after dequeuing an event whose position has no registered capture handle. The capture task sleeps forever and never reads another backend event. It must re-poll the backend instead of returning a bare `Pending`.
- **macOS event tap must never block.** The CGEventTap callback holds an async mutex via `blocking_lock` and pushes into a bounded channel with `blocking_send`. Any stall on the runtime side (including blocking clipboard I/O on the same single-threaded runtime) holds the OS tap thread until macOS disables the tap and system input freezes. The tap callback becomes non-blocking (bounded, lossy-but-coalesced sends; no cross-task lock held across backend work).
- **Clipboard I/O leaves the input path.** `arboard::Clipboard::new()`/`get_text`/`set_text` are synchronous OS calls executed inside the capture and listen event loops. They move to a dedicated task so input latency never depends on clipboard availability.
- **Cold-connection barrier entry no longer aborts the session.** A `NotConnected` send error while a background connect is still in flight currently triggers an unconditional capture release, so the first crossing on a fresh connection always bounces and only the second works. Session release becomes conditional on a definite failure rather than on a not-yet-established connection.
- **Graceful shutdown notifies the peer.** The service handles only `CTRL+C`, so quitting the app (window close, `SIGTERM`, Windows session end/shutdown) tears down capture and emulation without sending synthetic key-ups, a cleared modifier mask, or `ProtoEvent::Leave`. Add termination signals that reuse the existing release path, so a shutdown looks to the peer like a normal leave.
- **Inbound handshake stops being cancelled every 2s.** The `webrtc-rs#614` workaround drops the in-flight `accept()` future on each 2s tick, aborting a mid-flight DTLS handshake for a slow or sleeping peer. The workaround keeps its purpose without cancelling an in-progress handshake.
- **Two-way authorization on connect.** Outbound connections currently verify nothing (`insecure_skip_verify: true`), while inbound requires the peer fingerprint in `authorized_fingerprints`. Approving a peer on one machine therefore authorizes only that direction. A successful outbound connection to an explicitly configured client records the peer's certificate fingerprint so the reverse direction is authorized by the same approval. **BREAKING** (trust model): the outbound leg trusts whatever answers at the configured address instead of pinning; the reverse-direction approval inherits that trust.
- **Windows console verification.** `windows_subsystem = "windows"` is present in `src/main.rs`, but the packaged release artifact predates it. Confirm the shipped binary is built with it and that logs still reach a launching terminal via `AttachConsole(ATTACH_PARENT_PROCESS)`.

## Capabilities

### New Capabilities

- `input-capture`: durable invariants for the capture stream lifecycle — event delivery, non-blocking OS input callbacks, barrier entry across cold connections, and release on shutdown or unresponsive peers.
- `connection-lifecycle`: durable invariants for the DTLS session — handshake acceptance, liveness detection, peer authorization in both directions, and peer notification on disconnect or shutdown.

### Modified Capabilities

<!-- No pre-existing capability specs exist in the repository (openspec list --specs is empty). -->

## Impact

- **Affected Modules**:
  - `input-capture` (`src/lib.rs` stream contract, `src/macos.rs` tap callback)
  - `lan-mouse` (`src/capture.rs` release conditions, `src/listen.rs` accept loop, `src/connect.rs` peer authorization, `src/emulation.rs` clipboard dispatch, `src/service.rs` shutdown signals, `src/clipboard.rs`)
  - Packaging (`src/main.rs` console behavior verification only)
- **Breaking Changes**: Outbound peer trust becomes "configured address" rather than pinned fingerprint; the reverse direction is authorized from that connection. Operators who relied on the DTLS handshake refusing an unexpected peer at a configured address should pin fingerprints instead.
- **Out of Scope**: Wayland/libei/layer-shell backends, protocol wire-format changes (no version bump required), X11 capture (still unimplemented), multi-monitor DPI scaling.
- **Verification Constraint**: macOS tap and cursor behavior cannot be executed on the development host; those changes ship with documented manual verification steps.
