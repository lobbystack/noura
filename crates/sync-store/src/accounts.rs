//! Accounts and sign-in on the server (§7, §14, §29): signed-up accounts,
//! passkeys, web sessions of the account pages, magic links, sign-ups that
//! wait for their email, and passkey ceremonies. Every token is stored only
//! as its BLAKE3 hash, and no session carries an expiry. A magic link, a
//! waiting sign-up and a ceremony work once, and the store deletes each
//! when it's used or 10 minutes after it issued it.

use sync_protocol::crypto::{hash, random_array};
use sync_protocol::types::{Digest, Id};

use crate::chains::SIGN_IN_TTL;
use crate::{
    Backend, CeremonyKind, CeremonyRow, Error, PasskeyRow, PendingPasskeyRow, ReadTxn, Result,
    Store, WriteTxn,
};

/// The length of a sign-in token: a web session, a magic link or a sign-up
/// confirmation.
pub const SIGN_IN_TOKEN_LEN: usize = 32;

/// A sign-in token's 32 random bytes, which only the browser or the email
/// holds; the store keeps their hash.
pub type SignInToken = [u8; SIGN_IN_TOKEN_LEN];

fn new_token() -> Result<SignInToken> {
    random_array().map_err(Error::internal)
}

fn token_hash(token: &SignInToken) -> Digest {
    hash(token)
}

impl<B: Backend> Store<B> {
    /// Issues a magic link for `email` and returns its token.
    pub async fn issue_magic_link(&self, email: &str) -> Result<SignInToken> {
        let token = new_token()?;
        let hash = token_hash(&token);
        let email = email.to_owned();
        let expires_at = self.clock().now() + SIGN_IN_TTL;
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.insert_magic_link(hash, &email, expires_at).await })
            }))
            .await?;
        Ok(token)
    }

    /// Deletes a magic link, such as one whose email failed to send.
    pub async fn revoke_magic_link(&self, token: &SignInToken) -> Result<()> {
        let hash = token_hash(token);
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.take_magic_link(hash).await.map(|_| ()) })
            }))
            .await
    }

    /// Uses a magic link once: deletes it and, while it's current, signs up
    /// its email when no account has it yet and starts a web session. The
    /// account's ID derives from the email (§14). Returns the account and
    /// the web session's token, or none for an unknown, used or expired
    /// link.
    pub async fn sign_in_with_magic_link(
        &self,
        token: &SignInToken,
    ) -> Result<Option<(Id, SignInToken)>> {
        let hash = token_hash(token);
        let now = self.clock().now();
        let session = new_token()?;
        let session_hash = token_hash(&session);
        let secret = self.secret_for_ids()?;
        let signed_in = self
            .backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    let Some((email, expires_at)) = txn.take_magic_link(hash).await? else {
                        return Ok(None);
                    };
                    if expires_at <= now {
                        return Ok(None);
                    }
                    let account = sync_protocol::ids::account_id(&secret, &email);
                    txn.insert_account(account, &normalized(&email)).await?;
                    txn.insert_web_session(session_hash, account).await?;
                    Ok(Some(account))
                })
            }))
            .await?;
        Ok(signed_in.map(|account| (account, session)))
    }

    /// Holds a passkey that `email` registered to sign up until the email
    /// confirms it, and returns the confirmation token.
    pub async fn hold_passkey(
        &self,
        email: &str,
        credential_id: Vec<u8>,
        passkey: Vec<u8>,
    ) -> Result<SignInToken> {
        let token = new_token()?;
        let hash = token_hash(&token);
        let row = PendingPasskeyRow {
            email: email.to_owned(),
            credential_id,
            passkey,
            expires_at: self.clock().now() + SIGN_IN_TTL,
        };
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.insert_pending_passkey(hash, &row).await })
            }))
            .await?;
        Ok(token)
    }

    /// Deletes a held passkey, such as one whose email failed to send.
    pub async fn drop_held_passkey(&self, token: &SignInToken) -> Result<()> {
        let hash = token_hash(token);
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.take_pending_passkey(hash).await.map(|_| ()) })
            }))
            .await
    }

    /// Confirms a held passkey once: deletes it and, while it's current,
    /// signs up its email when no account has it yet, adds the passkey to
    /// that account and starts a web session. Returns none for an unknown,
    /// used or expired confirmation, or a passkey another account holds.
    pub async fn confirm_passkey(&self, token: &SignInToken) -> Result<Option<(Id, SignInToken)>> {
        let hash = token_hash(token);
        let now = self.clock().now();
        let session = new_token()?;
        let session_hash = token_hash(&session);
        let secret = self.secret_for_ids()?;
        let confirmed = self
            .backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    let Some(row) = txn.take_pending_passkey(hash).await? else {
                        return Ok(None);
                    };
                    if row.expires_at <= now {
                        return Ok(None);
                    }
                    let account = sync_protocol::ids::account_id(&secret, &row.email);
                    txn.insert_account(account, &normalized(&row.email)).await?;
                    let added = txn
                        .insert_passkey(&PasskeyRow {
                            credential_id: row.credential_id,
                            account,
                            passkey: row.passkey,
                        })
                        .await?;
                    if !added {
                        return Ok(None);
                    }
                    txn.insert_web_session(session_hash, account).await?;
                    Ok(Some(account))
                })
            }))
            .await?;
        Ok(confirmed.map(|account| (account, session)))
    }

    /// Starts a web session for a signed-up account and returns its token.
    pub async fn start_web_session(&self, account: Id) -> Result<SignInToken> {
        let token = new_token()?;
        let hash = token_hash(&token);
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.insert_web_session(hash, account).await })
            }))
            .await?;
        Ok(token)
    }

    /// The account of a web session's token.
    pub async fn web_session(&self, token: &SignInToken) -> Result<Option<Id>> {
        let hash = token_hash(token);
        self.backend()
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.web_session(hash).await })
            }))
            .await
    }

    /// Ends a web session: signing out of the account pages.
    pub async fn end_web_session(&self, token: &SignInToken) -> Result<()> {
        let hash = token_hash(token);
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.delete_web_session(hash).await })
            }))
            .await
    }

    /// The email of a signed-up account.
    pub async fn account_email(&self, account: Id) -> Result<Option<String>> {
        self.backend()
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.account_email(account).await })
            }))
            .await
    }

    /// Stores a ceremony's state and returns its new ID.
    pub async fn begin_ceremony(
        &self,
        kind: CeremonyKind,
        account: Option<Id>,
        email: Option<String>,
        state: Vec<u8>,
    ) -> Result<Id> {
        let row = CeremonyRow {
            id: Id::random().map_err(Error::internal)?,
            kind,
            account,
            email,
            state,
            expires_at: self.clock().now() + SIGN_IN_TTL,
        };
        let id = row.id;
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.insert_ceremony(&row).await })
            }))
            .await?;
        Ok(id)
    }

    /// Takes a ceremony of `kind` once: deletes it and returns it while it's
    /// current.
    pub async fn finish_ceremony(&self, id: Id, kind: CeremonyKind) -> Result<Option<CeremonyRow>> {
        let now = self.clock().now();
        let row = self
            .backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.take_ceremony(id).await })
            }))
            .await?;
        Ok(row.filter(|row| row.kind == kind && row.expires_at > now))
    }

    /// The account's passkeys.
    pub async fn passkeys(&self, account: Id) -> Result<Vec<PasskeyRow>> {
        self.backend()
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.passkeys(account).await })
            }))
            .await
    }

    /// The passkey with this credential ID.
    pub async fn passkey(&self, credential_id: Vec<u8>) -> Result<Option<PasskeyRow>> {
        self.backend()
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.passkey(&credential_id).await })
            }))
            .await
    }

    /// Adds a passkey to a signed-up account; returns false when another
    /// passkey holds its credential ID.
    pub async fn add_passkey(&self, row: PasskeyRow) -> Result<bool> {
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.insert_passkey(&row).await })
            }))
            .await
    }

    /// Replaces a passkey's stored form after a sign-in moved its counter.
    pub async fn update_passkey(&self, credential_id: Vec<u8>, passkey: Vec<u8>) -> Result<()> {
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.update_passkey(&credential_id, &passkey).await })
            }))
            .await
    }

    /// Deletes every magic link, held passkey, ceremony and enrollment
    /// challenge 10 minutes after the store issued it. The server runs this
    /// every 10 seconds.
    pub async fn delete_expired(&self) -> Result<u64> {
        let now = self.clock().now();
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.delete_expired(now).await })
            }))
            .await
    }

    fn secret_for_ids(&self) -> Result<sync_protocol::types::SecretKey> {
        self.secret
            .clone()
            .ok_or_else(|| Error::internal("the store has no server secret"))
    }
}

/// The form of an email the store keeps: ASCII letters lowercased, as the
/// account ID derives from it (§14).
pub fn normalized(email: &str) -> String {
    sync_protocol::ids::normalize_email(email.trim())
}
