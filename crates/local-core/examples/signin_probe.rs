//! Integration-test client; credentials are held in memory and never printed.
use local_core::{
    Result, WorkspaceEngine,
    sync::{SyncAccountPoll, SyncAccountService, SyncCredentials, WorkspaceSyncCoordinator},
};
use std::{
    cell::RefCell,
    collections::HashMap,
    io::{Read, Write},
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Default)]
struct Memory(RefCell<HashMap<String, Zeroizing<String>>>);
impl SyncCredentials for Memory {
    fn read_optional(&self, reference: &str) -> Result<Option<Zeroizing<String>>> {
        Ok(self.0.borrow().get(reference).cloned())
    }
    fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
        self.0.borrow().get(reference).cloned().ok_or_else(|| {
            local_core::CoreError::validation(
                "missing_credential",
                "Test credential missing",
                "test",
            )
        })
    }
    fn write(&self, reference: &str, value: &str) -> Result<()> {
        self.0
            .borrow_mut()
            .insert(reference.into(), Zeroizing::new(value.into()));
        Ok(())
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut origin = String::new();
    std::io::stdin().read_to_string(&mut origin)?;
    let store = Memory::default();
    let mut signin = SyncAccountService::default();
    let info = signin.begin(origin.trim(), &store).await?;
    println!("{}", serde_json::to_string(&info)?);
    std::io::stdout().flush()?;
    for _ in 0..60 {
        match signin.poll(&store).await? {
            SyncAccountPoll::Pending { retry_after } => {
                tokio::time::sleep(Duration::from_secs(u64::from(retry_after))).await
            }
            SyncAccountPoll::Connected { account } => {
                let mut restored = SyncAccountService::default();
                assert_eq!(
                    restored.current(&store)?.unwrap().device_id,
                    account.device_id
                );
                let connection = restored.connection(&store)?.unwrap();
                let transport = connection.transport(&store)?;
                let directory = tempfile::TempDir::new()?;
                let root = directory.path().join("workspace");
                let app_data = directory.path().join("app");
                let engine =
                    WorkspaceEngine::create_with_app_data(&root, "Desktop sync", &app_data)?;
                std::fs::write(root.join("external.txt"), b"canonical external edit")?;
                WorkspaceSyncCoordinator::enable(&engine, &connection, &store).await?;
                // Lost creation responses are safe to retry without changing ownership.
                WorkspaceSyncCoordinator::enable(&engine, &connection, &store).await?;
                let first = WorkspaceSyncCoordinator::pass(&engine, &connection, &store).await?;
                assert_eq!(first.uploaded, 1);
                assert_eq!(engine.sync_status()?.pending, 0);
                let workspace_id = engine.manifest().id;
                engine.sync_pause(true)?;
                drop(engine);
                let engine = WorkspaceEngine::open_with_app_data(&root, &app_data)?;
                assert!(!engine.sync_status()?.enabled);
                let paused = WorkspaceSyncCoordinator::pass(&engine, &connection, &store)
                    .await
                    .unwrap_err();
                assert_eq!(paused.code, "sync_paused");
                engine.sync_pause(false)?;
                std::fs::rename(root.join("external.txt"), root.join("moved.txt"))?;
                assert_eq!(
                    WorkspaceSyncCoordinator::pass(&engine, &connection, &store)
                        .await?
                        .uploaded,
                    1
                );
                std::fs::remove_file(root.join("moved.txt"))?;
                assert_eq!(
                    WorkspaceSyncCoordinator::pass(&engine, &connection, &store)
                        .await?
                        .uploaded,
                    1
                );
                assert_eq!(engine.sync_status()?.pending, 0);
                assert_eq!(engine.sync_status()?.conflicts, 0);
                restored.disconnect(&store).await?;
                assert!(restored.current(&store)?.is_none());
                let error = transport
                    .create_workspace(&format!("workspace_{}", uuid::Uuid::new_v4()))
                    .await
                    .unwrap_err();
                assert_eq!(error.code, "sync_sign_in_required");
                println!(
                    "{}",
                    serde_json::json!({"connected":true,"deviceId":connection.device_id,"workspaceId":workspace_id,"coordinatorRestart":true})
                );
                return Ok(());
            }
        }
    }
    Err("device sign-in timed out".into())
}
