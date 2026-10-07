//! The door in front of the handshake.
//!
//! rmcp's server reads its first message and, if that is not something a
//! conversation may open with, answers it with an error and gives up on the
//! transport. Over stdio there is only ever one transport, so giving up on it is
//! giving up on the agent interface -- and `perspicax` is a desktop as well as a
//! server. Issue #26: a client restarted without its handshake sent `tools/list`
//! first, and the person's session ended with every window in it.
//!
//! [`Gated`] stands between the transport and rmcp and keeps the door open until
//! the conversation has actually begun. Until then it answers a request that
//! cannot open one itself, drops what is not a request at all, and goes back to
//! reading. What it lets through is what rmcp's handshake accepts, judged the way
//! rmcp judges it, so the gate decides nothing rmcp would have decided
//! differently -- it only declines to end everything over it. Once something
//! opens the conversation the gate stands aside for good.
//!
//! The gate is also where a conversation takes the desktop's [`Slot`], because
//! it is the one place that knows the moment a conversation begins. A
//! connection that never gets that far holds nothing. One that does while
//! another conversation is open has its opening request answered with who
//! holds the slot, and the connection ends there: the other conversation is
//! not this one's to wait on.

use rmcp::{
    RoleServer,
    model::{
        ClientJsonRpcMessage, ClientRequest, ErrorData, GetMeta as _, ProtocolVersion,
        ServerJsonRpcMessage,
    },
    transport::Transport,
};

use crate::slot::{Claim, Slot};

/// A transport that will not let one early message end the conversation.
pub(crate) struct Gated<T> {
    inner: T,
    /// The desktop's one conversation, taken when this one opens.
    slot: Slot,
    /// The process on the other end, for whoever finds the slot taken.
    peer: Option<u32>,
    /// The slot, once a message that opens a conversation has gone through.
    /// One-way, and held for as long as rmcp holds this transport, which is
    /// exactly as long as the conversation lasts.
    claim: Option<Claim>,
}

impl<T> Gated<T> {
    pub(crate) fn new(inner: T, slot: Slot, peer: Option<u32>) -> Self {
        Self {
            inner,
            slot,
            peer,
            claim: None,
        }
    }
}

/// What the gate does with a message that arrives before the conversation has
/// begun.
enum Early {
    /// Hand it to rmcp, and stop looking: the conversation has begun.
    Opens,
    /// Hand it to rmcp, and keep looking. Only `ping`, which MCP allows before
    /// `initialize` and rmcp answers in place.
    Passes,
    /// Answer it here and read on.
    Refused,
    /// Not a request, so nothing to answer. Read on.
    Dropped,
}

fn early(message: &ClientJsonRpcMessage) -> Early {
    let ClientJsonRpcMessage::Request(request) = message else {
        return Early::Dropped;
    };
    match &request.request {
        ClientRequest::InitializeRequest(_) => Early::Opens,
        ClientRequest::PingRequest(_) => Early::Passes,
        // 2026-07-28 has no handshake: every request carries its own protocol
        // version and capabilities, and one that does opens the conversation.
        // The test is rmcp's own, so a request rmcp would serve gets through.
        other
            if other
                .get_meta()
                .missing_required_keys(&ProtocolVersion::V_2026_07_28)
                .is_empty() =>
        {
            Early::Opens
        }
        _ => Early::Refused,
    }
}

impl<T> Transport<RoleServer> for Gated<T>
where
    T: Transport<RoleServer>,
{
    type Error = T::Error;

    fn send(
        &mut self,
        item: ServerJsonRpcMessage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
        loop {
            let message = self.inner.receive().await?;
            if self.claim.is_some() {
                return Some(message);
            }
            match early(&message) {
                Early::Opens => {
                    let busy = match self.slot.claim(self.peer) {
                        Ok(claim) => {
                            self.claim = Some(claim);
                            return Some(message);
                        }
                        Err(busy) => busy,
                    };
                    let ClientJsonRpcMessage::Request(request) = message else {
                        unreachable!("only a request opens a conversation");
                    };
                    tracing::warn!(
                        peer = ?self.peer,
                        holder = ?busy.pid,
                        "refused a conversation while another is open"
                    );
                    let refusal = ServerJsonRpcMessage::error(
                        ErrorData::invalid_request(busy.to_string(), None),
                        Some(request.id),
                    );
                    if let Err(error) = self.inner.send(refusal).await {
                        tracing::warn!(%error, "the client went away before its refusal");
                    }
                    // The end of the input, which rmcp takes as the client
                    // going: an ordinary end to a conversation that never began.
                    return None;
                }
                Early::Passes => return Some(message),
                Early::Dropped => {
                    tracing::warn!(?message, "ignored a message sent before `initialize`");
                }
                Early::Refused => {
                    let ClientJsonRpcMessage::Request(request) = message else {
                        unreachable!("only a request is refused");
                    };
                    tracing::warn!(
                        id = ?request.id,
                        "refused a request sent before `initialize`"
                    );
                    let refusal = ServerJsonRpcMessage::error(
                        ErrorData::invalid_request(
                            "this conversation has not begun: send `initialize` first, or \
                             carry io.modelcontextprotocol/protocolVersion and \
                             io.modelcontextprotocol/clientCapabilities in the request's `_meta`",
                            None,
                        ),
                        Some(request.id),
                    );
                    // A refusal that cannot be written is a client that has gone,
                    // and that is how it is reported: the end of the input.
                    if let Err(error) = self.inner.send(refusal).await {
                        tracing::warn!(%error, "the client went away before its refusal");
                        return None;
                    }
                }
            }
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}
