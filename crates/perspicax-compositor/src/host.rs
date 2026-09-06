//! The boundary an agent reaches the compositor through, in both directions.
//!
//! [`crate::facts`] carries what the compositor knows *outward*, as a published
//! snapshot behind a lock. That works because reading is a question about the
//! past: a reader wants a consistent picture, not a live one, and a lock over a
//! value nobody mutates in place is the cheapest way to hand one over.
//!
//! Acting cannot work that way, and the reason is not a preference. The
//! `Compositor` is not `Send`: it owns Wayland client state, an xkb context
//! libxkbcommon documents as thread-unsafe, and a calloop loop that is running
//! on exactly one thread. There is no lock that would make it safe to touch
//! from an MCP server's thread, because the problem is not concurrent access to
//! a value -- it is that the value may only ever be touched from one thread at
//! all. So the inbound direction is **a message, not a lock**, as `facts.rs`
//! said it would have to be: a request goes onto a calloop channel, the loop
//! picks it up on its own thread and does the work, and the answer comes back
//! on a reply channel the caller is already waiting on.
//!
//! [`Host`] is what that pair looks like from outside, and it is the first
//! implementor of [`HostView`] in this project. Until now the trait was an
//! argument the architecture made and nothing had ever had to keep.

use std::{
    sync::{
        Arc, Mutex,
        mpsc::{self, RecvTimeoutError, SyncSender},
    },
    time::Duration,
};

use perspicax_index::{Action, HostView, judge};
use perspicax_node::{Origin, Rect, SurfaceId, Visibility};
use smithay::reexports::calloop::channel::{self, Channel, Sender};

use crate::{act::ActError, facts::Facts};

/// How long to wait for the compositor to answer before giving up.
///
/// A bound rather than a blocking receive, and the difference matters more than
/// the number: an unbounded wait on a loop that has wedged hangs the MCP server
/// for the rest of the session, with no error anywhere and no way for an agent
/// to tell that from a slow application. One second is far longer than
/// dispatching input can honestly take -- it is a handful of protocol writes on
/// a socket -- so a timeout here means something is wrong rather than busy.
const REPLY_TIMEOUT: Duration = Duration::from_secs(1);

/// One action, addressed to one surface, with somewhere to put the answer.
///
/// The surface is carried rather than derived. `perspicax-index` resolved a
/// node to a surface through the join, weighing two independently attested
/// pids; a compositor that re-derived it here from a coordinate would be
/// second- guessing that with worse evidence.
pub struct Request {
    /// Which surface to act on.
    pub surface: SurfaceId,
    /// What to do to it.
    pub action: Action,
    /// Where the outcome goes. A `SyncSender` with a bound of one: nobody ever
    /// wants a queue of stale answers, and a caller that has stopped waiting
    /// should make the send fail rather than fill a buffer.
    pub reply: SyncSender<Result<crate::act::Dispatched, ActError>>,
}

/// The inbound half of the compositor's boundary, symmetric with [`Facts`].
///
/// Cloneable and cheap, like `Facts` and `Stop`, so the caller can hold it,
/// hand copies to whoever acts, and pass it to [`crate::run`] -- which takes
/// the receiving end out of it exactly once.
#[derive(Clone)]
pub struct Requests {
    sender: Sender<Request>,
    /// The receiving end, waiting to be claimed by a running loop. `Option`
    /// because a `Channel` cannot be cloned or shared: exactly one loop may own
    /// it, and taking it is how that is enforced rather than hoped for.
    inbox: Arc<Mutex<Option<Channel<Request>>>>,
}

impl Requests {
    /// A channel nobody is listening on yet.
    #[must_use]
    pub fn new() -> Self {
        let (sender, channel) = channel::channel();
        Self {
            sender,
            inbox: Arc::new(Mutex::new(Some(channel))),
        }
    }

    /// Claim the receiving end. The second caller gets `None`.
    pub(crate) fn take_inbox(&self) -> Option<Channel<Request>> {
        self.inbox.lock().ok()?.take()
    }
}

impl Default for Requests {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Requests {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Requests").finish_non_exhaustive()
    }
}

/// What only a compositor knows, answered by one.
///
/// Reads come from the published snapshot and cost a read lock; the one write
/// crosses to the compositor's thread and waits for it. Both halves are here
/// because [`HostView`] is one trait, and a port of this project to a GNOME
/// extension or a KWin plugin has to answer the same four questions however it
/// obtains them.
#[derive(Clone)]
pub struct Host {
    facts: Facts,
    requests: Requests,
    timeout: Duration,
}

impl Host {
    /// Bind the two halves together.
    #[must_use]
    pub fn new(facts: &Facts, requests: &Requests) -> Self {
        Self {
            facts: facts.clone(),
            requests: requests.clone(),
            timeout: REPLY_TIMEOUT,
        }
    }

    /// The same host with a different patience. Tests want a short one, so that
    /// asserting the timeout works does not cost a second of wall clock.
    #[must_use]
    pub fn waiting(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Send an action to the compositor and wait for what happened.
    ///
    /// # Errors
    ///
    /// [`ActError::Unreachable`] if no loop is listening or none answers in
    /// time; otherwise whatever the compositor said went wrong.
    pub fn act(
        &self,
        surface: SurfaceId,
        action: &Action,
    ) -> Result<crate::act::Dispatched, ActError> {
        let (reply, answer) = mpsc::sync_channel(1);
        self.requests
            .sender
            .send(Request {
                surface,
                action: action.clone(),
                reply,
            })
            .map_err(|_| ActError::Unreachable)?;

        match answer.recv_timeout(self.timeout) {
            Ok(outcome) => outcome,
            // Disconnected means the loop took the request and then died, or
            // dropped it without replying. Timeout means it never got round to
            // it. An agent can do nothing different about either, so they are
            // one error with one meaning: the compositor did not answer.
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                Err(ActError::Unreachable)
            }
        }
    }
}

impl HostView for Host {
    type Error = ActError;

    /// Who owns this surface, from the credentials the connection carried.
    ///
    /// A surface nobody has published facts for is `Unattributed`, which the
    /// gate refuses -- so a race between an act and a window closing fails
    /// closed rather than acting on whatever took its id. Nothing takes its id:
    /// they are minted monotonically and never reused. Both halves are needed,
    /// because only one of them is this crate's to guarantee.
    fn origin(&self, surface: SurfaceId) -> Origin {
        self.facts
            .read()
            .surface(surface)
            .map_or(Origin::Unattributed, |facts| facts.origin.clone())
    }

    /// Whether a window-relative rect on this surface can actually be seen.
    ///
    /// Delegated to `perspicax_index::judge`, which is the same function the
    /// index uses when it judges a whole tree. Deliberately not a second
    /// implementation: two occlusion policies that agreed today would disagree
    /// eventually, and the disagreement would show up as an agent clicking
    /// something it had been told was visible.
    fn visibility(&self, surface: SurfaceId, rect: Rect) -> Visibility {
        judge(&self.facts.read(), surface, rect).visibility
    }

    /// How many frames of damage this surface has taken, ever.
    ///
    /// Monotonic, and deliberately *not* a staleness verdict. A host cannot
    /// answer "how stale is this for you" because it does not know what its
    /// reader last reconciled -- that number lives in the index, and
    /// `SurfaceFacts::damage_since` is where the two are compared. This method
    /// reports the counter; the reader owns the subtraction.
    fn damage_generation(&self, surface: SurfaceId) -> u64 {
        self.facts
            .read()
            .surface(surface)
            .map_or(0, |facts| facts.damage_generation)
    }

    fn dispatch(&mut self, surface: SurfaceId, action: &Action) -> Result<(), Self::Error> {
        self.act(surface, action).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::act::Dispatched;

    /// A stand-in for the compositor's loop: takes requests off the channel on
    /// its own thread and answers them. Enough to prove the round trip without
    /// binding a Wayland socket, which the ignored tests already cover.
    fn loop_answering(
        requests: &Requests,
        outcome: Result<Dispatched, ActError>,
    ) -> thread::JoinHandle<usize> {
        let inbox = requests.take_inbox().expect("the inbox is unclaimed");
        thread::spawn(move || {
            let mut served = 0;
            while let Ok(request) = inbox.recv() {
                served += 1;
                let _ = request.reply.send(outcome.clone());
            }
            served
        })
    }

    fn dispatched() -> Dispatched {
        Dispatched {
            at: std::time::Instant::now(),
            focus_before: None,
            focus_after: Some(SurfaceId(1)),
        }
    }

    #[test]
    fn an_action_crosses_to_the_loop_and_the_answer_comes_back() {
        let requests = Requests::new();
        let worker = loop_answering(&requests, Ok(dispatched()));
        let host = Host::new(&Facts::new(), &requests);

        let outcome = host.act(SurfaceId(1), &Action::Focus).expect("answered");
        assert_eq!(outcome.focus_after, Some(SurfaceId(1)));

        drop(host);
        drop(requests);
        assert_eq!(worker.join().expect("worker lived"), 1);
    }

    #[test]
    fn the_compositors_own_error_is_carried_back_rather_than_flattened() {
        let requests = Requests::new();
        let _worker = loop_answering(&requests, Err(ActError::Untypeable('\u{4e2d}')));
        let host = Host::new(&Facts::new(), &requests);

        assert_eq!(
            host.act(SurfaceId(1), &Action::Focus),
            Err(ActError::Untypeable('\u{4e2d}'))
        );
    }

    #[test]
    fn a_loop_that_never_answers_times_out_instead_of_hanging() {
        let requests = Requests::new();
        // The inbox is taken and then dropped: a loop that claimed the channel
        // and died, which is the shape of the hang this bound exists to stop.
        let _ = requests.take_inbox();
        let host = Host::new(&Facts::new(), &requests).waiting(Duration::from_millis(50));

        assert_eq!(
            host.act(SurfaceId(1), &Action::Focus),
            Err(ActError::Unreachable)
        );
    }

    #[test]
    fn a_surface_nobody_has_published_facts_for_fails_closed() {
        let host = Host::new(&Facts::new(), &Requests::new());
        assert_eq!(host.origin(SurfaceId(9)), Origin::Unattributed);
        assert_eq!(host.damage_generation(SurfaceId(9)), 0);
        // `Unknown`, not `Unmapped`, and the distinction is the honest one:
        // `Unmapped` claims we know the surface exists and is not on screen,
        // whereas nothing has been published about this one at all. Both are
        // refused, but only one of them is a statement we can support.
        assert_eq!(
            host.visibility(SurfaceId(9), Rect::new(0.0, 0.0, 1.0, 1.0)),
            Visibility::Unknown
        );
    }
}
