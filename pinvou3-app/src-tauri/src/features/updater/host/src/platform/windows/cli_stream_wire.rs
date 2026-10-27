//! Bounded binary stream framing for an already authenticated connection.
//! Connection authentication and process authority are deliberately external.
use std::{
    io::{self, Read, Write},
    sync::mpsc::{self, Receiver, SyncSender},
};

const MAX_DATA: usize = 16 * 1024;
const CAPACITY: usize = 4;

pub(super) struct Data(Vec<u8>);
impl Data {
    pub(super) fn new(bytes: Vec<u8>) -> io::Result<Self> {
        if bytes.is_empty() || bytes.len() > MAX_DATA {
            return Err(invalid());
        }
        Ok(Self(bytes))
    }
    pub(super) fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

pub(super) enum Frame {
    Input(Data),
    InputEof,
    /// Frontend confirms receipt of the terminal output frame; it is not a
    /// worker input message or an authorization token.
    OutputAck([u8; 64]),
    Stdout(Data),
    StdoutEof,
    Stderr(Data),
    StderrEof,
    Exit(i32),
    /// Fresh terminal receipt challenge; not a writer authorization.
    OutputChallenge([u8; 64]),
    /// A bounded protocol error category, never a free-form error message.
    Error(u8),
}

#[derive(Clone, Copy)]
pub(super) enum Direction {
    Input,
    FrontendInput,
    FrontendOutput,
    Output,
}

/// Backpressure applies before another frame can be queued; disconnection
/// wakes blocked sends and receives. Each frame owns at most MAX_DATA bytes.
pub(super) fn channel() -> (SyncSender<Frame>, Receiver<Frame>) {
    mpsc::sync_channel(CAPACITY)
}

struct State {
    direction: Direction,
    input_eof: bool,
    output_ack: bool,
    challenge: bool,
    stdout_eof: bool,
    stderr_eof: bool,
    exit: bool,
    failed: bool,
}
impl State {
    fn new(direction: Direction) -> Self {
        Self {
            direction,
            input_eof: false,
            output_ack: false,
            challenge: false,
            stdout_eof: false,
            stderr_eof: false,
            exit: false,
            failed: false,
        }
    }
    fn complete(&self) -> bool {
        self.failed
            || match self.direction {
                Direction::Input => self.input_eof,
                Direction::FrontendInput => self.output_ack,
                Direction::FrontendOutput => {
                    self.stdout_eof && self.stderr_eof && self.exit && self.challenge
                }
                Direction::Output => self.stdout_eof && self.stderr_eof && self.exit,
            }
    }
    fn accept(&mut self, frame: &Frame) -> io::Result<()> {
        if self.complete() {
            return Err(invalid());
        }
        match (self.direction, frame) {
            (Direction::Input, Frame::Input(_)) if !self.input_eof => (),
            (Direction::Input, Frame::InputEof) if !self.input_eof => self.input_eof = true,
            (Direction::FrontendInput, Frame::Input(_)) if !self.input_eof => (),
            (Direction::FrontendInput, Frame::InputEof) if !self.input_eof => self.input_eof = true,
            (Direction::FrontendInput, Frame::OutputAck(_)) if !self.output_ack => {
                self.output_ack = true
            }
            (Direction::Output, Frame::Stdout(_)) if !self.stdout_eof => (),
            (Direction::Output, Frame::Stderr(_)) if !self.stderr_eof => (),
            (Direction::Output, Frame::StdoutEof) if !self.stdout_eof => self.stdout_eof = true,
            (Direction::Output, Frame::StderrEof) if !self.stderr_eof => self.stderr_eof = true,
            (Direction::Output, Frame::Exit(_)) if !self.exit => self.exit = true,
            (Direction::FrontendOutput, Frame::Stdout(_)) if !self.stdout_eof => (),
            (Direction::FrontendOutput, Frame::Stderr(_)) if !self.stderr_eof => (),
            (Direction::FrontendOutput, Frame::StdoutEof) if !self.stdout_eof => {
                self.stdout_eof = true
            }
            (Direction::FrontendOutput, Frame::StderrEof) if !self.stderr_eof => {
                self.stderr_eof = true
            }
            (Direction::FrontendOutput, Frame::OutputChallenge(_))
                if !self.challenge && self.stdout_eof && self.stderr_eof =>
            {
                self.challenge = true
            }
            (Direction::FrontendOutput, Frame::Exit(_)) if !self.exit && self.challenge => {
                self.exit = true
            }
            (_, Frame::Error(code)) if (1..=4).contains(code) => self.failed = true,
            _ => return Err(invalid()),
        }
        Ok(())
    }
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid stream frame")
}

/// Each direction is separately sequenced. A partial I/O error poisons the
/// encoder/decoder: reconnecting cannot resume from an ambiguous byte offset.
pub(super) struct Encoder<W> {
    writer: W,
    state: State,
    poisoned: bool,
}
impl<W: Write> Encoder<W> {
    pub(super) fn new(writer: W, direction: Direction) -> Self {
        Self {
            writer,
            state: State::new(direction),
            poisoned: false,
        }
    }
    pub(super) fn send(&mut self, frame: Frame) -> io::Result<()> {
        if self.poisoned {
            return Err(invalid());
        }
        self.state.accept(&frame)?;
        let (kind, payload): (u8, &[u8]);
        let exit;
        let code;
        match &frame {
            Frame::Input(x) => {
                kind = 1;
                payload = &x.0;
            }
            Frame::InputEof => {
                kind = 2;
                payload = &[];
            }
            Frame::OutputAck(value) => {
                kind = 9;
                payload = value;
            }
            Frame::OutputChallenge(value) => {
                kind = 10;
                payload = value;
            }
            Frame::Stdout(x) => {
                kind = 3;
                payload = &x.0;
            }
            Frame::StdoutEof => {
                kind = 4;
                payload = &[];
            }
            Frame::Stderr(x) => {
                kind = 5;
                payload = &x.0;
            }
            Frame::StderrEof => {
                kind = 6;
                payload = &[];
            }
            Frame::Exit(x) => {
                kind = 7;
                exit = x.to_le_bytes();
                payload = &exit;
            }
            Frame::Error(x) => {
                kind = 8;
                code = [*x];
                payload = &code;
            }
        }
        let mut header = [0u8; 5];
        header[0] = kind;
        header[1..].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        let result = self
            .writer
            .write_all(&header)
            .and_then(|_| self.writer.write_all(payload))
            .and_then(|_| self.writer.flush());
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

pub(super) struct Decoder<R> {
    reader: R,
    state: State,
    poisoned: bool,
}
impl<R: Read> Decoder<R> {
    pub(super) fn new(reader: R, direction: Direction) -> Self {
        Self {
            reader,
            state: State::new(direction),
            poisoned: false,
        }
    }
    pub(super) fn receive(&mut self) -> io::Result<Option<Frame>> {
        if self.poisoned {
            return Err(invalid());
        }
        if self.state.complete() {
            return Ok(None);
        }
        let result = self.read_frame();
        if result.is_err() {
            self.poisoned = true;
        }
        result.map(Some)
    }
    fn read_frame(&mut self) -> io::Result<Frame> {
        let mut header = [0u8; 5];
        self.reader.read_exact(&mut header)?;
        let len = u32::from_le_bytes(header[1..5].try_into().map_err(|_| invalid())?) as usize;
        let valid = match header[0] {
            1 | 3 | 5 => (1..=MAX_DATA).contains(&len),
            2 | 4 | 6 => len == 0,
            9 | 10 => len == 64,
            7 => len == 4,
            8 => len == 1,
            _ => false,
        };
        if !valid {
            return Err(invalid());
        }
        let mut bytes = vec![0; len];
        self.reader.read_exact(&mut bytes)?;
        let frame = match header[0] {
            1 => Frame::Input(Data(bytes)),
            2 => Frame::InputEof,
            9 => Frame::OutputAck(bytes.try_into().map_err(|_| invalid())?),
            10 => Frame::OutputChallenge(bytes.try_into().map_err(|_| invalid())?),
            3 => Frame::Stdout(Data(bytes)),
            4 => Frame::StdoutEof,
            5 => Frame::Stderr(Data(bytes)),
            6 => Frame::StderrEof,
            7 => Frame::Exit(i32::from_le_bytes(bytes.try_into().map_err(|_| invalid())?)),
            8 => Frame::Error(bytes[0]),
            _ => return Err(invalid()),
        };
        self.state.accept(&frame)?;
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frontend_terminal_challenge_and_ack_are_separate_from_input_eof() {
        let challenge = [b'a'; 64];
        let mut out = Encoder::new(Vec::new(), Direction::FrontendOutput);
        assert!(out.send(Frame::OutputChallenge(challenge)).is_err());
        out.send(Frame::StdoutEof).unwrap();
        out.send(Frame::StderrEof).unwrap();
        out.send(Frame::OutputChallenge(challenge)).unwrap();
        out.send(Frame::Exit(259)).unwrap();
        let mut decoder = Decoder::new(&out.writer[..], Direction::FrontendOutput);
        assert!(matches!(decoder.receive().unwrap(), Some(Frame::StdoutEof)));
        assert!(matches!(decoder.receive().unwrap(), Some(Frame::StderrEof)));
        assert!(
            matches!(decoder.receive().unwrap(), Some(Frame::OutputChallenge(value)) if value == challenge)
        );
        assert!(matches!(decoder.receive().unwrap(), Some(Frame::Exit(259))));
        assert!(decoder.receive().unwrap().is_none());
        let mut input = Encoder::new(Vec::new(), Direction::FrontendInput);
        input.send(Frame::InputEof).unwrap();
        input.send(Frame::OutputAck(challenge)).unwrap();
        assert!(input.send(Frame::OutputAck(challenge)).is_err());
    }
    #[test]
    fn exit_before_eof_still_drains_both_outputs() {
        let mut enc = Encoder::new(Vec::new(), Direction::Output);
        enc.send(Frame::Exit(-5)).unwrap();
        enc.send(Frame::Stdout(Data::new(vec![1, 2]).unwrap()))
            .unwrap();
        enc.send(Frame::StdoutEof).unwrap();
        enc.send(Frame::Stderr(Data::new(vec![3]).unwrap()))
            .unwrap();
        enc.send(Frame::StderrEof).unwrap();
        let mut dec = Decoder::new(&enc.writer[..], Direction::Output);
        assert!(matches!(dec.receive().unwrap(), Some(Frame::Exit(-5))));
        assert!(matches!(dec.receive().unwrap(), Some(Frame::Stdout(x)) if x.as_slice() == [1, 2]));
        assert!(matches!(dec.receive().unwrap(), Some(Frame::StdoutEof)));
        assert!(matches!(dec.receive().unwrap(), Some(Frame::Stderr(x)) if x.as_slice() == [3]));
        assert!(matches!(dec.receive().unwrap(), Some(Frame::StderrEof)));
        assert!(dec.receive().unwrap().is_none());
    }
    #[test]
    fn reject_duplicate_eof_and_data_after_eof() {
        let mut enc = Encoder::new(Vec::new(), Direction::Output);
        enc.send(Frame::StdoutEof).unwrap();
        assert!(enc.send(Frame::StdoutEof).is_err());
        assert!(
            enc.send(Frame::Stdout(Data::new(vec![1]).unwrap()))
                .is_err()
        );
        enc.send(Frame::StderrEof).unwrap();
        enc.send(Frame::Exit(0)).unwrap();
    }
    #[test]
    fn oversized_header_is_rejected_before_reading_body() {
        let mut dec = Decoder::new(&[1, 1, 64, 0, 0][..], Direction::Input);
        assert_eq!(
            dec.receive().err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(dec.receive().is_err());
    }
    #[test]
    fn truncated_input_is_not_eof() {
        let mut dec = Decoder::new(&[1, 2, 0, 0, 0, 42][..], Direction::Input);
        assert_eq!(
            dec.receive().err().unwrap().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
    #[test]
    fn input_direction_rejects_output_and_error_payload_is_bounded() {
        let mut enc = Encoder::new(Vec::new(), Direction::Input);
        assert!(enc.send(Frame::Exit(0)).is_err());
        assert!(enc.send(Frame::Error(0)).is_err());
        enc.send(Frame::InputEof).unwrap();
        assert!(enc.send(Frame::InputEof).is_err());
    }
    #[test]
    fn dropped_receiver_wakes_backpressured_sender() {
        let (tx, rx) = channel();
        for _ in 0..CAPACITY {
            tx.send(Frame::InputEof).unwrap();
        }
        let thread = std::thread::spawn(move || tx.send(Frame::InputEof).is_err());
        drop(rx);
        assert!(thread.join().unwrap());
    }
}
