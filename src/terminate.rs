//! Termination signal for the service.
//!
//! The daemon can be stopped by the GUI exiting (its IPC socket closes), by a
//! service manager sending `SIGTERM`, or by `Ctrl+C` in a terminal. The GUI
//! case is handled by the frontend listener; this module covers the signals.
//!
//! `SIGKILL` and power loss cannot be observed; those rely on the peer's
//! liveness timeout.

/// Resolves when the process is asked to terminate via `SIGTERM`.
///
/// Never resolves on platforms without it, or when the handler cannot be
/// registered — the other shutdown paths still work.
pub(crate) async fn terminate_requested() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
                log::info!("received SIGTERM, shutting down");
            }
            Err(e) => {
                log::warn!("could not register SIGTERM handler: {e}");
                std::future::pending::<()>().await;
            }
        }
    }

    #[cfg(not(unix))]
    std::future::pending::<()>().await;
}
