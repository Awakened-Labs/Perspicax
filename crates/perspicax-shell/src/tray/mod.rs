//! The tray on the session bus: other programs' status icons, found as
//! the StatusNotifierItem specification has them found, and their menus
//! read as `com.canonical.dbusmenu` serves them.
//!
//! Programs register their icons with a watcher, `org.kde.StatusNotifierWatcher`,
//! and a tray is a host that the watcher tells of them. The shell is both:
//! it serves a watcher, and if another program already does, as Plasma or
//! waybar would, it waits in line for the name and shows what that one
//! lists meanwhile. Either way it follows whichever watcher holds the name,
//! and when that changes the programs register again with the new one, as
//! the specification has them do.
//!
//! All of it runs on a thread of its own, on a tokio runtime, as zbus is
//! built for the whole workspace. What it reads is told to the shell's loop
//! as [`News`], and what a person does with an icon reaches it as an
//! [`Ask`]. Nothing it waits for holds anything else up: each icon is read,
//! and each program asked, by a task of its own, so a program that never
//! answers keeps only its own icon waiting.

mod dbusmenu;
mod host;
mod item;
mod watcher;

use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use zbus::zvariant::Value;

use crate::model::{
    menu::Menu,
    tray::{Item, Press},
};

/// What the tray read, for the shell to show.
#[derive(Debug)]
pub(crate) enum News {
    /// The status icon of this key appeared, or changed: as it is now.
    Item(u64, Item),
    /// It went.
    Gone(u64),
    /// Its menu, read to be opened beside it.
    Menu(u64, Menu),
}

/// What a person asked of a status icon's program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ask {
    /// What a press on the icon of `key` is for, the pointer `at` a point
    /// of the desk.
    Press {
        key: u64,
        press: Press,
        at: (i32, i32),
    },
    /// Item `id` of its menu was chosen.
    Chosen { key: u64, id: i32 },
}

/// The tray's thread, while it runs. Dropping it stops it, which gives up
/// its names on the bus.
pub(crate) struct Running {
    inbox: UnboundedSender<host::Inbox>,
}

impl Running {
    /// Pass `ask` to the tray's thread.
    pub(crate) fn ask(&self, ask: Ask) {
        if self.inbox.send(host::Inbox::Asked(ask)).is_err() {
            tracing::debug!("the tray has stopped; {ask:?} is dropped");
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.inbox.send(host::Inbox::Stop).ok();
    }
}

/// Start the tray on the bus at address `bus`, or on the session's, telling
/// `tell` what it reads until `tell` says the shell is gone. A bus that
/// cannot be reached is reported, and the tray stays empty.
pub(crate) fn start(bus: Option<String>, tell: impl Fn(News) -> bool + Send + 'static) -> Running {
    let (inbox, received) = unbounded_channel();
    let sender = inbox.clone();
    let started = std::thread::Builder::new()
        .name("perspicax-tray".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::warn!("the tray has no runtime to run on: {error}");
                    return;
                }
            };
            runtime.block_on(host::run(bus, received, sender, Box::new(tell)));
        });
    if let Err(error) = started {
        tracing::warn!("the tray's thread could not start: {error}");
    }
    Running { inbox }
}

/// The next of `stream`'s items, or `None` once it has ended.
async fn next<S>(stream: &mut S) -> Option<S::Item>
where
    S: zbus::export::futures_core::Stream + Unpin,
{
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await
}

/// `value`, out of any variants it is wrapped in.
fn plain<'v>(value: &'v Value<'v>) -> &'v Value<'v> {
    match value {
        Value::Value(inner) => plain(inner),
        value => value,
    }
}

/// The text `value` holds, as a string or an object path.
fn text(value: &Value<'_>) -> Option<String> {
    match plain(value) {
        Value::Str(text) => Some(text.to_string()),
        Value::ObjectPath(path) => Some(path.to_string()),
        _ => None,
    }
}
