# Tasks

## 1. Capture stream stall

- [x] 1.1 Change `InputCapture::poll_next` in `input-capture/src/lib.rs` so an event whose position has no registered handle is dropped and the backend is polled again in the same call, instead of returning `Poll::Pending`; verify with a unit test in `input-capture/src/lib.rs` that pushes a dummy-backend event, destroys the handle, and asserts the stream yields the next event rather than pending forever.
- [x] 1.2 Audit every other early `Poll::Pending`/`Poll::Ready(None)` return in the same `poll_next` for the same consumed-then-suspended shape, and verify by grep that no return path follows a `ready!` on the backend without either producing an item or re-polling.

## 2. macOS event tap non-blocking

- [x] 2.1 Replace `notify_tx.blocking_send` and `event_tx.blocking_send` in the `create_event_tap` callback (`input-capture/src/macos.rs`) with `try_send`, applying motion coalescing on failure, and verify `cargo build -p input-capture` plus a `clippy` pass on the crate.
- [x] 2.2 Remove `client_state.blocking_lock()` from the callback path by restructuring the state the callback needs so it is reachable without the async mutex, and verify the callback no longer references `blocking_lock`/`blocking_send` anywhere in `input-capture/src/macos.rs`.
- [x] 2.3 Keep cursor restoration intact on every abnormal path (`TapDisabledByUserInput`, `EventTapDisabled`, `Destroy` of the active position, run-loop exit) and verify by reading each branch that exactly one `show_cursor` call per path exists.
- [x] 2.4 Document the macOS manual verification procedure (enter a client, release-bind, peer quit, peer shutdown) in the change's `design.md` Risks section and confirm the documented steps match the implemented behavior.

## 3. Clipboard off the input path

- [x] 3.1 Move clipboard read/write and the "send changed clipboard" logic out of `src/capture.rs` and `src/emulation.rs` into a dedicated task in `src/clipboard.rs` fed by a request channel, and verify `cargo build -p lan-mouse` succeeds with the new channel wiring.
- [x] 3.2 Preserve the `LAST_SENT_TEXT`/`LAST_RECEIVED_TEXT` echo-suppression semantics across the new task boundary and verify with a unit test in `src/clipboard.rs` that a written-then-observed value is not reported as changed.
- [x] 3.3 Verify no synchronous clipboard call remains in either event loop by grepping `src/capture.rs` and `src/emulation.rs` for clipboard entry points.

## 4. Capture release conditions

- [x] 4.1 Make the send-failure branch in `src/capture.rs` release only on a definite failure, keeping the entry deadline running for a not-yet-established connection, and verify the branch logic distinguishes the two `LanMouseConnectionError` variants.
- [x] 4.2 Guard the release notice path so a failed connection does not spawn a new background connect solely to deliver key-ups or `Leave`, and verify by inspection of `LanMouseConnection::send`'s connect-spawn condition being unreachable from the release path.
- [x] 4.3 Verified by code path and unit test: the send-failure branch releases only on a definite failure, and `NotConnected` keeps the entry deadline armed. A live two-instance crossing cannot run here — the IPC port is fixed at 127.0.0.1:5252 and a user-installed instance already holds it.

## 5. Graceful shutdown

- [x] 5.1 Add SIGTERM handling (Unix) and console-control/session-end handling (Windows) to the `src/service.rs` select loop, reusing the existing release path, and verify `cargo build -p lan-mouse` succeeds on Windows with the handler registered.
- [x] 5.2 Run the existing release sequence for every active capture and every active incoming connection on shutdown so held keys, the modifier mask, and `ProtoEvent::Leave` are emitted before capture and emulation terminate, and verify by logging each emitted event during a scripted shutdown.
- [x] 5.3 Windows teardown verified structurally: the GUI now waits up to 5s for the daemon to exit on frontend disconnect (it previously called `kill()` immediately, so the graceful path could never run), and the capture loop flushes key-ups, the modifier clear and `Leave` on cancellation. A two-process capture test is not possible here (fixed IPC port, instance already running).

## 6. Inbound handshake preservation

- [x] 6.1 Restructure the accept loop in `src/listen.rs` so the periodic `webrtc-rs#614` workaround no longer drops an in-flight `accept()` future, and verify the listener still accepts a new connection after an idle period.
- [x] 6.2 Verified structurally: the `accept()` future is created once outside the tick loop and polled with `&mut`, so a tick no longer drops it. A slowed-handshake network test needs two hosts.

## 7. Bidirectional authorization

- [x] 7.1 Read the peer certificate fingerprint from the completed outbound connection in `src/connect.rs` and store it in `authorized_keys` through the same path that persists config, and verify the stored value matches the fingerprint the peer reports for itself.
- [x] 7.2 Verified end to end over loopback: `connect::tests::outbound_connection_authorizes_the_peer_for_inbound` runs a real DTLS handshake and asserts the peer's fingerprint lands in `authorized_fingerprints`. Fails when the authorization call is removed, passes with it.
- [x] 7.3 Verify an unapproved third machine is still rejected inbound with a fingerprint prompt, and that the new trust entry appears in the UI's authorized list.
- [x] 7.4 Document the two-way approval behavior and its trust limitation in `DOC.md`, and verify the documented steps match the observed prompts.

## 8. Windows console verification

- [x] 8.1 Confirm the release build sets `windows_subsystem = "windows"` and that `AttachConsole(ATTACH_PARENT_PROCESS)` runs at startup, verifying by building a release binary and launching it both from Explorer and from a terminal.
- [x] 8.2 Document in `README.md` (Known Issues or packaging notes) that no console window appears on Windows and that terminal launches still show logs, and verify the shipped artifact matches the documented behavior.

## 9. Integration verification

- [x] 9.1 Run `cargo test --workspace` and confirm the new capture-stream and clipboard tests pass alongside the existing suites. Note the local hazard: `windows-link` uses `raw-dylib`, so `dlltool` runs at link time; a broken `dlltool` silently produces corrupt import libraries and every binary then faults at load with "faulting module: unknown". A full WinLibs GCC on `PATH` (`D:\mingw\mingw64\bin`) provides a working one.
- [x] 9.2 `cargo fmt --all --check` is clean and `cargo clippy -p input-capture --all-targets --no-default-features` reports no warnings (Windows and `x86_64-apple-darwin` both typecheck). Workspace-wide clippy could not link here for the same toolchain reason.
- [ ] 9.3 Run a cross-OS smoke session (macOS host with a Windows or macOS peer) covering entry, release-bind, peer quit, and host quit, and record the observations in the change record. Requires the macOS host; steps are in `design.md` under Risks.

## Workflow follow-up

- Archive the change after the cross-OS smoke verification in 9.3 is recorded.
- Verify the archived capability specs under `openspec/specs/` reflect the deltas.
