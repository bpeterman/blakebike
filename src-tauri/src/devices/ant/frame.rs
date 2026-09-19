//! ANT serial framing: sync, payload length, message id, payload and XOR checksum.

pub const SYNC: u8 = 0xA4;
const MAX_PAYLOAD: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn encode(&self) -> Vec<u8> {
        assert!(self.payload.len() <= MAX_PAYLOAD);
        let mut bytes = Vec::with_capacity(self.payload.len() + 4);
        bytes.push(SYNC);
        bytes.push(self.payload.len() as u8);
        bytes.push(self.id);
        bytes.extend_from_slice(&self.payload);
        bytes.push(bytes.iter().fold(0, |checksum, byte| checksum ^ byte));
        bytes
    }
}

#[derive(Default)]
pub struct Parser {
    buffer: Vec<u8>,
}

impl Parser {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Return the next valid frame, discarding garbage and checksum failures.
    pub fn next(&mut self) -> Option<Frame> {
        loop {
            let sync = self.buffer.iter().position(|byte| *byte == SYNC)?;
            if sync > 0 {
                self.buffer.drain(..sync);
            }
            if self.buffer.len() < 4 {
                return None;
            }
            let length = usize::from(self.buffer[1]);
            let frame_length = length + 4;
            if self.buffer.len() < frame_length {
                return None;
            }
            let checksum = self.buffer[..frame_length]
                .iter()
                .fold(0, |checksum, byte| checksum ^ byte);
            if checksum != 0 {
                // Drop only this sync byte so a later valid frame can be found.
                self.buffer.remove(0);
                continue;
            }
            let id = self.buffer[2];
            let payload = self.buffer[3..3 + length].to_vec();
            self.buffer.drain(..frame_length);
            return Some(Frame { id, payload });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_split_frame() {
        let encoded = Frame {
            id: 0x4E,
            payload: vec![0, 1, 2, 3, 4, 5, 6, 7, 140],
        }
        .encode();
        let mut parser = Parser::default();
        parser.push(&encoded[..3]);
        assert_eq!(parser.next(), None);
        parser.push(&encoded[3..]);
        assert_eq!(
            parser.next(),
            Some(Frame {
                id: 0x4E,
                payload: vec![0, 1, 2, 3, 4, 5, 6, 7, 140]
            })
        );
    }

    #[test]
    fn reads_multiple_frames_and_resynchronizes() {
        let first = Frame {
            id: 1,
            payload: vec![2],
        }
        .encode();
        let second = Frame {
            id: 3,
            payload: vec![4, 5],
        }
        .encode();
        let mut damaged = first.clone();
        *damaged.last_mut().unwrap() ^= 0xff;
        let mut parser = Parser::default();
        parser.push(&[9, 8, 7]);
        parser.push(&damaged);
        parser.push(&first);
        parser.push(&second);
        assert_eq!(parser.next().unwrap().id, 1);
        assert_eq!(parser.next().unwrap().id, 3);
        assert_eq!(parser.next(), None);
    }
}
