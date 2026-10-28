# Design

## Context

See `proposal.md` for problem background. The Windows capture engine in `input-capture/src/windows/event_thread.rs` manages the low-level Windows mouse hook (`WH_MOUSE_LL`). When `ACTIVE_CLIENT` is active, the hook returns `LRESULT(1)` to suppress local mouse input and dispatches events via `EVENT_TX`. A recent commit added a local `pulled_back` coordinate threshold check inside `check_client_activation` that cleared `ACTIVE_CLIENT` if `curr_pos` moved 10px inward. Because `curr_pos` remains adjacent to the display boundary, the barrier re-triggered on the next millisecond, causing an endless 1000 Hz loop of `CaptureBegin` and `Enter` network events.

## Goals / Non-Goals

**Goals:**
- Eliminate the high-frequency activation/deactivation oscillation and CPU starvation.
- Permit normal mouse navigation in all directions across the remote desktop.
- Prevent local mouse input freeze if the remote peer is unreachable using a 1.5s Ack timeout.
- Enable successful release artifact publishing on forked repositories.

**Non-Goals:**
- Altering the Wayland, X11, or macOS capture backends.
- Changing UDP wire encryption or the DTLS handshake.

## Decisions

### 1. Remove Local Pull-Back Coordinate Check
In `input-capture/src/windows/event_thread.rs`, revert `check_client_activation` to simply return `ret` (true) while `ACTIVE_CLIENT` is active.
- *Rationale*: When controlling a remote client, physical mouse deltas in all directions are valid remote navigation. Returning from the remote machine to the local screen is cleanly managed by the remote screen boundary protocol (`ProtoEvent::Leave`) or the local timeout/release chord.
- *Alternative considered*: Increasing the pull-back threshold (e.g. 100px). Rejected because any arbitrary coordinate threshold still interferes with wide cursor sweeps across the remote desktop and risks edge oscillation.

### 2. Retain 1.5-Second Ack Timeout in Capture Session
In `src/capture.rs`, preserve the timeout branch inside `tokio::select!`:
```rust
_ = tokio::time::sleep(Duration::from_millis(1500)), if self.state == State::WaitingForAck && self.active_client.is_some() => {
    log::warn!("connection to client timed out waiting for Ack, releasing capture");
    self.release_capture(capture).await?;
}
```
- *Rationale*: Guarantees that if a remote client is disconnected or fails to answer, the host machine automatically restores pointer control within 1.5 seconds without requiring manual process termination.

### 3. Add Workflow Permissions to Release Job
In `.github/workflows/release.yml`, explicitly declare permissions for the `release` job:
```yaml
jobs:
  release:
    permissions:
      contents: write
```
- *Rationale*: Forked repositories default `GITHUB_TOKEN` to read-only, causing `softprops/action-gh-release@v2` to fail with HTTP 403. Explicit write permissions allow seamless GitHub Release creation.

## Risks / Trade-offs

- **Return to Host**: Navigating back from the remote screen requires moving the remote cursor to the shared edge or pressing the release chord (`Ctrl + Shift + Alt + Meta`). This is standard behavior across all Software KVMs (Barrier, Deskflow, Synergy).
