use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Display,
    mem::swap,
    task::{Poll, ready},
};

use async_trait::async_trait;
use futures::StreamExt;
use futures_core::Stream;

use input_event::{Event, KeyboardEvent, scancode};

pub use error::{CaptureCreationError, CaptureError, InputCaptureError};

pub mod error;

#[cfg(libei)]
mod libei;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(layer_shell)]
mod layer_shell;

#[cfg(windows)]
mod windows;

#[cfg(x11)]
mod x11;

/// fallback input capture (does not produce events)
mod dummy;

pub type CaptureHandle = u64;

/// Upper bound on backend events consumed inside a single `poll_next` call.
/// Events for positions without a registered handle are dropped in place; the
/// bound keeps such a stream from monopolizing the executor in one poll.
const MAX_EVENTS_PER_POLL: usize = 64;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum CaptureEvent {
    /// capture on this capture handle is now active
    Begin,
    /// input event coming from capture handle
    Input(Event),
}

impl Display for CaptureEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureEvent::Begin => write!(f, "begin capture"),
            CaptureEvent::Input(e) => write!(f, "{e}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub enum Position {
    Left,
    Right,
    Top,
    Bottom,
}

impl Position {
    pub fn opposite(&self) -> Self {
        match self {
            Position::Left => Self::Right,
            Position::Right => Self::Left,
            Position::Top => Self::Bottom,
            Position::Bottom => Self::Top,
        }
    }
}

impl Display for Position {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pos = match self {
            Position::Left => "left",
            Position::Right => "right",
            Position::Top => "top",
            Position::Bottom => "bottom",
        };
        write!(f, "{pos}")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    #[cfg(libei)]
    InputCapturePortal,
    #[cfg(layer_shell)]
    LayerShell,
    #[cfg(x11)]
    X11,
    #[cfg(windows)]
    Windows,
    #[cfg(target_os = "macos")]
    MacOs,
    Dummy,
}

impl Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(libei)]
            Backend::InputCapturePortal => write!(f, "input-capture-portal"),
            #[cfg(layer_shell)]
            Backend::LayerShell => write!(f, "layer-shell"),
            #[cfg(x11)]
            Backend::X11 => write!(f, "X11"),
            #[cfg(windows)]
            Backend::Windows => write!(f, "windows"),
            #[cfg(target_os = "macos")]
            Backend::MacOs => write!(f, "MacOS"),
            Backend::Dummy => write!(f, "dummy"),
        }
    }
}

pub struct InputCapture {
    /// capture backend
    capture: Box<dyn Capture>,
    /// keys pressed by active capture
    pressed_keys: HashSet<scancode::Linux>,
    /// map from position to ids
    position_map: HashMap<Position, Vec<CaptureHandle>>,
    /// map from id to position
    id_map: HashMap<CaptureHandle, Position>,
    /// pending events
    pending: VecDeque<(CaptureHandle, CaptureEvent)>,
}

impl InputCapture {
    /// create a new client with the given id
    pub async fn create(&mut self, id: CaptureHandle, pos: Position) -> Result<(), CaptureError> {
        assert!(!self.id_map.contains_key(&id));

        self.id_map.insert(id, pos);

        if let Some(v) = self.position_map.get_mut(&pos) {
            v.push(id);
            Ok(())
        } else {
            self.position_map.insert(pos, vec![id]);
            self.capture.create(pos).await
        }
    }

    /// destroy the client with the given id, if it exists
    pub async fn destroy(&mut self, id: CaptureHandle) -> Result<(), CaptureError> {
        let pos = self
            .id_map
            .remove(&id)
            .expect("no position for this handle");

        log::debug!("destroying capture {id} @ {pos}");
        let remaining = self.position_map.get_mut(&pos).expect("id vector");
        remaining.retain(|&i| i != id);

        log::debug!("remaining ids @ {pos}: {remaining:?}");
        if remaining.is_empty() {
            log::debug!("destroying capture @ {pos} - no remaining ids");
            self.position_map.remove(&pos);
            self.capture.destroy(pos).await?;
        }
        Ok(())
    }

    /// release mouse
    pub async fn release(&mut self) -> Result<(), CaptureError> {
        self.pressed_keys.clear();
        self.capture.release().await
    }

    /// Drain and return every key the capture has forwarded as
    /// down-but-not-up. The caller is expected to synthesize key-up
    /// events to the remote peer for each — otherwise the peer
    /// retains phantom-held keys after capture is released. The
    /// canonical case is the release-bind chord
    /// (Ctrl+Shift+Alt+Meta): the down events were sent while
    /// capture was active, but the matching up events arrive after
    /// the local tap has flipped to passthrough and never reach
    /// the peer.
    pub fn take_pressed_keys(&mut self) -> HashSet<scancode::Linux> {
        std::mem::take(&mut self.pressed_keys)
    }

    /// destroy the input capture
    pub async fn terminate(&mut self) -> Result<(), CaptureError> {
        self.capture.terminate().await
    }

    /// creates a new [`InputCapture`]
    pub async fn new(backend: Option<Backend>) -> Result<Self, CaptureCreationError> {
        let capture = create(backend).await?;
        Ok(Self {
            capture,
            id_map: Default::default(),
            pending: Default::default(),
            position_map: Default::default(),
            pressed_keys: HashSet::new(),
        })
    }

    /// check whether the given keys are pressed
    pub fn keys_pressed(&self, keys: &[scancode::Linux]) -> bool {
        keys.iter().all(|k| self.pressed_keys.contains(k))
    }

    fn update_pressed_keys(&mut self, key: u32, state: u8) {
        if let Ok(scancode) = scancode::Linux::try_from(key) {
            log::debug!("key: {key}, state: {state}, scancode: {scancode:?}");
            match state {
                1 => self.pressed_keys.insert(scancode),
                _ => self.pressed_keys.remove(&scancode),
            };
        }
    }
}

impl Stream for InputCapture {
    type Item = Result<(CaptureHandle, CaptureEvent), CaptureError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        // Events for positions whose capture was destroyed are dropped and the
        // backend is polled again in this same call. Returning `Pending` after
        // consuming such an event would leave the stream parked with no waker
        // registered, stalling the capture task forever.
        for _ in 0..MAX_EVENTS_PER_POLL {
            if let Some(e) = self.pending.pop_front() {
                return Poll::Ready(Some(Ok(e)));
            }

            // no event pending in the backend: register the waker and suspend
            let event = ready!(self.capture.poll_next_unpin(cx));

            // stream closed
            let event = match event {
                Some(e) => e,
                None => return Poll::Ready(None),
            };

            // error occurred
            let (pos, event) = match event {
                Ok(e) => e,
                Err(e) => return Poll::Ready(Some(Err(e))),
            };

            // handle key presses
            if let CaptureEvent::Input(Event::Keyboard(KeyboardEvent::Key { key, state, .. })) =
                event
            {
                self.update_pressed_keys(key, state);
            }

            let len = self
                .position_map
                .get(&pos)
                .map(|ids| ids.len())
                .unwrap_or(0);

            match len {
                // no handle registered at this position (anymore): drop the
                // event and keep going instead of suspending
                0 => continue,
                1 => {
                    return Poll::Ready(Some(Ok((
                        self.position_map.get(&pos).expect("no id")[0],
                        event,
                    ))));
                }
                _ => {
                    let mut position_map = HashMap::new();
                    swap(&mut self.position_map, &mut position_map);
                    {
                        for &id in position_map.get(&pos).expect("position") {
                            self.pending.push_back((id, event));
                        }
                    }
                    swap(&mut self.position_map, &mut position_map);

                    return Poll::Ready(Some(Ok(self.pending.pop_front().expect("event"))));
                }
            }
        }

        // yield to the executor and get polled again
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

#[async_trait]
trait Capture: Stream<Item = Result<(Position, CaptureEvent), CaptureError>> + Unpin {
    /// create a new client with the given id
    async fn create(&mut self, pos: Position) -> Result<(), CaptureError>;

    /// destroy the client with the given id, if it exists
    async fn destroy(&mut self, pos: Position) -> Result<(), CaptureError>;

    /// release mouse
    async fn release(&mut self) -> Result<(), CaptureError>;

    /// destroy the input capture
    async fn terminate(&mut self) -> Result<(), CaptureError>;
}

async fn create_backend(
    backend: Backend,
) -> Result<
    Box<dyn Capture<Item = Result<(Position, CaptureEvent), CaptureError>>>,
    CaptureCreationError,
> {
    match backend {
        #[cfg(libei)]
        Backend::InputCapturePortal => Ok(Box::new(libei::LibeiInputCapture::new().await?)),
        #[cfg(layer_shell)]
        Backend::LayerShell => Ok(Box::new(layer_shell::LayerShellInputCapture::new()?)),
        #[cfg(x11)]
        Backend::X11 => Ok(Box::new(x11::X11InputCapture::new()?)),
        #[cfg(windows)]
        Backend::Windows => Ok(Box::new(windows::WindowsInputCapture::new())),
        #[cfg(target_os = "macos")]
        Backend::MacOs => Ok(Box::new(macos::MacOSInputCapture::new().await?)),
        Backend::Dummy => Ok(Box::new(dummy::DummyInputCapture::new())),
    }
}

async fn create(
    backend: Option<Backend>,
) -> Result<
    Box<dyn Capture<Item = Result<(Position, CaptureEvent), CaptureError>>>,
    CaptureCreationError,
> {
    if let Some(backend) = backend {
        let b = create_backend(backend).await;
        if b.is_ok() {
            log::info!("using capture backend: {backend}");
        }
        return b;
    }

    for backend in [
        #[cfg(libei)]
        Backend::InputCapturePortal,
        #[cfg(layer_shell)]
        Backend::LayerShell,
        #[cfg(x11)]
        Backend::X11,
        #[cfg(windows)]
        Backend::Windows,
        #[cfg(target_os = "macos")]
        Backend::MacOs,
    ] {
        match create_backend(backend).await {
            Ok(b) => {
                log::info!("using capture backend: {backend}");
                return Ok(b);
            }
            Err(e) if e.cancelled_by_user() => return Err(e),
            Err(e) => log::warn!("{backend} input capture backend unavailable: {e}"),
        }
    }
    Err(CaptureCreationError::NoAvailableBackend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Wake, Waker};

    /// backend replaying a fixed script of events, then ending the stream
    struct ScriptedCapture {
        events: VecDeque<Result<(Position, CaptureEvent), CaptureError>>,
    }

    impl ScriptedCapture {
        fn new(events: Vec<(Position, CaptureEvent)>) -> Self {
            Self {
                events: events.into_iter().map(Ok).collect(),
            }
        }
    }

    #[async_trait]
    impl Capture for ScriptedCapture {
        async fn create(&mut self, _pos: Position) -> Result<(), CaptureError> {
            Ok(())
        }
        async fn destroy(&mut self, _pos: Position) -> Result<(), CaptureError> {
            Ok(())
        }
        async fn release(&mut self) -> Result<(), CaptureError> {
            Ok(())
        }
        async fn terminate(&mut self) -> Result<(), CaptureError> {
            Ok(())
        }
    }

    impl Stream for ScriptedCapture {
        type Item = Result<(Position, CaptureEvent), CaptureError>;

        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            match self.events.pop_front() {
                Some(e) => Poll::Ready(Some(e)),
                None => Poll::Ready(None),
            }
        }
    }

    fn capture_with(events: Vec<(Position, CaptureEvent)>) -> InputCapture {
        InputCapture {
            capture: Box::new(ScriptedCapture::new(events)),
            pressed_keys: HashSet::new(),
            position_map: Default::default(),
            id_map: Default::default(),
            pending: Default::default(),
        }
    }

    #[derive(Default)]
    struct CountingWaker(AtomicBool);

    impl Wake for CountingWaker {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// An event for a position with no handle must not park the stream: it is
    /// dropped and the following event is delivered in the same poll.
    #[test]
    fn event_without_handle_does_not_stall_the_stream() {
        let mut capture = capture_with(vec![
            (Position::Right, CaptureEvent::Begin),
            (Position::Right, CaptureEvent::Begin),
            (Position::Left, CaptureEvent::Begin),
        ]);
        capture.id_map.insert(7, Position::Left);
        capture.position_map.insert(Position::Left, vec![7]);

        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let polled = Stream::poll_next(std::pin::Pin::new(&mut capture), &mut cx);

        assert!(
            matches!(polled, Poll::Ready(Some(Ok((7, CaptureEvent::Begin))))),
            "event for a handle-less position must be skipped, not suspend the stream"
        );
    }

    /// A backend that keeps supplying handle-less events must eventually yield
    /// the executor, and must arrange to be polled again.
    #[test]
    fn handle_less_event_flood_yields_but_stays_wakeable() {
        let events = (0..MAX_EVENTS_PER_POLL + 10)
            .map(|_| (Position::Right, CaptureEvent::Begin))
            .collect();
        let mut capture = capture_with(events);

        let waker = Arc::new(CountingWaker::default());
        let waker_handle = Waker::from(waker.clone());
        let mut cx = Context::from_waker(&waker_handle);
        let polled = Stream::poll_next(std::pin::Pin::new(&mut capture), &mut cx);

        assert!(
            matches!(polled, Poll::Pending),
            "poll must yield to the executor"
        );
        assert!(
            waker.0.load(Ordering::SeqCst),
            "the stream must wake itself so it is polled again"
        );
    }

    /// The delivered event for a registered handle is still routed to it.
    #[test]
    fn event_with_handle_is_delivered() {
        let mut capture = capture_with(vec![(Position::Bottom, CaptureEvent::Begin)]);
        capture.id_map.insert(3, Position::Bottom);
        capture.position_map.insert(Position::Bottom, vec![3]);

        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let polled = Stream::poll_next(std::pin::Pin::new(&mut capture), &mut cx);

        assert!(matches!(
            polled,
            Poll::Ready(Some(Ok((3, CaptureEvent::Begin))))
        ));
    }
}
