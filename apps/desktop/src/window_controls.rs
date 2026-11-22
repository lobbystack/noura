//! The window buttons Noura draws itself on Linux, where its windows have
//! no frame: which ones, and at which end of the title bar row, follow the
//! desktop's button layout, as GNOME's own apps do. The layout comes from
//! the XDG Settings portal, which sandboxed and unsandboxed apps both
//! reach, or else from `gsettings`, and it follows later changes. Without
//! either, the buttons follow GNOME's default, a close button at the end.
//!
//! The layout reads as GNOME writes it, `org.gnome.desktop.wm.preferences
//! button-layout`: the buttons before the colon go at the start, the rest
//! at the end, such as `appmenu:minimize,maximize,close`. Names Noura has
//! no button for, such as `appmenu` and `spacer`, take no room.

use app_ui::{WindowControl, WindowControls};

/// GNOME's default layout.
pub const DEFAULT: &str = "appmenu:close";

/// The buttons `layout` names, at the ends it puts them. Each button shows
/// once, at its first place.
pub fn parse(layout: &str) -> WindowControls {
    let (start, end) = layout.split_once(':').unwrap_or((layout, ""));
    let mut seen = Vec::new();
    let mut side = |names: &str| -> Vec<WindowControl> {
        names
            .split(',')
            .filter_map(|name| match name.trim() {
                "minimize" => Some(WindowControl::Minimize),
                "maximize" => Some(WindowControl::Maximize),
                "close" => Some(WindowControl::Close),
                _ => None,
            })
            .filter(|control| {
                let new = !seen.contains(control);
                seen.push(*control);
                new
            })
            .collect()
    };
    let start = side(start);
    let end = side(end);
    WindowControls { start, end }
}

#[cfg(target_os = "linux")]
pub use linux::watch;

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        sync::{Arc, mpsc},
        time::Duration,
    };

    use app_ui::WindowControls;
    use eframe::egui::{Context, ViewportId, mutex::Mutex};
    use futures::StreamExt;

    const NAMESPACE: &str = "org.gnome.desktop.wm.preferences";
    const KEY: &str = "button-layout";

    /// How long the first window waits for the portal's answer before it
    /// asks `gsettings`.
    const PORTAL_PATIENCE: Duration = Duration::from_millis(500);

    /// The desktop's button layout now, kept up to date as it changes;
    /// `ctx` redraws the windows when it does.
    pub fn watch(ctx: Context) -> Arc<Mutex<WindowControls>> {
        let shared = Arc::new(Mutex::new(super::parse(super::DEFAULT)));
        let (first, answered) = mpsc::channel();
        let kept = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("noura-button-layout".into())
            .spawn(move || listen(first, kept, ctx));
        if let Err(error) = spawned {
            tracing::warn!(%error, "the button layout's thread didn't start");
        }
        let layout = answered
            .recv_timeout(PORTAL_PATIENCE)
            .ok()
            .flatten()
            .or_else(from_gsettings);
        match layout {
            Some(layout) => *shared.lock() = super::parse(&layout),
            None => tracing::info!("no button layout; the windows show GNOME's default"),
        }
        shared
    }

    /// Read the layout from the portal, hand the first answer to `first`,
    /// then keep `shared` up to date.
    fn listen(
        first: mpsc::Sender<Option<String>>,
        shared: Arc<Mutex<WindowControls>>,
        ctx: Context,
    ) {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::warn!(%error, "the button layout's runtime didn't start");
                let _ = first.send(None);
                return;
            }
        };
        runtime.block_on(async move {
            let settings = match ashpd::desktop::settings::Settings::new().await {
                Ok(settings) => settings,
                Err(error) => {
                    tracing::info!(%error, "no settings portal for the button layout");
                    let _ = first.send(None);
                    return;
                }
            };
            let layout = settings.read::<String>(NAMESPACE, KEY).await;
            if let Err(error) = &layout {
                tracing::info!(%error, "the settings portal has no button layout");
            }
            let _ = first.send(layout.ok());
            let changes = match settings
                .receive_setting_changed_with_args::<String>(NAMESPACE, KEY)
                .await
            {
                Ok(changes) => changes,
                Err(error) => {
                    tracing::info!(%error, "the button layout's changes can't be followed");
                    return;
                }
            };
            futures::pin_mut!(changes);
            while let Some(layout) = changes.next().await {
                if let Ok(layout) = layout {
                    *shared.lock() = super::parse(&layout);
                    ctx.request_repaint_of(ViewportId::ROOT);
                }
            }
        });
    }

    /// The layout `gsettings` reads, without its quotes.
    fn from_gsettings() -> Option<String> {
        let output = std::process::Command::new("gsettings")
            .args(["get", NAMESPACE, KEY])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        Some(text.trim().trim_matches('\'').to_owned())
    }
}

#[cfg(test)]
mod tests {
    use app_ui::WindowControl::{Close, Maximize, Minimize};

    use super::*;

    #[test]
    fn the_layout_puts_each_button_at_its_end() {
        for (layout, start, end) in [
            ("appmenu:close", vec![], vec![Close]),
            ("appmenu:minimize,close", vec![], vec![Minimize, Close]),
            (
                "appmenu:minimize,maximize,close",
                vec![],
                vec![Minimize, Maximize, Close],
            ),
            (
                "close,minimize,maximize:",
                vec![Close, Minimize, Maximize],
                vec![],
            ),
            ("close:appmenu", vec![Close], vec![]),
            ("icon,spacer:minimize, close", vec![], vec![Minimize, Close]),
            ("close", vec![Close], vec![]),
            ("close:close", vec![Close], vec![]),
            ("", vec![], vec![]),
        ] {
            assert_eq!(parse(layout), WindowControls { start, end }, "{layout:?}");
        }
    }
}
