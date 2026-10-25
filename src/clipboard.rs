use std::cell::RefCell;

thread_local! {
    static LAST_SENT_TEXT: RefCell<Option<String>> = const { RefCell::new(None) };
    static LAST_RECEIVED_TEXT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Read text from the system clipboard
pub fn read_clipboard_text() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    cb.get_text().ok()
}

/// Write text into the system clipboard, recording it as locally received to prevent echo loops
pub fn write_clipboard_text(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    LAST_RECEIVED_TEXT.with_borrow_mut(|last| {
        *last = Some(text.to_owned());
    });
    if let Ok(mut cb) = arboard::Clipboard::new() {
        cb.set_text(text.to_owned()).is_ok()
    } else {
        false
    }
}

/// Returns text if the local clipboard has changed and was not recently received from a remote peer
pub fn get_text_if_changed() -> Option<String> {
    let current = read_clipboard_text()?;
    if current.is_empty() {
        return None;
    }
    // Don't send what we just received from the other machine
    let is_recent_received =
        LAST_RECEIVED_TEXT.with_borrow(|last| last.as_deref() == Some(&current));
    if is_recent_received {
        return None;
    }
    // Don't resend what we already sent
    let is_recent_sent = LAST_SENT_TEXT.with_borrow(|last| last.as_deref() == Some(&current));
    if is_recent_sent {
        return None;
    }
    LAST_SENT_TEXT.with_borrow_mut(|last| {
        *last = Some(current.clone());
    });
    Some(current)
}
