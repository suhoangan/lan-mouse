# Proposal

## Why

A local cursor pull-back check introduced in `input-capture/src/windows/event_thread.rs` causes rapid oscillation between barrier activation and deactivation on every mouse move at 1000 Hz. This floods both the local Windows message loop and the remote macOS event tap with hundreds of `CaptureBegin`/`Enter`/`Release` events per second, resulting in 100% CPU lockup and completely freezing mouse, trackpad, and Bluetooth receiver input. Additionally, forked GitHub Actions release workflows fail with HTTP 403 because the default `GITHUB_TOKEN` lacks write permissions to publish release tags.

## What Changes

- **Remove local pull-back deactivation**: Delete the rapid cursor coordinate threshold check from `input-capture/src/windows/event_thread.rs` that prematurely released capture when mouse coordinates shifted, restoring standard Software KVM screen-edge transition semantics.
- **Retain clean Ack timeout**: Maintain the 1.5-second timeout in `src/capture.rs` for `State::WaitingForAck`, ensuring mouse capture automatically releases if the remote client is offline or unresponsive.
- **Emergency release chord assurance**: Guarantee that the keyboard release chord (`Ctrl + Shift + Alt + Meta`) cleanly breaks any hung capture session.
- **Fix GitHub Actions workflow permissions**: Update `.github/workflows/release.yml` with `permissions: contents: write` so GitHub Actions can publish release binaries and tags on forked repositories.

## Capabilities

### New Capabilities
- `input-capture`: Durable invariants for screen edge transition, barrier entry, remote control pass-through, and connection acknowledgment timeouts.

### Modified Capabilities
<!-- No pre-existing capability specs exist in the repository. -->

## Impact

- **Affected Modules**:
  - `input-capture` (Windows low-level hook event loop)
  - `lan-mouse` (`src/capture.rs` capture session lifecycle)
  - `.github/workflows/release.yml` (CI/CD release permissions)
- **Breaking Changes**: None. This eliminates CPU starvation and restores proper mouse control.
- **Migration**: None.
