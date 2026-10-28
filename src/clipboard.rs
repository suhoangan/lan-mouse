use std::sync::mpsc::{self, Sender};
use std::thread;

use tokio::sync::oneshot;

/// System clipboard access, off the event loops.
///
/// `arboard` performs synchronous window-system calls that can block for
/// seconds when another application holds the clipboard. Running them on the
/// runtime thread stalls every task sharing it, including the OS input callback
/// handoff. This type owns a dedicated thread and talks to it over channels, so
/// callers never execute a clipboard syscall themselves.
///
/// The echo-suppression state lives on that thread, which makes it shared
/// between all clones — matching the previous thread-local behavior on the
/// single-threaded runtime.
#[derive(Clone)]
pub struct Clipboard {
    tx: Sender<Request>,
}

enum Request {
    /// Replace the local clipboard with text received from a peer.
    Write(String),
    /// Read the local clipboard if it changed since it was last sent or received.
    ReadChanged(oneshot::Sender<Option<String>>),
}

impl Clipboard {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Request>();
        thread::Builder::new()
            .name("lan-mouse-clipboard".into())
            .spawn(move || {
                let mut last_sent: Option<String> = None;
                let mut last_received: Option<String> = None;
                while let Ok(request) = rx.recv() {
                    match request {
                        Request::Write(text) => {
                            if text.is_empty() {
                                continue;
                            }
                            last_received = Some(text.clone());
                            if let Ok(mut cb) = arboard::Clipboard::new() {
                                let _ = cb.set_text(text);
                            }
                        }
                        Request::ReadChanged(reply) => {
                            let current = arboard::Clipboard::new()
                                .ok()
                                .and_then(|mut cb| cb.get_text().ok())
                                .filter(|text| !text.is_empty());
                            let result = current.filter(|text| {
                                should_send(text, last_sent.as_deref(), last_received.as_deref())
                            });
                            if let Some(text) = &result {
                                last_sent = Some(text.clone());
                            }
                            // receiver may be gone; nothing to do then
                            let _ = reply.send(result);
                        }
                    }
                }
            })
            .expect("spawn clipboard thread");
        Self { tx }
    }

    /// Replace the local clipboard, recording it as received so it is not echoed back.
    pub fn write(&self, text: String) {
        let _ = self.tx.send(Request::Write(text));
    }

    /// Resolve to the local clipboard text if it changed since the last exchange.
    ///
    /// The clipboard thread does the work; the caller only awaits the result.
    pub async fn read_changed(&self) -> Option<String> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(Request::ReadChanged(reply)).is_err() {
            return None;
        }
        rx.await.ok().flatten()
    }
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

/// Decides whether clipboard text is worth sending to a peer, given the last
/// text sent and the last text received. Text that came from the peer, or that
/// was already sent, must not be echoed back.
fn should_send(current: &str, last_sent: Option<&str>, last_received: Option<&str>) -> bool {
    !current.is_empty() && last_received != Some(current) && last_sent != Some(current)
}

#[cfg(test)]
mod tests {
    use super::should_send;

    /// Text received from a peer must never be sent back to it.
    #[test]
    fn received_text_is_not_sent_back() {
        assert!(!should_send("from peer", None, Some("from peer")));
    }

    /// Text already sent must not be sent twice.
    #[test]
    fn already_sent_text_is_not_resent() {
        assert!(!should_send("already sent", Some("already sent"), None));
    }

    /// Genuinely new local text is sent.
    #[test]
    fn new_text_is_sent() {
        assert!(should_send("new", Some("old"), Some("older")));
    }

    /// An empty clipboard is never forwarded.
    #[test]
    fn empty_text_is_not_sent() {
        assert!(!should_send("", None, None));
    }
}
