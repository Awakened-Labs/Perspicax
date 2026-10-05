//! The tray as a host: it follows whichever watcher holds the name, reads
//! each status icon the watcher lists, reads it again whenever its program
//! says it changed, and carries out what the shell asks of the programs.
//!
//! This task alone holds what is known of the icons. Whatever waits on a
//! program, reading an icon or a menu, or asking for what a press is for,
//! is a task of its own that reports back here; so is each stream of news
//! from the bus.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};
use zbus::{
    Connection, Proxy,
    fdo::{DBusProxy, RequestNameFlags},
    proxy::{Builder, CacheProperties},
    zvariant::{OwnedValue, Value},
};

use super::{Ask, News, dbusmenu, item, next, text, watcher};
use crate::model::{
    menu::Menu,
    tray::{self, Press},
};

/// How long a program has to answer before it is taken not to.
const ANSWER: Duration = Duration::from_secs(5);

/// Which tray of this process the next is, for a name of its own on the
/// bus while the last one gives its up.
static TRAYS: AtomicU64 = AtomicU64::new(1);

/// Where the shell is told what the tray read: `false` once the shell is
/// gone.
pub(super) type Tell = Box<dyn Fn(News) -> bool + Send>;

/// What reaches the host.
pub(super) enum Inbox {
    /// The shell asked something of an icon's program.
    Asked(Ask),
    /// The shell has no more use for the tray.
    Stop,
    /// What the watcher lists: on starting, and whenever another program
    /// comes to hold its name.
    Listed(Vec<String>),
    Registered(String),
    Unregistered(String),
    /// The program of this bus name left the bus.
    Left(String),
    /// The icon of `key`, read.
    Read {
        key: u64,
        read: item::Read,
    },
    /// Its menu, read.
    Menu {
        key: u64,
        menu: Menu,
    },
}

/// What the host holds.
struct Host {
    connection: Connection,
    /// The tray's own name on the bus, by which it registers as a host.
    name: String,
    inbox: UnboundedSender<Inbox>,
    tell: Tell,
    /// The status icons the watcher listed, in the order it did.
    icons: Vec<Followed>,
    /// The key the next icon gets.
    next: u64,
}

/// A status icon the host follows.
struct Followed {
    key: u64,
    /// As the watcher lists it.
    service: String,
    /// Where its menu is served, once it is read and if it has one.
    menu: Option<String>,
    /// The task that reads it, and again whenever it changes.
    following: JoinHandle<()>,
}

/// Run the tray on the bus at `bus`, or the session's, until `received`
/// says to stop or `tell` says the shell is gone.
pub(super) async fn run(
    bus: Option<String>,
    mut received: UnboundedReceiver<Inbox>,
    inbox: UnboundedSender<Inbox>,
    tell: Tell,
) {
    let connection = match connect(bus.as_deref()).await {
        Ok(connection) => connection,
        Err(error) => {
            tracing::warn!("the tray cannot reach the session bus: {error}; it stays empty");
            return;
        }
    };
    if let Err(error) = watcher::serve(&connection).await {
        tracing::warn!(
            "the tray cannot watch for status icons: {error}; it shows what another program lists"
        );
    }
    let name = format!(
        "org.kde.StatusNotifierHost-{}-{}",
        std::process::id(),
        TRAYS.fetch_add(1, Ordering::Relaxed)
    );
    let name = match connection
        .request_name_with_flags(name.as_str(), RequestNameFlags::DoNotQueue.into())
        .await
    {
        Ok(_) => name,
        Err(error) => {
            tracing::debug!("the tray goes by its unique name: {error}");
            connection
                .unique_name()
                .map_or_else(String::new, ToString::to_string)
        }
    };
    let mut host = Host {
        connection,
        name,
        inbox,
        tell,
        icons: Vec::new(),
        next: 1,
    };
    if let Err(error) = host.listen().await {
        tracing::warn!("the tray cannot follow the status icons: {error}; it stays empty");
        return;
    }
    while let Some(message) = received.recv().await {
        if !host.handle(message) {
            break;
        }
    }
    for icon in &host.icons {
        icon.following.abort();
    }
}

/// A connection to the bus at `address`, or to the session's, on which a
/// program that does not answer in time is taken not to.
async fn connect(address: Option<&str>) -> zbus::Result<Connection> {
    let builder = match address {
        Some(address) => zbus::connection::Builder::address(address)?,
        None => zbus::connection::Builder::session()?,
    };
    builder.method_timeout(ANSWER).build().await
}

impl Host {
    /// Listen for what the watcher and the bus say, and read what the
    /// watcher lists now.
    async fn listen(&mut self) -> zbus::Result<()> {
        let watcher: Proxy<'static> = Builder::new(&self.connection)
            .destination(watcher::NAME)?
            .path(watcher::PATH)?
            .interface(watcher::NAME)?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        for (signal, news) in [
            (
                "StatusNotifierItemRegistered",
                Inbox::Registered as fn(String) -> Inbox,
            ),
            ("StatusNotifierItemUnregistered", Inbox::Unregistered),
        ] {
            let mut signals = watcher.receive_signal(signal).await?;
            let inbox = self.inbox.clone();
            tokio::spawn(async move {
                while let Some(signal) = next(&mut signals).await {
                    match signal.body().deserialize::<String>() {
                        Ok(service) => {
                            if inbox.send(news(service)).is_err() {
                                return;
                            }
                        }
                        Err(error) => tracing::debug!("a watcher's {signal:?} said: {error}"),
                    }
                }
            });
        }

        // Another program came to hold the watcher's name: what it lists is
        // read, and the tray registers with it.
        let mut holders = watcher.receive_owner_changed().await?;
        let (connection, name, inbox) = (
            self.connection.clone(),
            self.name.clone(),
            self.inbox.clone(),
        );
        tokio::spawn(async move {
            while let Some(holder) = next(&mut holders).await {
                if holder.is_some() {
                    tokio::spawn(relist(connection.clone(), name.clone(), inbox.clone()));
                }
            }
        });

        // A program leaving takes its icons with it.
        let mut owners = DBusProxy::new(&self.connection)
            .await?
            .receive_name_owner_changed()
            .await?;
        let inbox = self.inbox.clone();
        tokio::spawn(async move {
            while let Some(signal) = next(&mut owners).await {
                let Ok(changed) = signal.args() else {
                    continue;
                };
                if changed.new_owner().is_none()
                    && inbox.send(Inbox::Left(changed.name().to_string())).is_err()
                {
                    return;
                }
            }
        });

        tokio::spawn(relist(
            self.connection.clone(),
            self.name.clone(),
            self.inbox.clone(),
        ));
        Ok(())
    }

    /// Take in `message`. `false` to stop.
    fn handle(&mut self, message: Inbox) -> bool {
        match message {
            Inbox::Stop => false,
            Inbox::Asked(ask) => {
                self.ask(ask);
                true
            }
            Inbox::Listed(services) => {
                services.into_iter().for_each(|service| self.add(service));
                true
            }
            Inbox::Registered(service) => {
                self.add(service);
                true
            }
            Inbox::Unregistered(service) => self.remove(|icon| icon.service == service),
            // The watcher served here forgets what the program registered,
            // which unregisters its icons. The host drops them as well, for
            // an icon registered with a watcher that has since given up the
            // name, and will never say it went.
            Inbox::Left(name) => {
                if name.starts_with(':') {
                    let connection = self.connection.clone();
                    let left = name.clone();
                    tokio::spawn(async move {
                        if let Err(error) = watcher::left(&connection, &left).await {
                            tracing::debug!("the watcher could not forget {left}: {error}");
                        }
                    });
                }
                self.remove(|icon| tray::address(&icon.service).0 == name)
            }
            Inbox::Read { key, read } => match self.icons.iter_mut().find(|icon| icon.key == key) {
                Some(icon) => {
                    icon.menu = read.menu;
                    (self.tell)(News::Item(key, read.item))
                }
                None => true,
            },
            Inbox::Menu { key, menu } => {
                !self.icons.iter().any(|icon| icon.key == key) || (self.tell)(News::Menu(key, menu))
            }
        }
    }

    /// Follow the icon the watcher lists as `service`, unless it is already
    /// followed.
    fn add(&mut self, service: String) {
        if self.icons.iter().any(|icon| icon.service == service) {
            return;
        }
        let key = self.next;
        self.next += 1;
        let following = tokio::spawn(follow(
            self.connection.clone(),
            key,
            service.clone(),
            self.inbox.clone(),
        ));
        self.icons.push(Followed {
            key,
            service,
            menu: None,
            following,
        });
    }

    /// Stop following the icons that are `gone`, and tell the shell. `false`
    /// if the shell is gone.
    fn remove(&mut self, gone: impl Fn(&Followed) -> bool) -> bool {
        let mut told = true;
        self.icons.retain(|icon| {
            if !gone(icon) {
                return true;
            }
            icon.following.abort();
            told &= (self.tell)(News::Gone(icon.key));
            false
        });
        told
    }

    /// Carry out `ask`, by a task of its own.
    fn ask(&self, ask: Ask) {
        let (Ask::Press { key, .. } | Ask::Chosen { key, .. }) = ask;
        let Some(icon) = self.icons.iter().find(|icon| icon.key == key) else {
            return;
        };
        let (destination, path) = tray::address(&icon.service);
        let (destination, path) = (destination.to_owned(), path.to_owned());
        let (menu, connection, inbox) = (
            icon.menu.clone(),
            self.connection.clone(),
            self.inbox.clone(),
        );
        match ask {
            Ask::Chosen { id, .. } => {
                let Some(menu) = menu else {
                    return;
                };
                tokio::spawn(async move {
                    if let Err(error) =
                        dbusmenu::clicked(&connection, &destination, &menu, id).await
                    {
                        tracing::debug!(destination, id, "the choice was not taken: {error}");
                    }
                });
            }
            Ask::Press { press, at, .. } => {
                tokio::spawn(async move {
                    let open = match press {
                        Press::Menu => true,
                        press => {
                            match item::press(&connection, &destination, &path, press, at).await {
                                Ok(()) => false,
                                // A program that does not answer an activation,
                                // as libappindicator's do not, has its menu
                                // opened instead, as Plasma opens it.
                                Err(error) => {
                                    tracing::debug!(destination, ?press, "not answered: {error}");
                                    press == Press::Activate
                                }
                            }
                        }
                    };
                    let Some(menu) = menu.filter(|_| open) else {
                        return;
                    };
                    match dbusmenu::read(&connection, &destination, &menu).await {
                        Ok(menu) => {
                            inbox.send(Inbox::Menu { key, menu }).ok();
                        }
                        Err(error) => {
                            tracing::info!(destination, "its menu could not be read: {error}");
                        }
                    }
                });
            }
        }
    }
}

/// Register as a host with whoever holds the watcher's name, and read what
/// it lists.
async fn relist(connection: Connection, name: String, inbox: UnboundedSender<Inbox>) {
    let listed = async {
        connection
            .call_method(
                Some(watcher::NAME),
                watcher::PATH,
                Some(watcher::NAME),
                "RegisterStatusNotifierHost",
                &name,
            )
            .await?;
        let reply = connection
            .call_method(
                Some(watcher::NAME),
                watcher::PATH,
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(watcher::NAME, "RegisteredStatusNotifierItems"),
            )
            .await?;
        let listed: OwnedValue = reply.body().deserialize()?;
        let services: Vec<String> = match &*listed {
            Value::Array(services) => services.iter().filter_map(text).collect(),
            _ => Vec::new(),
        };
        zbus::Result::Ok(services)
    };
    match listed.await {
        Ok(services) => {
            inbox.send(Inbox::Listed(services)).ok();
        }
        Err(error) => tracing::info!("no watcher lists status icons yet: {error}"),
    }
}

/// Read the icon the watcher lists as `service`, now and whenever its
/// program says it changed, and pass each read to the host as icon `key`.
async fn follow(connection: Connection, key: u64, service: String, inbox: UnboundedSender<Inbox>) {
    let (destination, path) = tray::address(&service);
    let changes = async {
        let item: Proxy<'_> = Builder::new(&connection)
            .destination(destination)?
            .path(path)?
            .interface(item::INTERFACE)?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        item.receive_all_signals().await
    };
    let mut changes = match changes.await {
        Ok(changes) => Some(changes),
        Err(error) => {
            tracing::debug!(service, "its changes will not be followed: {error}");
            None
        }
    };
    loop {
        match item::read(&connection, destination, path).await {
            Ok(read) => {
                if inbox.send(Inbox::Read { key, read }).is_err() {
                    return;
                }
            }
            Err(error) => tracing::info!(service, "a status icon could not be read: {error}"),
        }
        let Some(changes) = &mut changes else {
            return;
        };
        if next(changes).await.is_none() {
            return;
        }
    }
}
