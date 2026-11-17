//! Linux desktop services over D-Bus, on one thread with its own Tokio
//! runtime: the XDG GlobalShortcuts portal, which hears plugins'
//! shortcuts under Wayland, where apps can't see keys pressed elsewhere,
//! and the desktop's notification server, `org.freedesktop.Notifications`,
//! which shows plugins' notifications with their actions.
//!
//! When the portal is missing, as on an older desktop, the shortcuts work
//! through `noura --shortcut`, which you bind in the desktop's settings.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use app_core::{NotificationChosen, SystemNotification};
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use ashpd::zbus;
use futures::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// A shortcut to bind: `<plugin>/<shortcut>`, what it does and its keys,
/// such as `Ctrl+Space`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub id: String,
    pub title: String,
    pub keys: String,
}

/// What the portal heard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heard {
    /// The shortcut `<plugin>/<shortcut>` went down, or up, then.
    Shortcut {
        id: String,
        pressed: bool,
        at: Instant,
    },
    /// The portal bound these shortcuts. `None`: there's no portal.
    Bound(Option<Vec<String>>),
}

type Listener = Arc<dyn Fn(Heard) + Send + Sync>;

enum Request {
    Bind(Vec<Binding>),
    Notify(SystemNotification, NotificationChosen),
    Withdraw(String),
}

/// The D-Bus thread.
pub struct Portal {
    requests: UnboundedSender<Request>,
    listener: Arc<Mutex<Option<Listener>>>,
}

static PORTAL: OnceLock<Portal> = OnceLock::new();

/// Whether this session runs under Wayland.
pub fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind == "wayland")
}

impl Portal {
    /// The thread, started the first time.
    pub fn get() -> &'static Self {
        PORTAL.get_or_init(|| {
            let (requests, received) = unbounded_channel();
            let listener: Arc<Mutex<Option<Listener>>> = Arc::default();
            let heard = listener.clone();
            let started = std::thread::Builder::new()
                .name("noura-portal".into())
                .spawn(move || {
                    match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(runtime) => runtime.block_on(run(received, heard)),
                        Err(error) => tracing::warn!(%error, "the D-Bus thread didn't start"),
                    }
                });
            if let Err(error) = started {
                tracing::warn!(%error, "the D-Bus thread didn't start");
            }
            Self { requests, listener }
        })
    }

    /// Hand what the portal hears to `listener`.
    pub fn listen(&self, listener: impl Fn(Heard) + Send + Sync + 'static) {
        *self
            .listener
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(Arc::new(listener));
    }

    /// Bind `bindings` in place of the shortcuts bound before.
    pub fn bind(&self, bindings: Vec<Binding>) {
        let _ = self.requests.send(Request::Bind(bindings));
    }

    pub fn notify(&self, notification: SystemNotification, chosen: NotificationChosen) {
        let _ = self.requests.send(Request::Notify(notification, chosen));
    }

    pub fn withdraw(&self, key: &str) {
        let _ = self.requests.send(Request::Withdraw(key.to_owned()));
    }
}

fn tell(listener: &Arc<Mutex<Option<Listener>>>, heard: Heard) {
    let listener = listener
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    if let Some(listener) = listener {
        listener(heard);
    }
}

async fn run(mut requests: UnboundedReceiver<Request>, listener: Arc<Mutex<Option<Listener>>>) {
    let mut shortcuts: Option<Bound> = None;
    let mut notifications = Notifications::default();
    while let Some(request) = requests.recv().await {
        match request {
            Request::Bind(bindings) => {
                if let Some(old) = shortcuts.take() {
                    old.close().await;
                }
                shortcuts = bind(&bindings, &listener).await;
                if shortcuts.is_none() {
                    tell(&listener, Heard::Bound(None));
                }
            }
            Request::Notify(notification, chosen) => {
                if let Err(error) = notifications.show(notification, chosen).await {
                    tracing::warn!(%error, "the notification server refused a notification");
                }
            }
            Request::Withdraw(key) => notifications.withdraw(&key).await,
        }
    }
}

/// A portal session with shortcuts bound, and the task that hears them.
struct Bound {
    session: ashpd::desktop::Session<GlobalShortcuts>,
    heard: tokio::task::JoinHandle<()>,
}

impl Bound {
    async fn close(self) {
        self.heard.abort();
        let _ = self.session.close().await;
    }
}

/// `keys` as the XDG shortcuts specification writes a trigger, such as
/// `CTRL+space` for `Ctrl+Space`.
pub fn trigger(keys: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for part in keys.split('+') {
        let part = match part {
            "Ctrl" => "CTRL".to_owned(),
            "Alt" => "ALT".to_owned(),
            "Shift" => "SHIFT".to_owned(),
            "Cmd" => "LOGO".to_owned(),
            key if key.len() > 1 && key.starts_with('F') && key[1..].parse::<u8>().is_ok() => {
                key.to_owned()
            }
            key => key.to_lowercase(),
        };
        parts.push(part);
    }
    parts.join("+")
}

async fn bind(bindings: &[Binding], listener: &Arc<Mutex<Option<Listener>>>) -> Option<Bound> {
    if bindings.is_empty() {
        tell(listener, Heard::Bound(Some(Vec::new())));
        return None;
    }
    let portal = GlobalShortcuts::new()
        .await
        .inspect_err(|error| tracing::info!(%error, "no GlobalShortcuts portal"))
        .ok()?;
    let session = portal
        .create_session(Default::default())
        .await
        .inspect_err(|error| tracing::warn!(%error, "the shortcuts portal refused a session"))
        .ok()?;
    let triggers: Vec<String> = bindings
        .iter()
        .map(|binding| trigger(&binding.keys))
        .collect();
    let shortcuts: Vec<NewShortcut> = bindings
        .iter()
        .zip(&triggers)
        .map(|(binding, trigger)| {
            NewShortcut::new(binding.id.clone(), binding.title.clone())
                .preferred_trigger(Some(trigger.as_str()))
        })
        .collect();
    let bound = portal
        .bind_shortcuts(&session, &shortcuts, None, Default::default())
        .await
        .and_then(|request| request.response())
        .inspect_err(|error| tracing::warn!(%error, "the shortcuts portal didn't bind"))
        .ok()?;
    let ids: Vec<String> = bound
        .shortcuts()
        .iter()
        .map(|shortcut| shortcut.id().to_owned())
        .collect();
    tell(listener, Heard::Bound(Some(ids)));
    let mut activated = portal.receive_activated().await.ok()?;
    let mut deactivated = portal.receive_deactivated().await.ok()?;
    let listener = listener.clone();
    let heard = tokio::spawn(async move {
        // The portal and its streams live as long as this task.
        let _portal = portal;
        loop {
            let (id, pressed) = tokio::select! {
                Some(down) = activated.next() => (down.shortcut_id().to_owned(), true),
                Some(up) = deactivated.next() => (up.shortcut_id().to_owned(), false),
                else => return,
            };
            tell(
                &listener,
                Heard::Shortcut {
                    id,
                    pressed,
                    at: Instant::now(),
                },
            );
        }
    });
    Some(Bound { session, heard })
}

/// The desktop's notification server, connected the first time.
#[derive(Default)]
struct Notifications {
    proxy: Option<zbus::Proxy<'static>>,
    /// The server's ID of each notification that shows, by key.
    ids: BTreeMap<String, u32>,
    /// What each notification calls back, by the server's ID.
    chosen: Arc<Mutex<BTreeMap<u32, NotificationChosen>>>,
}

const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";

/// The action key the server sends when you click the notification.
const DEFAULT_ACTION: &str = "default";

impl Notifications {
    async fn proxy(&mut self) -> zbus::Result<zbus::Proxy<'static>> {
        if let Some(proxy) = &self.proxy {
            return Ok(proxy.clone());
        }
        let connection = zbus::Connection::session().await?;
        let proxy = zbus::Proxy::new(
            &connection,
            NOTIFICATIONS,
            NOTIFICATIONS_PATH,
            NOTIFICATIONS,
        )
        .await?;
        // Hear choices and closes for as long as Noura runs.
        let mut invoked = proxy.receive_signal("ActionInvoked").await?;
        let mut closed = proxy.receive_signal("NotificationClosed").await?;
        let chosen = self.chosen.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(message) = invoked.next() => {
                        let Ok((id, action)) = message.body().deserialize::<(u32, String)>() else {
                            continue;
                        };
                        let callback = chosen
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .remove(&id);
                        if let Some(callback) = callback {
                            callback((action != DEFAULT_ACTION).then_some(action));
                        }
                    }
                    Some(message) = closed.next() => {
                        if let Ok((id, _reason)) = message.body().deserialize::<(u32, u32)>() {
                            chosen
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .remove(&id);
                        }
                    }
                    else => return,
                }
            }
        });
        self.proxy = Some(proxy.clone());
        Ok(proxy)
    }

    async fn show(
        &mut self,
        notification: SystemNotification,
        chosen: NotificationChosen,
    ) -> zbus::Result<()> {
        let proxy = self.proxy().await?;
        let replaces = self.ids.get(&notification.key).copied().unwrap_or(0);
        // A click on the notification sends the default action.
        let mut actions: Vec<&str> = vec![DEFAULT_ACTION, ""];
        for (id, label) in &notification.actions {
            actions.push(id);
            actions.push(label);
        }
        let hints: std::collections::HashMap<&str, zbus::Variant<'_>> =
            std::collections::HashMap::new();
        let id: u32 = proxy
            .call(
                "Notify",
                &(
                    "Noura",
                    replaces,
                    "",
                    notification.title.as_str(),
                    notification.body.as_str(),
                    actions,
                    hints,
                    -1_i32,
                ),
            )
            .await?;
        self.ids.insert(notification.key, id);
        self.chosen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id, chosen);
        Ok(())
    }

    async fn withdraw(&mut self, key: &str) {
        let Some(id) = self.ids.remove(key) else {
            return;
        };
        self.chosen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id);
        if let Ok(proxy) = self.proxy().await {
            let _: zbus::Result<()> = proxy.call("CloseNotification", &(id,)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::trigger;

    #[test]
    fn keys_become_xdg_triggers() {
        assert_eq!(trigger("Ctrl+Space"), "CTRL+space");
        assert_eq!(trigger("Ctrl+Alt+D"), "CTRL+ALT+d");
        assert_eq!(trigger("Shift+Cmd+F5"), "SHIFT+LOGO+F5");
    }
}
