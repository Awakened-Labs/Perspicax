//! `org.kde.StatusNotifierWatcher`: where programs register their status
//! icons, and trays register to be told of them.
//!
//! The shell serves one, and asks for its name in line behind any other
//! program that holds it. It lists each icon as the specification's hosts
//! read one, a bus name or a bus name and a path, and forgets an icon, or a
//! tray, when the program that registered it leaves the bus.

use zbus::{
    Connection,
    fdo::{self, RequestNameReply},
    interface,
    message::Header,
    object_server::SignalEmitter,
};

use crate::model::tray;

/// The watcher's name on the bus, which its interface has too, and its
/// object.
pub(super) const NAME: &str = "org.kde.StatusNotifierWatcher";
pub(super) const PATH: &str = "/StatusNotifierWatcher";

/// What the watcher was told.
#[derive(Debug, Default)]
pub(super) struct Watcher {
    /// Each status icon, as it is listed, with the unique bus name of the
    /// program that registered it.
    items: Vec<(String, String)>,
    /// Each tray, likewise.
    hosts: Vec<(String, String)>,
}

/// Serve the watcher on `connection`, and ask for its name, in line behind
/// any program that holds it now.
pub(super) async fn serve(connection: &Connection) -> zbus::Result<()> {
    connection
        .object_server()
        .at(PATH, Watcher::default())
        .await?;
    match connection
        .request_name_with_flags(NAME, Default::default())
        .await?
    {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {
            tracing::info!("the tray watches for status icons");
        }
        RequestNameReply::InQueue | RequestNameReply::Exists => tracing::info!(
            "another program watches for status icons; the tray shows what it lists, \
             and watches itself if that program stops"
        ),
    }
    Ok(())
}

/// The program of unique bus name `name` left the bus: forget the icons
/// and the trays it registered.
pub(super) async fn left(connection: &Connection, name: &str) -> zbus::Result<()> {
    let watcher = connection
        .object_server()
        .interface::<_, Watcher>(PATH)
        .await?;
    let emitter = watcher.signal_emitter();
    let mut watching = watcher.get_mut().await;
    let gone: Vec<String> = watching
        .items
        .iter()
        .filter(|(_, owner)| owner == name)
        .map(|(item, _)| item.clone())
        .collect();
    watching.items.retain(|(_, owner)| owner != name);
    let hosts = watching.hosts.len();
    watching.hosts.retain(|(_, owner)| owner != name);
    for item in &gone {
        Watcher::status_notifier_item_unregistered(emitter, item).await?;
    }
    if !gone.is_empty() {
        watching
            .registered_status_notifier_items_changed(emitter)
            .await?;
    }
    if watching.hosts.len() != hosts {
        Watcher::status_notifier_host_unregistered(emitter).await?;
        if watching.hosts.is_empty() {
            watching
                .is_status_notifier_host_registered_changed(emitter)
                .await?;
        }
    }
    Ok(())
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    /// A program's status icon, at `service`: its bus name, or a path on
    /// the bus name it sends from.
    async fn register_status_notifier_item(
        &mut self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let sender = sender(&header)?;
        let item = tray::registered(service, &sender);
        if self.items.iter().any(|(listed, _)| *listed == item) {
            return Ok(());
        }
        tracing::debug!(item, "a status icon registered");
        self.items.push((item.clone(), sender));
        Self::status_notifier_item_registered(&emitter, &item).await?;
        self.registered_status_notifier_items_changed(&emitter)
            .await?;
        Ok(())
    }

    /// A tray, by its bus name.
    async fn register_status_notifier_host(
        &mut self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let sender = sender(&header)?;
        if self.hosts.iter().any(|(host, _)| host == service) {
            return Ok(());
        }
        let first = self.hosts.is_empty();
        self.hosts.push((service.to_owned(), sender));
        Self::status_notifier_host_registered(&emitter).await?;
        if first {
            self.is_status_notifier_host_registered_changed(&emitter)
                .await?;
        }
        Ok(())
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.iter().map(|(item, _)| item.clone()).collect()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        !self.hosts.is_empty()
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_unregistered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// The unique bus name a call came from.
fn sender(header: &Header<'_>) -> fdo::Result<String> {
    header
        .sender()
        .map(ToString::to_string)
        .ok_or_else(|| fdo::Error::Failed("a call from no one".to_owned()))
}
