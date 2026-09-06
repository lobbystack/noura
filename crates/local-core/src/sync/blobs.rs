//! Streaming attachment encryption. Only ciphertext digests and lengths reach the server.
use std::io::{self, Read, Write};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use bech32::{Bech32, Hrp};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{ObjectKey, invalid};
use crate::Result;

pub const MAX_BLOB_BYTES: u64 = 1024 * 1024 * 1024;

/// This descriptor lives inside an authenticated encrypted operation, never in server metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncryptedBlob {
    pub id: String,
    pub size: u64,
    pub plaintext_size: u64,
    pub revision: String,
}

impl EncryptedBlob {
    pub fn validate(&self) -> Result<()> {
        if self.size == 0
            || self.size > MAX_BLOB_BYTES
            || self.plaintext_size >= self.size
            || !digest(&self.id)
            || !digest(&self.revision)
        {
            return Err(invalid("sync_invalid_blob"));
        }
        Ok(())
    }

    /// Encrypt directly into a caller-owned staging file; the caller must fsync before publishing.
    pub fn encrypt(key: &ObjectKey, mut input: impl Read, output: impl Write) -> Result<Self> {
        let identity = identity(key)?;
        let recipient = identity.to_public();
        let mut output = CipherWriter {
            inner: output,
            hash: Sha256::new(),
            length: 0,
        };
        let encryptor =
            age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
                .map_err(|_| invalid("sync_blob_encrypt_failed"))?;
        let mut stream = encryptor
            .wrap_output(&mut output)
            .map_err(|_| invalid("sync_blob_write_failed"))?;
        let mut hash = blake3::Hasher::new();
        let mut plaintext_size = 0;
        let mut buffer = Zeroizing::new([0u8; 64 * 1024]);
        loop {
            let count = input
                .read(&mut *buffer)
                .map_err(|_| invalid("sync_blob_read_failed"))?;
            if count == 0 {
                break;
            }
            plaintext_size += count as u64;
            if plaintext_size >= MAX_BLOB_BYTES {
                return Err(invalid("sync_blob_too_large"));
            }
            hash.update(&buffer[..count]);
            stream
                .write_all(&buffer[..count])
                .map_err(|_| invalid("sync_blob_write_failed"))?;
        }
        stream
            .finish()
            .map_err(|_| invalid("sync_blob_write_failed"))?;
        let result = Self {
            id: format!("{:x}", output.hash.finalize()),
            size: output.length,
            plaintext_size,
            revision: hash.finalize().to_hex().to_string(),
        };
        result.validate()?;
        Ok(result)
    }

    /// Authenticate ciphertext, length and plaintext revision before committing the staging output.
    /// Callers must discard output on error; authenticated early chunks are not a complete file.
    pub fn decrypt(&self, key: &ObjectKey, input: impl Read, mut output: impl Write) -> Result<()> {
        self.validate()?;
        // age parses its own format. Bound header work before its streaming payload begins.
        let budget = Arc::new(AtomicU64::new(8192));
        let input = BudgetReader {
            inner: input,
            remaining: budget.clone(),
        };
        let decryptor =
            age::Decryptor::new(input).map_err(|_| invalid("sync_blob_decrypt_failed"))?;
        budget.store(self.size, Ordering::Relaxed);
        let identity = identity(key)?;
        let mut stream = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|_| invalid("sync_blob_decrypt_failed"))?;
        let mut hash = blake3::Hasher::new();
        let mut length = 0;
        let mut buffer = Zeroizing::new([0u8; 64 * 1024]);
        loop {
            let count = stream
                .read(&mut *buffer)
                .map_err(|_| invalid("sync_blob_decrypt_failed"))?;
            if count == 0 {
                break;
            }
            length += count as u64;
            if length > self.plaintext_size {
                return Err(invalid("sync_blob_length_mismatch"));
            }
            hash.update(&buffer[..count]);
            output
                .write_all(&buffer[..count])
                .map_err(|_| invalid("sync_blob_write_failed"))?;
        }
        if length != self.plaintext_size || hash.finalize().to_hex().as_str() != self.revision {
            return Err(invalid("sync_blob_revision_mismatch"));
        }
        Ok(())
    }

    /// Check an opaque downloaded file before attempting decryption.
    pub fn verify(&self, mut input: impl Read) -> Result<()> {
        self.validate()?;
        let mut hash = Sha256::new();
        let mut length = 0;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = input
                .read(&mut buffer)
                .map_err(|_| invalid("sync_blob_read_failed"))?;
            if count == 0 {
                break;
            }
            length += count as u64;
            if length > self.size {
                return Err(invalid("sync_blob_length_mismatch"));
            }
            hash.update(&buffer[..count]);
        }
        if length != self.size || format!("{:x}", hash.finalize()) != self.id {
            return Err(invalid("sync_blob_digest_mismatch"));
        }
        Ok(())
    }
}

fn identity(key: &ObjectKey) -> Result<age::x25519::Identity> {
    let seed = Zeroizing::new(blake3::derive_key(
        "noura.sync.blob.x25519.v1",
        key.secret(),
    ));
    let hrp = Hrp::parse("age-secret-key-").map_err(|_| invalid("sync_invalid_key"))?;
    let encoded = Zeroizing::new(
        bech32::encode::<Bech32>(hrp, &*seed)
            .map_err(|_| invalid("sync_invalid_key"))?
            .to_ascii_uppercase(),
    );
    encoded.parse().map_err(|_| invalid("sync_invalid_key"))
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

struct CipherWriter<W> {
    inner: W,
    hash: Sha256,
    length: u64,
}
impl<W: Write> Write for CipherWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.length + bytes.len() as u64 > MAX_BLOB_BYTES {
            return Err(io::Error::other("encrypted attachment exceeds limit"));
        }
        let count = self.inner.write(bytes)?;
        self.hash.update(&bytes[..count]);
        self.length += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
struct BudgetReader<R> {
    inner: R,
    remaining: Arc<AtomicU64>,
}
impl<R: Read> Read for BudgetReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let available = self.remaining.load(Ordering::Relaxed);
        if available == 0 {
            return Err(io::Error::other("attachment read budget exceeded"));
        }
        let limit = bytes.len().min(available as usize);
        let count = self.inner.read(&mut bytes[..limit])?;
        self.remaining.fetch_sub(count as u64, Ordering::Relaxed);
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streaming_blob_rejects_wrong_keys_tampering_truncation_and_wrong_descriptors() {
        let key = ObjectKey::generate();
        for size in [0, 1, 65_536, 65_537, 2 * 1024 * 1024 + 123] {
            let input: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let mut ciphertext = Vec::new();
            let descriptor =
                EncryptedBlob::encrypt(&key, input.as_slice(), &mut ciphertext).unwrap();
            descriptor.verify(ciphertext.as_slice()).unwrap();
            let mut output = Vec::new();
            descriptor
                .decrypt(&key, ciphertext.as_slice(), &mut output)
                .unwrap();
            assert_eq!(input, output);
            assert!(
                descriptor
                    .decrypt(&ObjectKey::generate(), ciphertext.as_slice(), io::sink())
                    .is_err()
            );
            let mut second = Vec::new();
            EncryptedBlob::encrypt(&key, input.as_slice(), &mut second).unwrap();
            assert_ne!(ciphertext, second);
            for position in [0, ciphertext.len() / 2, ciphertext.len() - 1] {
                let mut tampered = ciphertext.clone();
                tampered[position] ^= 1;
                assert!(descriptor.verify(tampered.as_slice()).is_err());
                assert!(
                    descriptor
                        .decrypt(&key, tampered.as_slice(), io::sink())
                        .is_err()
                );
            }
            assert!(
                descriptor
                    .decrypt(&key, &ciphertext[..ciphertext.len() - 1], io::sink())
                    .is_err()
            );
            let mut wrong = descriptor.clone();
            wrong.revision = "0".repeat(64);
            assert!(
                wrong
                    .decrypt(&key, ciphertext.as_slice(), io::sink())
                    .is_err()
            );
            let mut wrong = descriptor.clone();
            wrong.plaintext_size += 1;
            assert!(
                wrong
                    .decrypt(&key, ciphertext.as_slice(), io::sink())
                    .is_err()
            );
        }
    }
}
