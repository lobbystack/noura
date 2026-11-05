//! Where AI provider keys live. The app keeps them in the operating
//! system's credential store, and tests keep them in memory, so no key
//! ever lands in a file Noura writes.

use std::{collections::HashMap, sync::Mutex};

use crate::{CoreError, ErrorCategory, Result};

/// The credential store's service name for AI provider keys.
const AI_SERVICE: &str = "org.noura.ai";

/// A store of secrets by reference. Noura writes only the reference to
/// its settings; the secret stays in the store.
pub trait CredentialStore: Send + Sync {
    /// Keep `secret` under `reference`, replacing what it held.
    fn set(&self, reference: &str, secret: &str) -> Result<()>;

    /// The secret under `reference`.
    fn get(&self, reference: &str) -> Result<String>;

    /// Forget the secret under `reference`. Forgetting one that isn't
    /// there succeeds.
    fn delete(&self, reference: &str) -> Result<()>;
}

/// The operating system's credential store: Keychain on macOS, Credential
/// Manager on Windows and the Secret Service on Linux.
pub struct SystemCredentials;

impl CredentialStore for SystemCredentials {
    fn set(&self, reference: &str, secret: &str) -> Result<()> {
        keyring::Entry::new(AI_SERVICE, reference)
            .and_then(|entry| entry.set_password(secret))
            .map_err(|_| credential_error("ai_credential_set"))
    }

    fn get(&self, reference: &str) -> Result<String> {
        keyring::Entry::new(AI_SERVICE, reference)
            .and_then(|entry| entry.get_password())
            .map_err(|_| credential_error("ai_credential_get"))
    }

    fn delete(&self, reference: &str) -> Result<()> {
        match keyring::Entry::new(AI_SERVICE, reference).and_then(|entry| entry.delete_credential())
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(credential_error("ai_credential_delete")),
        }
    }
}

/// Secrets kept in memory for as long as the store lives, as tests and
/// systems without a credential store use.
#[derive(Default)]
pub struct MemoryCredentials {
    secrets: Mutex<HashMap<String, String>>,
}

impl MemoryCredentials {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every secret the store holds, for tests that look for them.
    pub fn secrets(&self) -> Vec<String> {
        self.secrets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }
}

impl CredentialStore for MemoryCredentials {
    fn set(&self, reference: &str, secret: &str) -> Result<()> {
        self.secrets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(reference.to_owned(), secret.to_owned());
        Ok(())
    }

    fn get(&self, reference: &str) -> Result<String> {
        self.secrets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(reference)
            .cloned()
            .ok_or_else(|| credential_error("ai_credential_get"))
    }

    fn delete(&self, reference: &str) -> Result<()> {
        self.secrets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(reference);
        Ok(())
    }
}

/// A credential store that refuses every request, for clients with no
/// operating system services.
pub struct NoCredentials;

impl CredentialStore for NoCredentials {
    fn set(&self, _reference: &str, _secret: &str) -> Result<()> {
        Err(credential_error("ai_credential_set"))
    }

    fn get(&self, _reference: &str) -> Result<String> {
        Err(credential_error("ai_credential_get"))
    }

    fn delete(&self, _reference: &str) -> Result<()> {
        Ok(())
    }
}

pub(crate) fn credential_error(operation: &str) -> CoreError {
    CoreError::new(
        "credential_store_error",
        ErrorCategory::Credential,
        "The operating system credential store operation failed",
        operation,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_credentials_keep_replace_and_forget_secrets() {
        let store = MemoryCredentials::new();
        store.set("credential_1", "first").unwrap();
        store.set("credential_1", "second").unwrap();
        assert_eq!(store.get("credential_1").unwrap(), "second");
        store.delete("credential_1").unwrap();
        store.delete("credential_1").unwrap();
        let error = store.get("credential_1").unwrap_err();
        assert_eq!(error.code, "credential_store_error");
        assert!(!error.message.contains("second"));
    }
}
