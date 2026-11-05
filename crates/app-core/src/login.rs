//! Open at login: whether Noura opens when you sign in to this computer.
//! The system keeps that state, so Noura reads it from the system each
//! time Settings opens instead of remembering it.

use std::io;

/// The system's list of apps that open at login. The desktop app adds
/// and removes Noura there; tests pass a fake.
pub trait LoginItem: Send + Sync {
    /// Whether Noura is on the list now.
    fn is_enabled(&self) -> io::Result<bool>;

    /// Add Noura to the list, or remove it. An error leaves the list as
    /// it was.
    fn set_enabled(&self, enabled: bool) -> io::Result<()>;
}

/// No list to change, as on a system Noura can't add itself to.
pub struct NoLoginItem;

impl LoginItem for NoLoginItem {
    fn is_enabled(&self) -> io::Result<bool> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no login items here",
        ))
    }

    fn set_enabled(&self, _enabled: bool) -> io::Result<()> {
        self.is_enabled().map(|_| ())
    }
}

/// The Open at login switch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenAtLogin {
    /// Whether Noura opens at login, as the system says. `None` when the
    /// system can't say, which turns the switch off limits.
    pub enabled: Option<bool>,
    /// Why the system can't say, or why the last change didn't happen.
    pub problem: Option<String>,
}

impl OpenAtLogin {
    /// The switch as the system has it now.
    pub(crate) fn read(item: &dyn LoginItem) -> Self {
        match item.is_enabled() {
            Ok(enabled) => Self {
                enabled: Some(enabled),
                problem: None,
            },
            Err(error) => {
                tracing::warn!(%error, "the login items could not be read");
                Self {
                    enabled: None,
                    problem: Some(refusal(&error, None)),
                }
            }
        }
    }

    /// Ask the system to add or remove Noura, then show what it did. A
    /// refusal leaves the switch as it was and says why.
    pub(crate) fn set(&mut self, item: &dyn LoginItem, enabled: bool) {
        if self.enabled.is_none() {
            return;
        }
        match item.set_enabled(enabled) {
            Ok(()) => {
                // The system has the last word; trust what it says now.
                self.enabled = Some(item.is_enabled().unwrap_or(enabled));
                self.problem = None;
            }
            Err(error) => {
                tracing::warn!(%error, enabled, "the login items could not be changed");
                self.problem = Some(refusal(&error, Some(enabled)));
            }
        }
    }
}

/// Why the system wouldn't read or change the list, in words. The
/// system's own message stays in the log.
fn refusal(error: &io::Error, enabling: Option<bool>) -> String {
    match (error.kind(), enabling) {
        (io::ErrorKind::PermissionDenied, _) => {
            "Noura doesn't have permission to change the apps that open at login.".to_owned()
        }
        (io::ErrorKind::Unsupported, _) | (_, None) => {
            "This computer doesn't let Noura change the apps that open at login.".to_owned()
        }
        (_, Some(true)) => {
            "Your computer refused to add Noura to the apps that open at login.".to_owned()
        }
        (_, Some(false)) => {
            "Your computer refused to remove Noura from the apps that open at login.".to_owned()
        }
    }
}
