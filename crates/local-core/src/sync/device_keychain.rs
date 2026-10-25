//! The credential store items that hold a protocol v2 device's secrets
//! (docs/workspace-format/sync-v2.md §7): its Ed25519 signing seed, its
//! X-Wing seed and its session token, in items that never sync or migrate
//! to another machine.
//!
//! On macOS each item is a data protection keychain item, accessible only
//! while the Mac is unlocked and only on this device
//! (`kSecAttrAccessibleWhenUnlockedThisDeviceOnly`), with synchronization
//! off, so neither iCloud Keychain nor a migration or backup restore to
//! another Mac carries it. The data protection keychain needs the app's
//! keychain access group, which the signed app bundle carries. On Windows
//! the items are Credential Manager entries with local-machine persistence,
//! which DPAPI encrypts for this user and no roaming profile carries. On
//! Linux they are Secret Service items, and a
//! copied home folder carries them to another machine: the Secret Service
//! has no item that stays on one device, so the app documents that limit.

use zeroize::Zeroizing;

use crate::Result;
use crate::sync::SyncCredentials;

/// The service name of the v2 device items.
pub const DEVICE_SERVICE: &str = "org.noura.sync.v2";

/// The operating system's credential store for v2 device secrets.
#[derive(Debug, Default, Clone, Copy)]
pub struct DeviceKeychain;

fn unavailable() -> crate::CoreError {
    crate::CoreError::new(
        "sync_credentials_unavailable",
        crate::ErrorCategory::Credential,
        "Sync credentials are unavailable in the operating system credential store",
        "sync",
    )
}

#[cfg(target_os = "macos")]
mod platform {
    use security_framework::access_control::{ProtectionMode, SecAccessControl};
    use security_framework::passwords::{
        delete_generic_password_options, generic_password, set_generic_password_options,
    };
    use security_framework::passwords_options::PasswordOptions;

    use super::{DEVICE_SERVICE, unavailable};
    use crate::Result;

    /// `errSecItemNotFound`.
    const NOT_FOUND: i32 = -25300;

    /// A data protection keychain item that never leaves this Mac.
    fn options(reference: &str) -> PasswordOptions {
        let mut options = PasswordOptions::new_generic_password(DEVICE_SERVICE, reference);
        options.use_protected_keychain();
        options.set_access_synchronized(Some(false));
        options
    }

    pub fn read(reference: &str) -> Result<Option<Vec<u8>>> {
        match generic_password(options(reference)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.code() == NOT_FOUND => Ok(None),
            Err(_) => Err(unavailable()),
        }
    }

    pub fn write(reference: &str, value: &[u8]) -> Result<()> {
        let mut options = options(reference);
        let access = SecAccessControl::create_with_protection(
            Some(ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly),
            0,
        )
        .map_err(|_| unavailable())?;
        options.set_access_control(access);
        set_generic_password_options(value, options).map_err(|_| unavailable())
    }

    pub fn delete(reference: &str) -> Result<()> {
        match delete_generic_password_options(options(reference)) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == NOT_FOUND => Ok(()),
            Err(_) => Err(unavailable()),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{DEVICE_SERVICE, unavailable};
    use crate::Result;

    /// The item for `reference`. Building keyring's entry first sets up the
    /// platform store. On Windows the entry is then rebuilt with local
    /// persistence, so the Credential Manager keeps it on this machine and
    /// a roaming profile doesn't carry it, as keyring's default enterprise
    /// persistence would.
    fn entry(reference: &str) -> Result<keyring::Entry> {
        let entry = keyring::Entry::new(DEVICE_SERVICE, reference).map_err(|_| unavailable())?;
        #[cfg(windows)]
        let entry = on_this_machine(entry, reference)?;
        Ok(entry)
    }

    /// The Credential Manager entry for `reference` with local-machine
    /// persistence, once `_store_ready` set up the store.
    #[cfg(windows)]
    fn on_this_machine(_store_ready: keyring::Entry, reference: &str) -> Result<keyring::Entry> {
        let inner = keyring_core::Entry::new_with_modifiers(
            DEVICE_SERVICE,
            reference,
            &std::collections::HashMap::from([("persistence", "local")]),
        )
        .map_err(|_| unavailable())?;
        Ok(keyring::Entry { inner })
    }

    pub fn read(reference: &str) -> Result<Option<Vec<u8>>> {
        match entry(reference)?.get_password() {
            Ok(value) => Ok(Some(value.into_bytes())),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(unavailable()),
        }
    }

    pub fn write(reference: &str, value: &[u8]) -> Result<()> {
        let text = std::str::from_utf8(value).map_err(|_| unavailable())?;
        entry(reference)?
            .set_password(text)
            .map_err(|_| unavailable())
    }

    pub fn delete(reference: &str) -> Result<()> {
        match entry(reference)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(unavailable()),
        }
    }
}

impl SyncCredentials for DeviceKeychain {
    fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
        self.read_optional(reference)?.ok_or_else(unavailable)
    }

    fn read_optional(&self, reference: &str) -> Result<Option<Zeroizing<String>>> {
        match platform::read(reference)? {
            Some(bytes) => String::from_utf8(bytes)
                .map(|text| Some(Zeroizing::new(text)))
                .map_err(|_| unavailable()),
            None => Ok(None),
        }
    }

    fn write(&self, reference: &str, value: &str) -> Result<()> {
        platform::write(reference, value.as_bytes())
    }

    fn delete(&self, reference: &str) -> Result<()> {
        platform::delete(reference)
    }
}
