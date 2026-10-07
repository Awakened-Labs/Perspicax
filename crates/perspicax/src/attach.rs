//! `perspicax attach`: an agent's stdin and stdout, joined to a session's agent
//! socket.
//!
//! An MCP client speaks stdio to a process it starts itself, and an agent
//! inside a session cannot start the session it is in. So it starts this,
//! which connects to the socket that session serves (`--mcp-socket`) and copies
//! bytes both ways: `claude mcp add perspicax -- perspicax attach`. Nothing is
//! parsed on the way. The gate and the server are on the other end, and this is
//! a length of wire.
//!
//! Its ends are exact, which a borrowed netcat's are only with the right flags.
//! The client closing stdin is passed on as the end of what it will say, so
//! its conversation ends and the desktop is free for the next one; answers to
//! what it had already asked still come back. The session closing the
//! connection ends this process, so the client sees its server go.

use std::{
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    thread,
};

use anyhow::{Context as _, Result, bail};

use crate::session::AGENT_SOCKET;

/// The socket to attach to: the one named, or else the session's own.
///
/// `var` reads the environment; a function rather than the process's own, so
/// the rule can be checked without changing the test's environment.
///
/// # Errors
///
/// None named, and the environment names none either, saying which of the two
/// ways that happened: outside any session, or in one that serves no socket.
pub fn socket_path(
    named: Option<PathBuf>,
    var: impl Fn(&str) -> Option<String>,
) -> Result<PathBuf> {
    if let Some(path) = named {
        return Ok(path);
    }
    match var(AGENT_SOCKET) {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        Some(_) => bail!(
            "this session serves no agent interface ({AGENT_SOCKET} is empty): \
             start it with --mcp-socket PATH"
        ),
        None => bail!(
            "{AGENT_SOCKET} is not set, so there is no session here to attach to: \
             run this inside a perspicax session started with --mcp-socket PATH, \
             or name the socket"
        ),
    }
}

/// Join this process's stdin and stdout to the socket at `path`, until the
/// session closes it.
///
/// # Errors
///
/// When the socket will not answer, or the wire breaks on this side.
pub fn attach(path: &Path) -> Result<()> {
    let stream = UnixStream::connect(path)
        .with_context(|| format!("could not attach to {}", path.display()))?;
    bridge(stream, io::stdin(), io::stdout())
}

/// Copy `input` into `stream`, and `stream` out to `output`, until the far end
/// closes.
///
/// `input` running out is passed on as a shutdown of the socket's writing half,
/// which the far end reads as this side being done, and then this waits on:
/// answers to what was already sent may still be coming. The far end closing
/// is the end. What goes to `output` is flushed as it arrives, since the
/// client is waiting on each frame.
///
/// # Errors
///
/// When `output` cannot be written, or the socket read.
pub fn bridge(
    mut stream: UnixStream,
    mut input: impl Read + Send + 'static,
    mut output: impl Write,
) -> Result<()> {
    let mut to_session = stream
        .try_clone()
        .context("a second handle on the socket")?;
    thread::spawn(move || {
        // However this ends, the far end hears this side is done. A failure
        // here is the session gone, which the other direction reports.
        let _ = pass_on(&mut input, &mut to_session);
        let _ = to_session.shutdown(Shutdown::Write);
    });
    pass_on(&mut stream, &mut output).context("the agent interface's wire broke")
}

/// Copy `from` to `to` until `from` runs out, flushing each piece.
fn pass_on(from: &mut impl Read, to: &mut impl Write) -> io::Result<()> {
    let mut piece = [0; 64 * 1024];
    loop {
        let read = match from.read(&mut piece) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        to.write_all(&piece[..read])?;
        to.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn the_socket_named_wins_and_otherwise_the_sessions_is_used() {
        let named = socket_path(Some(PathBuf::from("/tmp/elsewhere")), |_| {
            Some("/run/user/1000/perspicax-mcp".to_owned())
        });
        assert_eq!(named.expect("named"), Path::new("/tmp/elsewhere"));

        let the_sessions = socket_path(None, |key| {
            (key == AGENT_SOCKET).then(|| "/run/user/1000/perspicax-mcp".to_owned())
        });
        assert_eq!(
            the_sessions.expect("the session's"),
            Path::new("/run/user/1000/perspicax-mcp")
        );
    }

    #[test]
    fn no_socket_anywhere_says_which_way_there_is_none() {
        let outside = socket_path(None, |_| None).expect_err("no session");
        assert!(outside.to_string().contains("not set"), "{outside}");
        let unserved = socket_path(None, |_| Some(String::new())).expect_err("none served");
        assert!(
            unserved.to_string().contains("serves no agent interface"),
            "{unserved}"
        );
    }

    #[test]
    fn what_is_said_goes_both_ways_and_the_end_of_it_is_passed_on() {
        let (near, far) = UnixStream::pair().expect("a pair of sockets");
        let session = thread::spawn(move || {
            // Read to the end, which only comes if the end of input was passed
            // on; then answer, after it, as a server finishing a request does.
            let mut heard = String::new();
            (&far).read_to_string(&mut heard).expect("what was said");
            (&far)
                .write_all(b"answered\n")
                .expect("the bridge is listening");
            heard
        });

        let mut output = Vec::new();
        bridge(near, Cursor::new(b"asked\n".to_vec()), &mut output).expect("a clean end");
        assert_eq!(session.join().expect("the session"), "asked\n");
        assert_eq!(output, b"answered\n");
    }
}
