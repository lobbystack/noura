//! Generate a public interoperability fixture. The content key is the test-only [7; 32].
use base64::{Engine as _, engine::general_purpose::STANDARD};
use local_core::{
    Result,
    sync::{
        AccessMember, AccessObject, AccessPolicy, AccessTransition, CheckpointContent, DeviceKeys,
        DocumentDescriptor, DocumentMode, EncryptedCheckpoint, FileChange, ObjectKey,
        OperationKind, SyncCredentials, WorkspaceRole,
    },
};
use std::{cell::RefCell, collections::HashMap};
use zeroize::Zeroizing;

#[derive(Default)]
struct Memory(RefCell<HashMap<String, String>>);
impl SyncCredentials for Memory {
    fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
        Ok(Zeroizing::new(self.0.borrow()[reference].clone()))
    }
    fn write(&self, reference: &str, value: &str) -> Result<()> {
        self.0.borrow_mut().insert(reference.into(), value.into());
        Ok(())
    }
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let device = DeviceKeys::create(&Memory::default())?;
    let key = ObjectKey::from_bytes([7; 32]);
    let live = std::env::args().any(|arg| arg == "--live");
    let policy = AccessPolicy::sign(
        "workspace",
        "1",
        None,
        &device,
        vec![AccessMember {
            account_id: "owner".into(),
            role: WorkspaceRole::Owner,
        }],
        vec![AccessObject {
            document: live.then(|| DocumentDescriptor {
                generation: "generation_2".into(),
                mode: DocumentMode::Text,
            }),
            object_id: "object".into(),
            epoch: 2,
            grants: vec![],
            envelopes: vec![
                device
                    .wrap_key(
                        "workspace",
                        "object",
                        2,
                        device.device_id(),
                        &device.recipient(),
                        &key,
                    )?
                    .into(),
            ],
        }],
    )?;
    let content = CheckpointContent {
        version: 1,
        object_id: "object".into(),
        generation: "generation_2".into(),
        content_revision: Some(blake3::hash(b"current content\n").to_hex().to_string()),
        change: FileChange {
            version: 1,
            path: "private/note.txt".into(),
            previous_path: None,
            base_revision: None,
            content: Some(STANDARD.encode(b"current content\n")),
            accepted_revisions: None,
            blob: None,
        },
    };
    let checkpoint = EncryptedCheckpoint::seal(&device, &key, &policy, "4", &content)?;
    let transition = AccessTransition::sign(&device, policy, "4".into(), vec![checkpoint])?;
    let live_operation = if live {
        use local_core::sync::collaboration::{CollaborativeChange, TextDocument};
        let base = TextDocument::fresh_generation("current content\n", "generation_2")?;
        let (_, delta) = base.replace_text("current content 😀\n")?;
        let change = CollaborativeChange {
            version: 1,
            object_id: "object".into(),
            generation: "generation_2".into(),
            updates: vec![delta],
        };
        Some(device.signer().seal_for_document(
            &key,
            "workspace",
            "object",
            device.device_id(),
            (2, "1", "generation_2", OperationKind::Text),
            &serde_json::to_vec(&change)?,
        )?)
    } else {
        None
    };
    println!(
        "{}",
        serde_json::json!({"publicKey": device.signer().public_key(), "digest": transition.digest()?, "transition": transition, "liveOperation":live_operation})
    );
    Ok(())
}
