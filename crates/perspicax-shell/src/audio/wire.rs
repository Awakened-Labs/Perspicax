//! The socket to the server, as messages: commands written, and what comes
//! back read whole, each reply matched to the command it answers.
//!
//! The socket never blocks. What cannot be written yet waits in an outbox
//! for the socket to take it, and what has come in waits in an inbox until
//! a whole message is there, its length told by the descriptor before it.
//! A message the protocol crate cannot read, a command newer than it knows,
//! is skipped whole and the rest read on: the server is not wrong to send
//! it, and the shell has no use for it.

use std::{
    collections::HashMap,
    io::{self, Cursor, Read as _, Write as _},
    os::unix::net::UnixStream,
};

use pulseaudio::protocol::{
    self, AuthReply, CardInfo, CardInfoList, Command, CreateRecordStreamReply, DESCRIPTOR_SIZE,
    MAX_VERSION, ProtocolError, PulseError, ServerInfo, SetClientNameReply, SinkInfo, SinkInfoList,
    TagStructReader,
};

/// The longest message read: past this it is not a message.
const MESSAGE_MOST: usize = 16 << 20;

/// What the server's reply to a command will be, and so how to read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Expect {
    /// The protocol version both ends speak.
    Auth,
    /// The shell's number among the server's clients.
    Named,
    /// Nothing but that it was done.
    Done,
    /// The server's defaults.
    Server,
    /// Every sink.
    Sinks,
    /// The sink of this index.
    Sink(u32),
    /// Every card.
    Cards,
    /// The card of this index.
    Card(u32),
    /// The record stream of this number made: its channel.
    Recording(u64),
}

/// A reply, read as its command expected.
#[derive(Debug)]
pub(super) enum Reply {
    Auth(AuthReply),
    Named,
    Done,
    Server(Box<ServerInfo>),
    Sinks(SinkInfoList),
    Sink(Box<SinkInfo>),
    Cards(CardInfoList),
    Card(Box<CardInfo>),
    Recording(Box<CreateRecordStreamReply>),
}

/// A message from the server.
#[derive(Debug)]
pub(super) enum Incoming {
    /// The reply to a command.
    Reply(Expect, Reply),
    /// A command refused, and why.
    Refused(Expect, PulseError),
    /// A command of the server's own: a subscription's event, most often.
    Event(Box<Command>),
    /// What a stream of this channel recorded.
    Data(u32, Vec<u8>),
}

/// The connection to the server.
pub(super) struct Wire {
    stream: UnixStream,
    /// The protocol version both ends speak: the newest the crate knows
    /// until the server has said.
    pub(super) version: u16,
    inbox: Vec<u8>,
    outbox: Vec<u8>,
    /// The next command's sequence number.
    next: u32,
    /// The command each reply awaited answers, by sequence number.
    awaited: HashMap<u32, Expect>,
}

impl Wire {
    /// `stream`, made not to block.
    pub(super) fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            version: MAX_VERSION,
            inbox: Vec::new(),
            outbox: Vec::new(),
            next: 0,
            awaited: HashMap::new(),
        })
    }

    /// Send `command`, whose reply is `expect`ed.
    pub(super) fn send(&mut self, command: &Command, expect: Expect) -> io::Result<()> {
        let seq = self.next;
        // `u32::MAX` is the server's own commands' number.
        self.next = (self.next + 1) % u32::MAX;
        protocol::write_command_message(&mut self.outbox, seq, command, self.version)
            .map_err(broken)?;
        self.awaited.insert(seq, expect);
        self.flush()
    }

    /// Write what the socket will take of the outbox.
    pub(super) fn flush(&mut self) -> io::Result<()> {
        while !self.outbox.is_empty() {
            match (&self.stream).write(&self.outbox) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(written) => {
                    self.outbox.drain(..written);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Read what the server sent, and every whole message it makes.
    pub(super) fn pump(&mut self) -> io::Result<Vec<Incoming>> {
        let mut buffer = [0; 64 * 1024];
        loop {
            match (&self.stream).read(&mut buffer) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the server closed the connection",
                    ));
                }
                Ok(read) => self.inbox.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        let mut incoming = Vec::new();
        let mut at = 0;
        while self.inbox.len() - at >= DESCRIPTOR_SIZE {
            let descriptor = protocol::read_descriptor(&mut &self.inbox[at..]).map_err(broken)?;
            let length = descriptor.length as usize;
            if length > MESSAGE_MOST {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("a message {length} bytes long"),
                ));
            }
            let start = at + DESCRIPTOR_SIZE;
            if self.inbox.len() - start < length {
                break;
            }
            at = start + length;
            let message = self.inbox[start..at].to_vec();
            if descriptor.channel == u32::MAX {
                incoming.extend(self.read(&message));
            } else {
                incoming.push(Incoming::Data(descriptor.channel, message));
            }
        }
        self.inbox.drain(..at);
        Ok(incoming)
    }

    /// The message `message` holds, if it is one to act on.
    fn read(&mut self, message: &[u8]) -> Option<Incoming> {
        let mut cursor = Cursor::new(message);
        match Command::read_tag_prefixed(&mut cursor, self.version) {
            Ok((seq, Command::Reply)) => {
                let expect = self.awaited.remove(&seq)?;
                let mut reader = TagStructReader::new(&mut cursor, self.version);
                match expect.read(&mut reader) {
                    Ok(reply) => Some(Incoming::Reply(expect, reply)),
                    Err(error) => {
                        tracing::debug!("a reply to {expect:?} could not be read: {error}");
                        None
                    }
                }
            }
            Ok((seq, Command::Error(error))) => self
                .awaited
                .remove(&seq)
                .map(|expect| Incoming::Refused(expect, error)),
            Ok((_, command)) => Some(Incoming::Event(Box::new(command))),
            Err(error) => {
                tracing::debug!("a message the shell does not read was skipped: {error}");
                None
            }
        }
    }
}

impl Expect {
    /// Its reply, from `reader`.
    fn read(self, reader: &mut TagStructReader<'_>) -> Result<Reply, ProtocolError> {
        Ok(match self {
            Self::Auth => Reply::Auth(reader.read()?),
            Self::Named => {
                reader.read::<SetClientNameReply>()?;
                Reply::Named
            }
            Self::Done => Reply::Done,
            Self::Server => Reply::Server(Box::new(reader.read()?)),
            Self::Sinks => Reply::Sinks(reader.read()?),
            Self::Sink(_) => Reply::Sink(Box::new(reader.read()?)),
            Self::Cards => Reply::Cards(reader.read()?),
            Self::Card(_) => Reply::Card(Box::new(reader.read()?)),
            Self::Recording(_) => Reply::Recording(Box::new(reader.read()?)),
        })
    }
}

/// A protocol error, as the connection breaking.
fn broken(error: ProtocolError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
