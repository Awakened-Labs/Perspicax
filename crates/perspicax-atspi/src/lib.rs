//! `impl Ingest` over the AT-SPI2 accessibility bus.
//!
//! This is the slow path, and it ships first on purpose. It works against stock
//! GTK and Qt with no patched toolkit and no cooperation from the application,
//! which means the whole pipeline above it -- stable ids, selectors, the
//! compositor join, refusals, receipts -- can be proven against real programs
//! before any of the interesting transport work begins.
//!
//! Two constraints govern everything here, and both are enforced in code rather
//! than asked for in a comment:
//!
//! - **Never poll a tree.** A full AT-SPI read costs a D-Bus round trip per
//!   property and is measured in seconds on the slow path. Bulk-read where the
//!   application offers it, and invalidate from signals otherwise.
//! - **Never trust its coordinates as global.** A Wayland client cannot know
//!   its own position on screen, so `Component.GetExtents` is window-relative
//!   at best, and relative to whatever the toolkit calls its window: GTK and
//!   Qt the window geometry, Firefox its buffer, shadow included. These are
//!   *node space*. The index measures where each window begins in it, and
//!   only a [`HostView`] turns bounds into anything global.
//!
//! # What this crate refuses to conclude
//!
//! Every node it produces is [`Origin::Unattributed`](perspicax_node::Origin) and
//! [`Visibility::Unknown`](perspicax_node::Visibility), so every node it produces is
//! un-actable. That is not a gap waiting to be filled in with the best
//! available guess -- the accessibility bus genuinely cannot answer either
//! question, and answering them anyway is the failure this project exists to
//! remove. A compositor fills them in M2. Until then the refusal gate says no,
//! and [a test](read) says it must.
//!
//! [`HostView`]: perspicax_index::HostView

pub mod app;
pub mod error;
pub mod events;
pub mod map;
pub mod read;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use atspi::{
    proxy::{application::ApplicationProxy, bus::BusProxy},
    zbus::{self, Address, Connection, names::BusName, proxy::CacheProperties},
};
use perspicax_index::{Change, Ingest, Interner};
use perspicax_node::{NodeId, ObservedNode};

pub use crate::{
    app::{AppRef, ObjectKey},
    error::Error,
    events::Subscription,
    read::{GaveUp, Patience, Strategy},
};

/// The one id map for a desktop, shared by every ingest reading it.
///
/// # Why this is shared rather than owned
///
/// [`Interner`] mints from one and never reuses, which makes a [`NodeId`]
/// stable for a node's lifetime -- and every layer above depends on that,
/// most sharply the MCP server, which hands ids to an agent and takes them
/// back. An interner per application breaks it silently: two applications both
/// mint id 1, and nothing anywhere can tell the two apart afterwards.
///
/// [`ObjectKey`] is `(unique connection name, object path)` and is therefore
/// already unique across the whole bus, so one interner over it is correct by
/// construction rather than by a merge step somebody has to remember.
///
/// A `Mutex` and not a `RefCell`, because [`Ingest`]'s futures are `Send` by
/// the trait's own bound and a reader may well drive several applications on a
/// multi-threaded runtime. Nothing holds the lock across an `await`.
pub type Ids = Arc<Mutex<Interner<ObjectKey>>>;

/// The id map, locked.
///
/// A free function rather than a method on [`AtspiIngest`], because a method
/// taking `&self` borrows the whole ingest and [`Ingest::drain_changes`] needs
/// the subscription mutably at the same moment. Borrowing the one field says
/// what is actually wanted.
///
/// A poisoned interner is unrecoverable rather than an error to report: it
/// means a thread panicked mid-mint, so the map may hold a key with no id or an
/// id with no key, and every stability guarantee above it rests on that map
/// being consistent.
fn lock(ids: &Ids) -> std::sync::MutexGuard<'_, Interner<ObjectKey>> {
    ids.lock().expect("a panic left the id map inconsistent")
}

/// Reads one application's accessibility tree off the AT-SPI2 bus.
///
/// Scoped to a single application rather than the whole desktop, because the
/// two read strategies are chosen per application: `--app gtk4-widget-factory`
/// takes one round trip and `--app <a Qt program>` takes six per node, and an
/// ingest that averaged them would hide the only number M1 is trying to
/// produce.
#[derive(Debug)]
pub struct AtspiIngest {
    /// The accessibility bus, with a deadline on every call. See
    /// [`reading_bus`].
    bus: Connection,
    app: AppRef,
    /// AT-SPI's `(bus name, object path)` in, opaque [`NodeId`]s out.
    ///
    /// Private by default and shared by [`AtspiIngest::sharing`]. A reader of
    /// one application may keep its own; a reader of a desktop must not, and
    /// [`Ids`] says why.
    interner: Ids,
    /// The strategy the last cold read actually used. `None` until one has run
    /// -- it is a measurement, not a configuration.
    strategy: Option<Strategy>,
    /// A strategy to use instead of probing for one. `None` is the normal
    /// case; see [`AtspiIngest::forcing`].
    forced: Option<Strategy>,
    with_geometry: bool,
    /// How long one read keeps asking before it settles for what it has.
    /// `None` is no budget; see [`AtspiIngest::within`].
    budget: Option<Duration>,
    /// How much silence the walk sits through, remembered from one read of
    /// this application to the next.
    walking: Patience,
    /// The same for asking each node where it is.
    placing: Patience,
    /// The live signal subscription. Opened by [`AtspiIngest::connect`] before
    /// any tree is read, so that nothing can change in the gap between reading
    /// a tree and starting to listen.
    events: Subscription,
}

impl AtspiIngest {
    /// Connect to the accessibility bus and resolve one application by name.
    ///
    /// # Errors
    ///
    /// [`Error::Bus`] if the accessibility bus cannot be reached at all --
    /// over SSH that is nearly always a missing `XDG_RUNTIME_DIR` or
    /// `DBUS_SESSION_BUS_ADDRESS` rather than anything about the code.
    /// [`Error::NoSuchApp`] if the bus is fine and nothing on it answers to
    /// `name`; it carries the names that were present.
    pub async fn connect(name: &str) -> Result<Self, Error> {
        let bus = reading_bus().await?;

        let mut available = Vec::new();
        for candidate in applications(&bus).await? {
            if candidate.name() == name {
                return Self::attach(candidate).await;
            }
            available.push(candidate.name().to_owned());
        }

        Err(Error::NoSuchApp {
            name: name.to_owned(),
            available,
        })
    }

    /// Read an application already enumerated by [`on_the_bus`].
    ///
    /// # Why a name is not an address
    ///
    /// [`connect`](Self::connect) resolves a display name, and a display name
    /// is not unique: a desktop can perfectly well be running two copies of one
    /// program, and one of them can be a leftover nobody has noticed. Resolving
    /// by name silently picks the first, so a caller that enumerated the bus,
    /// found two, and asked for each by name would read the same application
    /// twice and never learn that it had. Measured 2026-09-04 on the test bed,
    /// where two `gtk4-widget-factory` processes were on the bus and the second
    /// was unreachable by name.
    ///
    /// So a caller that already holds an [`AppRef`] passes it here and gets the
    /// application it actually chose, addressed by the unique connection name
    /// underneath. It also saves enumerating the whole bus a second time.
    ///
    /// # Errors
    ///
    /// [`Error::Bus`] if the accessibility bus cannot be reached, or if the
    /// application has gone since it was enumerated.
    pub async fn attach(app: AppRef) -> Result<Self, Error> {
        let bus = reading_bus().await?;
        // Subscribe before returning, and therefore before the caller can take
        // a snapshot. The alternative -- snapshot, then subscribe -- silently
        // loses every change that happens in between, and produces an index
        // that is wrong in a way no later signal corrects.
        let events = Subscription::open(app.root().bus()).await?;
        Ok(Self {
            bus,
            app,
            interner: Ids::default(),
            strategy: None,
            forced: None,
            with_geometry: false,
            budget: None,
            walking: Patience::new(),
            placing: Patience::new(),
            events,
        })
    }

    /// Also read per-node bounds, at one extra D-Bus round trip per node.
    ///
    /// Off by default, and a separate switch rather than something the cold
    /// read always does, because no bulk geometry API exists on either toolkit
    /// -- so this is the difference between the M1 latency table's "cold tree"
    /// and "cold tree + geometry" rows. See [`read::extents`].
    #[must_use]
    pub fn with_geometry(mut self, geometry: bool) -> Self {
        self.with_geometry = geometry;
        self
    }

    /// Stop asking `budget` after a read begins, and keep what was read by
    /// then.
    ///
    /// Without one, a read asks until it has the tree -- which is what a
    /// measurement wants, since a budget would cut short exactly the slow
    /// read it is measuring. A reader of a desktop wants the opposite: one
    /// application, however slowly it answers, must not hold up every other,
    /// and a partial tree of it is worth more than none. A node the read did
    /// not reach is missing, and one it did not place is refused, so settling
    /// early costs what can be acted on and nothing worse.
    ///
    /// Every call is bounded on its own, by [`read::ANSWER`], so a read with a
    /// budget ends within about one node's calls of it.
    #[must_use]
    pub fn within(mut self, budget: Duration) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Read with a named strategy instead of probing for one.
    ///
    /// Exists for measurement, not for configuration. The M1 latency table
    /// compares a bulk read against a walk **of the same application**, and
    /// there is no other way to get both numbers from one tree -- probing, by
    /// design, only ever returns the faster one an application supports.
    ///
    /// It is also how the fast path gets audited. A cache is a claim about a
    /// tree; walking the same tree and comparing is the only way to find out
    /// whether the claim is complete.
    ///
    /// Forcing [`Strategy::Cache`] on an application that has none yields an
    /// empty read rather than an error -- that is what the probe exists to
    /// notice, and suppressing it would hide the very thing being measured.
    #[must_use]
    pub fn forcing(mut self, strategy: Strategy) -> Self {
        self.forced = Some(strategy);
        self
    }

    /// Mint ids from a map shared with every other ingest reading this desktop.
    ///
    /// Required of anything reading more than one application at once, and the
    /// reason is in [`Ids`]: without it two applications each mint id 1 and the
    /// layers above cannot tell those two nodes apart. A caller reading a
    /// single application in isolation -- `perspicax-probe`, the M1 latency
    /// table -- does not need it.
    #[must_use]
    pub fn sharing(mut self, ids: &Ids) -> Self {
        self.interner = Arc::clone(ids);
        self
    }

    /// The application this ingest is reading.
    #[must_use]
    pub fn app(&self) -> &AppRef {
        &self.app
    }

    /// The strategy the last cold read used, once one has run.
    #[must_use]
    pub fn strategy(&self) -> Option<Strategy> {
        self.strategy
    }

    /// The id of the application's root accessible.
    ///
    /// Minted on demand so that a caller has something to hand
    /// [`Ingest::snapshot`] before any tree has been read.
    pub fn root_id(&mut self) -> NodeId {
        lock(&self.interner).intern(self.app.root().clone())
    }

    /// How many ids this ingest has minted, retired ones included.
    #[must_use]
    pub fn minted(&self) -> u64 {
        lock(&self.interner).minted()
    }

    async fn read_subtree(&mut self, root: NodeId) -> Result<Vec<ObservedNode>, Error> {
        // The guard is scoped so that it is gone before the awaits below: a
        // lock held across an `await` would make this future non-`Send`, which
        // `Ingest`'s own bound would refuse.
        let key = lock(&self.interner)
            .key(root)
            .ok_or(Error::UnknownRoot(root.0))?
            .clone();

        let connection = self.bus.clone();
        // One budget for the whole read, shared by both passes: a walk that
        // spends it leaves nothing for geometry, which is the right order --
        // a node unplaced is refused, a node unread is not there at all.
        let until = self.budget.map(|budget| Instant::now() + budget);
        self.walking.begin(until);
        self.placing.begin(until);

        let (strategy, nodes) = match self.forced {
            Some(forced) => (
                forced,
                read::read_with(&connection, &self.app, forced, &mut self.walking).await?,
            ),
            None => read::cold_read(&connection, &self.app, &mut self.walking).await?,
        };
        self.strategy = Some(strategy);
        tracing::debug!(
            app = self.app.name(),
            ?strategy,
            nodes = nodes.len(),
            "cold read"
        );
        settled(self.app.name(), "walking its tree", &self.walking);

        let mut nodes = read::subtree(nodes, &key);
        if self.with_geometry {
            read::geometry(&connection, &mut nodes, &mut self.placing).await;
            settled(self.app.name(), "asking GetExtents", &self.placing);
        }
        Ok(read::to_observed(nodes, &mut lock(&self.interner)))
    }
}

/// Say, once per pass of a read, that it stopped asking and why.
///
/// Once, not per call: the calls themselves are logged at debug, and an
/// application that leaves sixty-four unanswered must not fill a log with
/// sixty-four warnings about one fact.
fn settled(app: &str, pass: &str, patience: &Patience) {
    match patience.gave_up() {
        Some(GaveUp::Unanswered { calls }) => tracing::warn!(
            app,
            calls,
            "stopped {pass}: the application left {calls} call(s) unanswered, \
             so the rest go unasked this read and what was read is kept"
        ),
        Some(GaveUp::OutOfTime) => tracing::warn!(
            app,
            "stopped {pass}: the read ran out of time, and what was read is kept"
        ),
        None => {}
    }
}

/// The accessibility bus, with a deadline on every call made over it.
///
/// # Why not `AccessibilityConnection`
///
/// Because it cannot be given one. It builds its connection itself, from the
/// bus's address, with nothing to say how long a call may wait -- and a call
/// that waits forever is how one Flutter application, which never answers
/// `GetExtents`, used to cost its whole tree (#51). So this asks the session
/// bus for the address the same way it does, and builds the connection with
/// [`read::ANSWER`] as zbus's `method_timeout`. Every call a read makes goes
/// over it, so none can be made without the deadline.
///
/// It also leaves out what `AccessibilityConnection` adds that a read never
/// uses: a registry proxy, and peer-to-peer connections to applications.
///
/// # Errors
///
/// [`Error::Bus`] if the session bus, or the accessibility bus it names,
/// cannot be reached.
async fn reading_bus() -> Result<Connection, Error> {
    // Every failure here is the bus being out of reach rather than a call
    // failing, and says so.
    let unreachable = |error: zbus::Error| Error::Bus(error.into());
    let session = Connection::session().await.map_err(unreachable)?;
    let address: Address = BusProxy::new(&session)
        .await
        .map_err(unreachable)?
        .get_address()
        .await
        .map_err(unreachable)?
        .parse()
        .map_err(unreachable)?;
    zbus::connection::Builder::address(address)
        .map_err(unreachable)?
        .method_timeout(read::ANSWER)
        .build()
        .await
        .map_err(unreachable)
}

impl Ingest for AtspiIngest {
    type Error = Error;

    /// Read `root`'s subtree in as few round trips as this application permits.
    ///
    /// Written as a plain `async fn`, which satisfies the trait's
    /// `impl Future + Send` and -- more usefully -- fails to compile if the
    /// future ever stops being `Send`. zbus proxies are `Send`, so this holds
    /// today; the bound is spelled on the trait so that the day it stops
    /// holding, the error arrives here rather than at some future
    /// `tokio::spawn`.
    async fn snapshot(&mut self, root: NodeId) -> Result<Vec<ObservedNode>, Self::Error> {
        self.read_subtree(root).await
    }

    /// Take what the bus has volunteered since the last call.
    ///
    /// Volunteered, not fetched: this reads a queue of signals that already
    /// arrived and never asks the application anything. It cannot block, and
    /// on a quiet desktop it returns an empty vector immediately.
    ///
    /// Not `Ok(Vec::new())` with a re-read hidden behind it. Diffing a fresh
    /// tree against the cached one would satisfy this signature, pass a test
    /// suite, and be a poll wearing a delta's clothes -- which is the single
    /// thing [`Ingest`]'s own documentation forbids.
    async fn drain_changes(&mut self) -> Result<Vec<Change>, Self::Error> {
        Ok(self.events.drain(&mut lock(&self.interner)))
    }
}

/// Turn accessibility on for this session.
///
/// `org.a11y.Status.IsEnabled` is what Qt's AT-SPI bridge gates on. With it
/// false the application starts perfectly, draws its window, prints nothing,
/// and never joins the accessibility bus -- while GTK's bridge ignores the flag
/// and registers either way, so the symptom is *one* toolkit silently missing
/// and no error anywhere.
///
/// A desktop sets it from dconf (`org.gnome.desktop.interface
/// toolkit-accessibility`), which is why this is invisible on a workstation and
/// fatal in a container: a fresh `HOME` has no dconf state, so the flag
/// defaults to false exactly where nobody is watching for it.
///
/// # Errors
///
/// [`Error::Bus`] if the session bus cannot be reached or the property cannot
/// be set -- which on a machine with no `org.a11y.Bus` at all is the honest
/// answer rather than something to shrug off.
pub async fn enable() -> Result<(), Error> {
    let connection = Connection::session().await?;
    let status = atspi::zbus::Proxy::new(
        &connection,
        "org.a11y.Bus",
        "/org/a11y/bus",
        "org.a11y.Status",
    )
    .await?;
    // `set_property` reports D-Bus's own error type rather than zbus's, and
    // the distinction is not worth a variant: from here both mean the same
    // thing, which is that the accessibility bus would not take the answer.
    status
        .set_property("IsEnabled", true)
        .await
        .map_err(|error| Error::Call(error.into()))?;
    Ok(())
}

/// Every application currently on the accessibility bus.
///
/// The first thing to run when `--app` finds nothing: it distinguishes "that
/// name is wrong" from "the bus is empty", and an empty list on a desktop that
/// visibly has windows on it means the accessibility bus is not the one this
/// process is talking to -- almost always `XDG_RUNTIME_DIR` and
/// `DBUS_SESSION_BUS_ADDRESS` over SSH.
///
/// # Errors
///
/// [`Error::Bus`] if the accessibility bus cannot be reached.
pub async fn on_the_bus() -> Result<Vec<AppRef>, Error> {
    applications(&reading_bus().await?).await
}

/// Every application currently on the accessibility bus.
///
/// Each is asked its name and its toolkit over the reading connection, so an
/// application that does not answer costs [`read::ANSWER`] and is left out,
/// rather than holding up the list of every other one. Every application on
/// the bus is asked, not only the ones a caller wants, so this matters even
/// to a reader that would never have read it.
async fn applications(connection: &Connection) -> Result<Vec<AppRef>, Error> {
    let registry = read::accessible(
        connection,
        &ObjectKey::new("org.a11y.atspi.Registry", "/org/a11y/atspi/accessible/root"),
    )
    .await?;
    let dbus = atspi::zbus::fdo::DBusProxy::new(connection).await?;

    let mut apps = Vec::new();
    for reference in registry.get_children().await? {
        let Some(root) = ObjectKey::from_owned(&reference) else {
            continue;
        };
        let Ok(accessible) = read::accessible(connection, &root).await else {
            continue;
        };

        // An application that answers the registry but not a property read is
        // mid-teardown. It is not this function's business to fail over it.
        let Ok(name) = accessible.name().await else {
            continue;
        };
        // `CacheProperties::No` on every proxy, without exception -- see
        // `read::accessible` for the Qt adaptor this would otherwise kill.
        let toolkit = match ApplicationProxy::builder(connection)
            .destination(root.bus().to_owned())
            .and_then(|builder| builder.path(root.path().to_owned()))
        {
            Ok(builder) => match builder.cache_properties(CacheProperties::No).build().await {
                Ok(proxy) => proxy.toolkit_name().await.unwrap_or_default(),
                Err(_) => String::new(),
            },
            Err(_) => String::new(),
        };

        // From the bus daemon's peer credentials, not from anything the
        // application said about itself. See `AppRef` for why it still does
        // not make an `Origin`.
        let bus_pid = match BusName::try_from(root.bus().to_owned()) {
            Ok(name) => dbus.get_connection_unix_process_id(name).await.ok(),
            Err(_) => None,
        };

        apps.push(AppRef::new(name, toolkit, root, bus_pid));
    }
    Ok(apps)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug [`Ids`] exists to remove, stated as the case that produces it.
    ///
    /// AT-SPI object paths are numbered per application, so
    /// `/org/a11y/atspi/accessible/1` exists in *every* program on the bus.
    /// With an interner each, two applications both mint id 1 for it and
    /// nothing above can tell those two nodes apart -- which matters most
    /// exactly where it is least visible, at the MCP wire, where an agent is
    /// handed an id and hands it back.
    #[test]
    fn one_id_map_across_two_applications_cannot_mint_a_collision() {
        let ids = Ids::default();
        let path = "/org/a11y/atspi/accessible/1";
        let gtk = ObjectKey::new(":1.42", path);
        let qt = ObjectKey::new(":1.57", path);

        let first = lock(&ids).intern(gtk.clone());
        let second = lock(&ids).intern(qt);
        assert_ne!(first, second);

        // And still stable for the node it named, which is the property the
        // sharing must not cost.
        assert_eq!(lock(&ids).intern(gtk), first);
        assert_eq!(lock(&ids).minted(), 2);
    }
}
