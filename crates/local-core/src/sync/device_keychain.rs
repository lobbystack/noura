//! The credential store items that hold a protocol v2 device's secrets
//! (docs/workspace-format/sync-v2.md §7): its Ed25519 signing seed, its
//! X-Wing seed and its session token, in items that never sync or migrate
//! to another machine.
//!
//! On macOS each item is a data protection keychain item, accessible only
//! while the Mac is unlocked and only on this device
//! (`kSecAttrAccessibleWhenUnlockedThisDeviceOnly`), with synchronization
//! off, so neither iCloud Keychain nor a migration or backup restore to
//! another Mac carries it. That keychain needs a keychain access group,
//! which only a build with a provisioning profile holds. Any other build
//! gets `errSecMissingEntitlement` and keeps the item in the login keychain
//! instead, with the access list that trusts the app that made it. A login
//! keychain item has no ThisDeviceOnly class, so a migration or a backup
//! restore can carry it to another Mac. The data protection keychain stays
//! the first choice, so a build that gains the entitlement moves there
//! without code changes: each write lands there and removes the login item.
//! On Windows
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

/// One keychain, whose failures are the `OSStatus` it answered.
#[cfg(any(target_os = "macos", test))]
trait Keychain {
    fn read(&self, reference: &str) -> std::result::Result<Option<Vec<u8>>, i32>;
    fn write(&self, reference: &str, value: &[u8]) -> std::result::Result<(), i32>;
    /// Succeeds when the item is gone, whether or not it was there.
    fn delete(&self, reference: &str) -> std::result::Result<(), i32>;
}

/// The data protection keychain first, and the login keychain when this
/// build lacks the entitlement it needs. Any other failure stays a failure:
/// the login keychain never stands in for a locked or refusing store.
#[cfg(any(target_os = "macos", test))]
struct Fallback<P, L> {
    protected: P,
    login: L,
    /// Whether the diagnostic went to the log.
    noted: std::sync::atomic::AtomicBool,
}

#[cfg(any(target_os = "macos", test))]
impl<P: Keychain, L: Keychain> Fallback<P, L> {
    /// `errSecMissingEntitlement`.
    const MISSING_ENTITLEMENT: i32 = -34018;

    fn note_fallback(&self) {
        if !self.noted.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!(
                "this build has no keychain access group, so sync keys stay in the login keychain"
            );
        }
    }

    fn read(&self, reference: &str) -> Result<Option<Vec<u8>>> {
        match self.protected.read(reference) {
            Ok(Some(bytes)) => return Ok(Some(bytes)),
            Ok(None) => {}
            Err(Self::MISSING_ENTITLEMENT) => self.note_fallback(),
            Err(_) => return Err(unavailable()),
        }
        self.login.read(reference).map_err(|_| unavailable())
    }

    fn write(&self, reference: &str, value: &[u8]) -> Result<()> {
        match self.protected.write(reference, value) {
            Ok(()) => {
                // The new value is durable, so an old login item only
                // lingers. The next write tries to remove it again.
                if self.login.delete(reference).is_err() {
                    tracing::warn!("an earlier login keychain item stayed");
                }
                Ok(())
            }
            Err(Self::MISSING_ENTITLEMENT) => {
                self.note_fallback();
                self.login
                    .write(reference, value)
                    .map_err(|_| unavailable())
            }
            Err(_) => Err(unavailable()),
        }
    }

    fn delete(&self, reference: &str) -> Result<()> {
        let protected = self.protected.delete(reference);
        let login = self.login.delete(reference);
        match (protected, login) {
            (Ok(()) | Err(Self::MISSING_ENTITLEMENT), Ok(())) => Ok(()),
            _ => Err(unavailable()),
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use security_framework::access_control::{ProtectionMode, SecAccessControl};
    use security_framework::passwords::{
        delete_generic_password_options, generic_password, set_generic_password_options,
    };
    use security_framework::passwords_options::PasswordOptions;

    use super::{DEVICE_SERVICE, Fallback, Keychain};
    use crate::Result;

    /// `errSecItemNotFound`.
    const NOT_FOUND: i32 = -25300;

    /// The data protection keychain: an item that never leaves this Mac.
    struct Protected;

    /// The login keychain: an item whose access list trusts this app.
    struct Login;

    fn protected_options(reference: &str) -> PasswordOptions {
        let mut options = PasswordOptions::new_generic_password(DEVICE_SERVICE, reference);
        options.use_protected_keychain();
        options.set_access_synchronized(Some(false));
        options
    }

    fn read_item(options: PasswordOptions) -> std::result::Result<Option<Vec<u8>>, i32> {
        match generic_password(options) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.code() == NOT_FOUND => Ok(None),
            Err(error) => Err(error.code()),
        }
    }

    fn delete_item(options: PasswordOptions) -> std::result::Result<(), i32> {
        match delete_generic_password_options(options) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == NOT_FOUND => Ok(()),
            Err(error) => Err(error.code()),
        }
    }

    impl Keychain for Protected {
        fn read(&self, reference: &str) -> std::result::Result<Option<Vec<u8>>, i32> {
            read_item(protected_options(reference))
        }

        fn write(&self, reference: &str, value: &[u8]) -> std::result::Result<(), i32> {
            let mut options = protected_options(reference);
            let access = SecAccessControl::create_with_protection(
                Some(ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly),
                0,
            )
            .map_err(|error| error.code())?;
            options.set_access_control(access);
            set_generic_password_options(value, options).map_err(|error| error.code())
        }

        fn delete(&self, reference: &str) -> std::result::Result<(), i32> {
            delete_item(protected_options(reference))
        }
    }

    impl Keychain for Login {
        fn read(&self, reference: &str) -> std::result::Result<Option<Vec<u8>>, i32> {
            read_item(PasswordOptions::new_generic_password(
                DEVICE_SERVICE,
                reference,
            ))
        }

        fn write(&self, reference: &str, value: &[u8]) -> std::result::Result<(), i32> {
            let options = PasswordOptions::new_generic_password(DEVICE_SERVICE, reference);
            set_generic_password_options(value, options).map_err(|error| error.code())
        }

        fn delete(&self, reference: &str) -> std::result::Result<(), i32> {
            delete_item(PasswordOptions::new_generic_password(
                DEVICE_SERVICE,
                reference,
            ))
        }
    }

    static KEYCHAINS: Fallback<Protected, Login> = Fallback {
        protected: Protected,
        login: Login,
        noted: std::sync::atomic::AtomicBool::new(false),
    };

    pub fn read(reference: &str) -> Result<Option<Vec<u8>>> {
        KEYCHAINS.read(reference)
    }

    pub fn write(reference: &str, value: &[u8]) -> Result<()> {
        KEYCHAINS.write(reference, value)
    }

    pub fn delete(reference: &str) -> Result<()> {
        KEYCHAINS.delete(reference)
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    /// A keychain in memory that answers `refusal` to everything, if set.
    #[derive(Default)]
    struct Memory {
        items: Mutex<HashMap<String, Vec<u8>>>,
        refusal: Option<i32>,
    }

    impl Memory {
        fn refusing(status: i32) -> Self {
            Self {
                refusal: Some(status),
                ..Self::default()
            }
        }

        fn holds(&self, reference: &str) -> bool {
            self.items.lock().unwrap().contains_key(reference)
        }
    }

    impl Keychain for Memory {
        fn read(&self, reference: &str) -> std::result::Result<Option<Vec<u8>>, i32> {
            self.refusal.map_or(Ok(()), Err)?;
            Ok(self.items.lock().unwrap().get(reference).cloned())
        }

        fn write(&self, reference: &str, value: &[u8]) -> std::result::Result<(), i32> {
            self.refusal.map_or(Ok(()), Err)?;
            let mut items = self.items.lock().unwrap();
            items.insert(reference.to_owned(), value.to_owned());
            Ok(())
        }

        fn delete(&self, reference: &str) -> std::result::Result<(), i32> {
            self.refusal.map_or(Ok(()), Err)?;
            self.items.lock().unwrap().remove(reference);
            Ok(())
        }
    }

    fn keychains(protected: Memory, login: Memory) -> Fallback<Memory, Memory> {
        Fallback {
            protected,
            login,
            noted: AtomicBool::new(false),
        }
    }

    const MISSING_ENTITLEMENT: i32 = -34018;

    #[test]
    fn a_build_without_the_entitlement_keeps_items_in_the_login_keychain() {
        let store = keychains(Memory::refusing(MISSING_ENTITLEMENT), Memory::default());
        store.write("seed", b"secret").unwrap();
        assert!(store.login.holds("seed"));
        assert!(
            store.noted.load(Ordering::Relaxed),
            "the diagnostic went out"
        );
        assert_eq!(store.read("seed").unwrap().unwrap(), b"secret");
        assert!(store.read("other").unwrap().is_none());
        store.delete("seed").unwrap();
        assert!(!store.login.holds("seed"));
        assert!(store.read("seed").unwrap().is_none());
    }

    #[test]
    fn a_build_with_the_entitlement_uses_the_data_protection_keychain_alone() {
        let store = keychains(Memory::default(), Memory::default());
        store.write("seed", b"secret").unwrap();
        assert!(store.protected.holds("seed"));
        assert!(!store.login.holds("seed"));
        assert!(!store.noted.load(Ordering::Relaxed), "nothing fell back");
        assert_eq!(store.read("seed").unwrap().unwrap(), b"secret");
    }

    /// A signed build that gains the entitlement still finds what an
    /// earlier build stored, and its next write moves the item.
    #[test]
    fn a_new_entitlement_finds_login_items_and_moves_them_on_write() {
        let store = keychains(Memory::default(), Memory::default());
        store.login.write("seed", b"old").unwrap();
        assert_eq!(store.read("seed").unwrap().unwrap(), b"old");
        store.write("seed", b"new").unwrap();
        assert!(!store.login.holds("seed"));
        assert_eq!(store.read("seed").unwrap().unwrap(), b"new");
    }

    #[test]
    fn any_other_failure_stays_unavailable_and_reaches_no_login_item() {
        const LOCKED: i32 = -25308;
        let store = keychains(Memory::refusing(LOCKED), Memory::default());
        store.login.write("seed", b"secret").unwrap();
        for error in [
            store.read("seed").unwrap_err(),
            store.write("seed", b"new").unwrap_err(),
        ] {
            assert_eq!(error.code, "sync_credentials_unavailable");
        }
        assert_eq!(store.login.read("seed").unwrap().unwrap(), b"secret");
        assert!(!store.noted.load(Ordering::Relaxed));
    }

    /// The real keychains with a throwaway value. It writes to your login
    /// keychain, so it runs only when you ask for it.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "writes a throwaway item to the real keychain"]
    fn the_real_store_holds_an_item_in_this_build() {
        let reference = "noura-device-keychain-test";
        let store = DeviceKeychain;
        store.write(reference, "throwaway").unwrap();
        let read = store.read(reference).map(|value| value.to_string());
        store.delete(reference).unwrap();
        assert_eq!(read.unwrap(), "throwaway");
        assert!(store.read_optional(reference).unwrap().is_none());
    }
}
