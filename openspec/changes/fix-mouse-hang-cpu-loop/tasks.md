# Tasks

## 1. Input Capture Hook Stabilization

- [x] 1.1 Remove local cursor pull-back coordinate check in `input-capture/src/windows/event_thread.rs`
- [x] 1.2 Verify that pointer motion across all directions streams properly without barrier deactivation oscillation
- [x] 1.3 Run unit tests for `input-capture` to ensure zero regressions

## 2. Capture Session Ack Timeout

- [x] 2.1 Verify and enforce 1.5-second `Ack` timeout in `src/capture.rs` inside `do_capture_session`
- [x] 2.2 Confirm that unresponsive or offline remote peers trigger automatic capture release back to host

## 3. CI/CD Release Workflow Permissions

- [x] 3.1 Update `.github/workflows/release.yml` with `permissions: contents: write` on the `release` job
- [x] 3.2 Commit, push to `main`, and verify successful release publication on GitHub Actions
