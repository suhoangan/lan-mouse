use input_event::{Event as InputEvent, KeyboardEvent, PointerEvent};
use num_enum::{IntoPrimitive, TryFromPrimitive, TryFromPrimitiveError};
use paste::paste;
use std::{
    fmt::{Debug, Display, Formatter},
    mem::size_of,
};
use thiserror::Error;

/// maximum size of clipboard text chunk in one datagram
pub const MAX_CLIPBOARD_CHUNK: usize = 900;
/// defines the maximum size an encoded event can take up
pub const MAX_EVENT_SIZE: usize = 1 + 2 + 2 + 2 + MAX_CLIPBOARD_CHUNK;

/// error type for protocol violations
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// event type does not exist
    #[error("invalid event id: `{0}`")]
    InvalidEventId(#[from] TryFromPrimitiveError<EventType>),
    /// position type does not exist
    #[error("invalid event id: `{0}`")]
    InvalidPosition(#[from] TryFromPrimitiveError<Position>),
}

/// Position of a client
#[derive(Clone, Copy, Debug, TryFromPrimitive, IntoPrimitive)]
#[repr(u8)]
pub enum Position {
    Left,
    Right,
    Top,
    Bottom,
}

impl Display for Position {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let pos = match self {
            Position::Left => "left",
            Position::Right => "right",
            Position::Top => "top",
            Position::Bottom => "bottom",
        };
        write!(f, "{pos}")
    }
}

/// main lan-mouse protocol event type
#[derive(Clone, Copy, Debug)]
pub enum ProtoEvent {
    /// notify a client that the cursor entered its region at the given position
    /// [`ProtoEvent::Ack`] with the same serial is used for synchronization between devices
    Enter(Position),
    /// notify a client that the cursor left its region
    /// [`ProtoEvent::Ack`] with the same serial is used for synchronization between devices
    Leave(u32),
    /// acknowledge of an [`ProtoEvent::Enter`] or [`ProtoEvent::Leave`] event
    Ack(u32),
    /// Input event
    Input(InputEvent),
    /// Ping event for tracking unresponsive clients.
    /// A client has to respond with [`ProtoEvent::Pong`].
    Ping,
    /// Response to [`ProtoEvent::Ping`], true if emulation is enabled / available
    Pong(bool),
    /// Build identification for the sending peer. Sent by the
    /// connect side once after the connection authenticates, and
    /// echoed back by the listen side in reply, so each end can
    /// display the peer's build hash and warn (soft) on mismatch.
    /// `commit` is the 8-byte ASCII short commit hash from
    /// `shadow_rs`'s `SHORT_COMMIT`. Old peers that don't
    /// recognize the event type silently skip it per the
    /// forward-compat handling in the receive loop.
    Hello { commit: [u8; 8] },
    /// Clipboard text chunk
    Clipboard {
        seq: u16,
        total: u16,
        len: u16,
        data: [u8; MAX_CLIPBOARD_CHUNK],
    },
}

impl Display for ProtoEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtoEvent::Enter(s) => write!(f, "Enter({s})"),
            ProtoEvent::Leave(s) => write!(f, "Leave({s})"),
            ProtoEvent::Ack(s) => write!(f, "Ack({s})"),
            ProtoEvent::Input(e) => write!(f, "{e}"),
            ProtoEvent::Ping => write!(f, "ping"),
            ProtoEvent::Pong(alive) => {
                write!(
                    f,
                    "pong: {}",
                    if *alive { "alive" } else { "not available" }
                )
            }
            ProtoEvent::Hello { commit } => {
                let s = std::str::from_utf8(commit).unwrap_or("????????");
                write!(f, "Hello({s})")
            }
            ProtoEvent::Clipboard {
                seq, total, len, ..
            } => {
                write!(f, "Clipboard({seq}/{total}, len={len})")
            }
        }
    }
}

#[derive(TryFromPrimitive, IntoPrimitive)]
#[repr(u8)]
pub enum EventType {
    PointerMotion,
    PointerButton,
    PointerAxis,
    PointerAxisValue120,
    KeyboardKey,
    KeyboardModifiers,
    Ping,
    Pong,
    Enter,
    Leave,
    Ack,
    Hello,
    Clipboard,
}

impl ProtoEvent {
    fn event_type(&self) -> EventType {
        match self {
            ProtoEvent::Input(e) => match e {
                InputEvent::Pointer(p) => match p {
                    PointerEvent::Motion { .. } => EventType::PointerMotion,
                    PointerEvent::Button { .. } => EventType::PointerButton,
                    PointerEvent::Axis { .. } => EventType::PointerAxis,
                    PointerEvent::AxisDiscrete120 { .. } => EventType::PointerAxisValue120,
                },
                InputEvent::Keyboard(k) => match k {
                    KeyboardEvent::Key { .. } => EventType::KeyboardKey,
                    KeyboardEvent::Modifiers { .. } => EventType::KeyboardModifiers,
                },
            },
            ProtoEvent::Ping => EventType::Ping,
            ProtoEvent::Pong(_) => EventType::Pong,
            ProtoEvent::Enter(_) => EventType::Enter,
            ProtoEvent::Leave(_) => EventType::Leave,
            ProtoEvent::Ack(_) => EventType::Ack,
            ProtoEvent::Hello { .. } => EventType::Hello,
            ProtoEvent::Clipboard { .. } => EventType::Clipboard,
        }
    }
}

impl TryFrom<[u8; MAX_EVENT_SIZE]> for ProtoEvent {
    type Error = ProtocolError;

    fn try_from(buf: [u8; MAX_EVENT_SIZE]) -> Result<Self, Self::Error> {
        let mut buf = &buf[..];
        let event_type = decode_u8(&mut buf)?;
        match EventType::try_from(event_type)? {
            EventType::PointerMotion => {
                Ok(Self::Input(InputEvent::Pointer(PointerEvent::Motion {
                    time: decode_u32(&mut buf)?,
                    dx: decode_f64(&mut buf)?,
                    dy: decode_f64(&mut buf)?,
                })))
            }
            EventType::PointerButton => {
                Ok(Self::Input(InputEvent::Pointer(PointerEvent::Button {
                    time: decode_u32(&mut buf)?,
                    button: decode_u32(&mut buf)?,
                    state: decode_u32(&mut buf)?,
                })))
            }
            EventType::PointerAxis => Ok(Self::Input(InputEvent::Pointer(PointerEvent::Axis {
                time: decode_u32(&mut buf)?,
                axis: decode_u8(&mut buf)?,
                value: decode_f64(&mut buf)?,
            }))),
            EventType::PointerAxisValue120 => Ok(Self::Input(InputEvent::Pointer(
                PointerEvent::AxisDiscrete120 {
                    axis: decode_u8(&mut buf)?,
                    value: decode_i32(&mut buf)?,
                },
            ))),
            EventType::KeyboardKey => Ok(Self::Input(InputEvent::Keyboard(KeyboardEvent::Key {
                time: decode_u32(&mut buf)?,
                key: decode_u32(&mut buf)?,
                state: decode_u8(&mut buf)?,
            }))),
            EventType::KeyboardModifiers => Ok(Self::Input(InputEvent::Keyboard(
                KeyboardEvent::Modifiers {
                    depressed: decode_u32(&mut buf)?,
                    latched: decode_u32(&mut buf)?,
                    locked: decode_u32(&mut buf)?,
                    group: decode_u32(&mut buf)?,
                },
            ))),
            EventType::Ping => Ok(Self::Ping),
            EventType::Pong => Ok(Self::Pong(decode_u8(&mut buf)? != 0)),
            EventType::Enter => Ok(Self::Enter(decode_u8(&mut buf)?.try_into()?)),
            EventType::Leave => Ok(Self::Leave(decode_u32(&mut buf)?)),
            EventType::Ack => Ok(Self::Ack(decode_u32(&mut buf)?)),
            EventType::Hello => {
                let mut commit = [0u8; 8];
                for b in commit.iter_mut() {
                    *b = decode_u8(&mut buf)?;
                }
                Ok(Self::Hello { commit })
            }
            EventType::Clipboard => {
                let seq = decode_u16(&mut buf)?;
                let total = decode_u16(&mut buf)?;
                let len = decode_u16(&mut buf)?;
                let mut data = [0u8; MAX_CLIPBOARD_CHUNK];
                let len_usize = (len as usize).min(MAX_CLIPBOARD_CHUNK);
                for b in data[..len_usize].iter_mut() {
                    *b = decode_u8(&mut buf)?;
                }
                Ok(Self::Clipboard {
                    seq,
                    total,
                    len,
                    data,
                })
            }
        }
    }
}

impl From<ProtoEvent> for ([u8; MAX_EVENT_SIZE], usize) {
    fn from(event: ProtoEvent) -> Self {
        let mut buf = [0u8; MAX_EVENT_SIZE];
        let mut len = 0usize;
        {
            let mut buf = &mut buf[..];
            let buf = &mut buf;
            let len = &mut len;
            encode_u8(buf, len, event.event_type() as u8);
            match event {
                ProtoEvent::Input(event) => match event {
                    InputEvent::Pointer(p) => match p {
                        PointerEvent::Motion { time, dx, dy } => {
                            encode_u32(buf, len, time);
                            encode_f64(buf, len, dx);
                            encode_f64(buf, len, dy);
                        }
                        PointerEvent::Button {
                            time,
                            button,
                            state,
                        } => {
                            encode_u32(buf, len, time);
                            encode_u32(buf, len, button);
                            encode_u32(buf, len, state);
                        }
                        PointerEvent::Axis { time, axis, value } => {
                            encode_u32(buf, len, time);
                            encode_u8(buf, len, axis);
                            encode_f64(buf, len, value);
                        }
                        PointerEvent::AxisDiscrete120 { axis, value } => {
                            encode_u8(buf, len, axis);
                            encode_i32(buf, len, value);
                        }
                    },
                    InputEvent::Keyboard(k) => match k {
                        KeyboardEvent::Key { time, key, state } => {
                            encode_u32(buf, len, time);
                            encode_u32(buf, len, key);
                            encode_u8(buf, len, state);
                        }
                        KeyboardEvent::Modifiers {
                            depressed,
                            latched,
                            locked,
                            group,
                        } => {
                            encode_u32(buf, len, depressed);
                            encode_u32(buf, len, latched);
                            encode_u32(buf, len, locked);
                            encode_u32(buf, len, group);
                        }
                    },
                },
                ProtoEvent::Ping => {}
                ProtoEvent::Pong(alive) => encode_u8(buf, len, alive as u8),
                ProtoEvent::Enter(pos) => encode_u8(buf, len, pos as u8),
                ProtoEvent::Leave(serial) => encode_u32(buf, len, serial),
                ProtoEvent::Ack(serial) => encode_u32(buf, len, serial),
                ProtoEvent::Hello { commit } => {
                    for b in commit.iter() {
                        encode_u8(buf, len, *b);
                    }
                }
                ProtoEvent::Clipboard {
                    seq,
                    total,
                    len: chunk_len,
                    data,
                } => {
                    encode_u16(buf, len, seq);
                    encode_u16(buf, len, total);
                    encode_u16(buf, len, chunk_len);
                    let valid_len = (chunk_len as usize).min(MAX_CLIPBOARD_CHUNK);
                    for b in &data[..valid_len] {
                        encode_u8(buf, len, *b);
                    }
                }
            }
        }
        (buf, len)
    }
}

macro_rules! decode_impl {
    ($t:ty) => {
        paste! {
            fn [<decode_ $t>](data: &mut &[u8]) -> Result<$t, ProtocolError> {
                let (int_bytes, rest) = data.split_at(size_of::<$t>());
                *data = rest;
                Ok($t::from_be_bytes(int_bytes.try_into().unwrap()))
            }
        }
    };
}

decode_impl!(u8);
decode_impl!(u16);
decode_impl!(u32);
decode_impl!(i32);
decode_impl!(f64);

macro_rules! encode_impl {
    ($t:ty) => {
        paste! {
            fn [<encode_ $t>](buf: &mut &mut [u8], amt: &mut usize, n: $t) {
                let src = n.to_be_bytes();
                let data = std::mem::take(buf);
                let (int_bytes, rest) = data.split_at_mut(size_of::<$t>());
                int_bytes.copy_from_slice(&src);
                *amt += size_of::<$t>();
                *buf = rest
            }
        }
    };
}

encode_impl!(u8);
encode_impl!(u16);
encode_impl!(u32);
encode_impl!(i32);
encode_impl!(f64);

impl ProtoEvent {
    pub fn clipboard_chunks(text: &str) -> Vec<ProtoEvent> {
        let bytes = text.as_bytes();
        if bytes.is_empty() {
            return Vec::new();
        }
        let total = ((bytes.len() + MAX_CLIPBOARD_CHUNK - 1) / MAX_CLIPBOARD_CHUNK) as u16;
        let mut events = Vec::with_capacity(total as usize);
        for (i, chunk) in bytes.chunks(MAX_CLIPBOARD_CHUNK).enumerate() {
            let mut data = [0u8; MAX_CLIPBOARD_CHUNK];
            data[..chunk.len()].copy_from_slice(chunk);
            events.push(ProtoEvent::Clipboard {
                seq: i as u16,
                total,
                len: chunk.len() as u16,
                data,
            });
        }
        events
    }
}

#[derive(Default, Debug)]
pub struct ClipboardAssembler {
    chunks: Vec<Option<Vec<u8>>>,
    total: u16,
}

impl ClipboardAssembler {
    pub fn push(
        &mut self,
        seq: u16,
        total: u16,
        len: u16,
        data: &[u8; MAX_CLIPBOARD_CHUNK],
    ) -> Option<String> {
        if total == 0 {
            return None;
        }
        if self.total != total {
            self.total = total;
            self.chunks = vec![None; total as usize];
        }
        let seq_usize = seq as usize;
        if seq_usize < self.chunks.len() {
            let valid_len = (len as usize).min(MAX_CLIPBOARD_CHUNK);
            self.chunks[seq_usize] = Some(data[..valid_len].to_vec());
        }
        if self.chunks.iter().all(|c| c.is_some()) {
            let mut full = Vec::new();
            for c in self.chunks.drain(..) {
                if let Some(chunk) = c {
                    full.extend_from_slice(&chunk);
                }
            }
            self.total = 0;
            String::from_utf8(full).ok()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clipboard_serialization_roundtrip() {
        let text = "Hello from Windows clipboard across LAN!";
        let chunks = ProtoEvent::clipboard_chunks(text);
        assert_eq!(chunks.len(), 1);

        let (buf, len) = <([u8; MAX_EVENT_SIZE], usize)>::from(chunks[0]);
        assert!(len > 0 && len <= MAX_EVENT_SIZE);

        let decoded = ProtoEvent::try_from(buf).expect("decode failed");
        match decoded {
            ProtoEvent::Clipboard {
                seq,
                total,
                len: clen,
                data,
            } => {
                assert_eq!(seq, 0);
                assert_eq!(total, 1);
                assert_eq!(clen as usize, text.len());
                let mut assembler = ClipboardAssembler::default();
                let result = assembler.push(seq, total, clen, &data);
                assert_eq!(result, Some(text.to_string()));
            }
            _ => panic!("wrong event type"),
        }
    }

    #[test]
    fn test_multi_chunk_clipboard_assembly() {
        let large_text = "A".repeat(2500);
        let chunks = ProtoEvent::clipboard_chunks(&large_text);
        assert_eq!(chunks.len(), 3); // 900 + 900 + 700 = 2500

        let mut assembler = ClipboardAssembler::default();
        assert_eq!(
            assembler.push(
                0,
                3,
                900,
                match &chunks[0] {
                    ProtoEvent::Clipboard { data, .. } => data,
                    _ => unreachable!(),
                }
            ),
            None
        );
        assert_eq!(
            assembler.push(
                1,
                3,
                900,
                match &chunks[1] {
                    ProtoEvent::Clipboard { data, .. } => data,
                    _ => unreachable!(),
                }
            ),
            None
        );
        let full = assembler.push(
            2,
            3,
            700,
            match &chunks[2] {
                ProtoEvent::Clipboard { data, .. } => data,
                _ => unreachable!(),
            },
        );
        assert_eq!(full, Some(large_text));
    }
}
