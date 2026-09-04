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
//!   at best. Only a [`HostView`] turns bounds into anything global.
//!
//! # What this crate refuses to conclude
//!
//! Every node it produces is [`Origin::Unattributed`](wm_node::Origin) and
//! [`Visibility::Unknown`](wm_node::Visibility), so every node it produces is
//! un-actable. That is not a gap waiting to be filled in with the best
//! available guess -- the accessibility bus genuinely cannot answer either
//! question, and answering them anyway is the failure this project exists to
//! remove. A compositor fills them in M2. Until then the refusal gate says no,
//! and [a test](read) says it must.
//!
//! [`HostView`]: wm_index::HostView

pub mod app;
pub mod error;
pub mod events;
pub mod map;
pub mod read;

use atspi::{
    AccessibilityConnection,
    proxy::application::ApplicationProxy,
    zbus::{Connection, names::BusName, proxy::CacheProperties},
};
use wm_index::{Change, Ingest, Interner};
use wm_node::{NodeId, ObservedNode};

pub use crate::{
    app::{AppRef, ObjectKey},
    error::Error,
    events::Subscription,
    read::Strategy,
};

/// Reads one application's accessibility tree off the AT-SPI2 bus.
///
/// Scoped to a single application rather than the whole desktop, because the
/// two read strategies are chosen per application: `--app gtk4-widget-factory`
/// takes one round trip and `--app <a Qt program>` takes six per node, and an
/// ingest that averaged them would hide the only number M1 is trying to
/// produce.
#[derive(Debug)]
pub struct AtspiIngest {
    bus: AccessibilityConnection,
    app: AppRef,
    /// AT-SPI's `(bus name, object path)` in, opaque [`NodeId`]s out. The one
    /// id map in the process: `wm-index` owns the never-reuse rule, and this
    /// crate would only get it wrong a second time.
    interner: Interner<ObjectKey>,
    /// The strategy the last cold read actually used. `None` until one has run
    /// -- it is a measurement, not a configuration.
    strategy: Option<Strategy>,
    /// A strategy to use instead of probing for one. `None` is the normal
    /// case; see [`AtspiIngest::forcing`].
    forced: Option<Strategy>,
    with_geometry: bool,
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
        let bus = AccessibilityConnection::new().await?;

        let mut available = Vec::new();
        for candidate in applications(&bus).await? {
            if candidate.name() == name {
                // Subscribe before returning, and therefore before the caller
                // can take a snapshot. The alternative -- snapshot, then
                // subscribe -- silently loses every change that happens in
                // between, and produces an index that is wrong in a way no
                // later signal corrects.
                let events = Subscription::open(candidate.root().bus()).await?;
                return Ok(Self {
                    bus,
                    app: candidate,
                    interner: Interner::new(),
                    strategy: None,
                    forced: None,
                    with_geometry: false,
                    events,
                });
            }
            available.push(candidate.name().to_owned());
        }

        Err(Error::NoSuchApp {
            name: name.to_owned(),
            available,
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
        self.interner.intern(self.app.root().clone())
    }

    /// How many ids this ingest has minted, retired ones included.
    #[must_use]
    pub fn minted(&self) -> u64 {
        self.interner.minted()
    }

    async fn read_subtree(&mut self, root: NodeId) -> Result<Vec<ObservedNode>, Error> {
        let key = self
            .interner
            .key(root)
            .ok_or(Error::UnknownRoot(root.0))?
            .clone();

        let connection: Connection = self.bus.connection().clone();
        let (strategy, nodes) = match self.forced {
            Some(forced) => (
                forced,
                read::read_with(&connection, &self.app, forced).await?,
            ),
            None => read::cold_read(&connection, &self.app).await?,
        };
        self.strategy = Some(strategy);
        tracing::debug!(
            app = self.app.name(),
            ?strategy,
            nodes = nodes.len(),
            "cold read"
        );

        let mut nodes = read::subtree(nodes, &key);
        if self.with_geometry {
            for node in &mut nodes {
                node.bounds = read::extents(&connection, &node.key).await;
            }
        }
        Ok(read::to_observed(nodes, &mut self.interner))
    }
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
        Ok(self.events.drain(&mut self.interner))
    }
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
    let bus = AccessibilityConnection::new().await?;
    applications(&bus).await
}

/// Every application currently on the accessibility bus.
async fn applications(bus: &AccessibilityConnection) -> Result<Vec<AppRef>, Error> {
    let registry = bus.root_accessible_on_registry().await?;
    let connection = bus.connection();
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
