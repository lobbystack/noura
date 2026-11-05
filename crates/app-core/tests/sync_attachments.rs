//! Attachments at seam 4 (sync-v2.md §21): files above the inline limit
//! sync between two desktops as encrypted blobs. The server stores only
//! padded ciphertext; a move inside the domain reuses the blob; a corrupt
//! download leaves the old file alone; and two edits of one attachment keep
//! both versions, as text files do.

mod sync_support;

use sync_protocol::blob::is_valid_blob_ciphertext_len;
use sync_protocol::types::to_hex;
use sync_support::{Pair, pair, rounds};

/// A marker that appears only in the plaintext of the test files.
const MARKER: &[u8] = b"ATTACHMENT-PLAINTEXT-MARKER";

/// `len` bytes that start with `head` and repeat the marker.
fn attachment(head: &[u8], len: usize, seed: u8) -> Vec<u8> {
    let mut bytes = head.to_vec();
    let mut i: usize = seed.into();
    while bytes.len() < len {
        if i.is_multiple_of(4096) {
            bytes.extend_from_slice(MARKER);
        } else {
            bytes.push((i.wrapping_mul(31) % 251) as u8);
        }
        i += 1;
    }
    bytes.truncate(len);
    bytes
}

fn holds_marker(bytes: &[u8]) -> bool {
    bytes.windows(MARKER.len()).any(|w| w == MARKER)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pdf_and_an_image_sync_as_blobs_and_a_move_uploads_nothing() {
    let Pair {
        server,
        proxy: _proxy,
        laptop: _laptop,
        mut service,
        a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = pair().await;
    let pdf = attachment(b"%PDF-1.7\n", 700_000, 1);
    let png = attachment(b"\x89PNG\r\n\x1a\n", 1_500_000, 2);
    a.write("Docs/Report.pdf", &pdf);
    a.write("Images/Photo.png", &png);
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert!(b.files()["Docs/Report.pdf"] == pdf);
    assert!(b.files()["Images/Photo.png"] == png);
    assert!(a.files() == b.files());

    // The server holds only padded ciphertext: one 1 MiB segment for the
    // PDF and two for the image, none with the marker.
    let segments = server.blob_files();
    assert_eq!(segments.len(), 3);
    let mut lengths = std::collections::BTreeMap::new();
    for file in &segments {
        let bytes = std::fs::read(file).expect("a segment");
        assert!(!holds_marker(&bytes), "{} holds plaintext", file.display());
        let blob = file.parent().expect("a blob folder").to_owned();
        *lengths.entry(blob).or_insert(0u64) += bytes.len() as u64;
    }
    assert_eq!(lengths.len(), 2);
    for length in lengths.values() {
        assert!(is_valid_blob_ciphertext_len(*length));
    }
    for file in server.stored_files().await {
        let bytes = std::fs::read(&file).unwrap_or_default();
        assert!(!holds_marker(&bytes), "{} holds plaintext", file.display());
    }

    // A move inside the domain reuses the blob_ref and uploads nothing.
    std::fs::create_dir_all(a.path("Archive")).expect("a folder");
    std::fs::rename(a.path("Docs/Report.pdf"), a.path("Archive/Report.pdf")).expect("moved");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert_eq!(server.blob_files().len(), 3, "the move uploaded nothing");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert!(b.files()["Archive/Report.pdf"] == pdf);
    assert!(!b.files().contains_key("Docs/Report.pdf"));

    // An edit on the other desktop comes back as a new blob.
    let edited = attachment(b"%PDF-1.7\n", 800_000, 9);
    b.write("Archive/Report.pdf", &edited);
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert!(a.files()["Archive/Report.pdf"] == edited);
    assert!(a.files() == b.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_corrupt_download_leaves_the_old_file_and_the_cursor() {
    let Pair {
        server: _server,
        proxy,
        laptop: _laptop,
        mut service,
        a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = pair().await;
    let first = attachment(b"%PDF-1.7\n", 600_000, 3);
    a.write("Report.pdf", &first);
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert!(b.files()["Report.pdf"] == first);

    let second = attachment(b"%PDF-1.7\n", 650_000, 4);
    b.write("Report.pdf", &second);
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    // Every blob byte the laptop downloads through the proxy comes back
    // with its last byte flipped.
    proxy.tamper(Box::new(|path: &str, mut bytes: Vec<u8>| {
        if path.contains("/blobs/")
            && let Some(byte) = bytes.last_mut()
        {
            *byte ^= 1;
        }
        bytes
    }));
    assert!(sync_a.round(&mut service, 0).await.is_err());
    assert!(a.files()["Report.pdf"] == first, "the old file stays");

    // Once the downloads are honest, the same records apply.
    proxy.honest();
    sync_a.round(&mut service, 0).await.expect("a round");
    assert!(a.files()["Report.pdf"] == second);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_edits_of_one_attachment_keep_both_versions() {
    let Pair {
        server: _server,
        proxy: _proxy,
        laptop: _laptop,
        mut service,
        a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = pair().await;
    let original = attachment(b"\x89PNG\r\n\x1a\n", 480_000, 7);
    a.write("Photo.png", &original);
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert!(b.files()["Photo.png"] == original);

    let laptop_version = attachment(b"\x89PNG\r\n\x1a\n", 500_000, 5);
    let desktop_version = attachment(b"\x89PNG\r\n\x1a\n", 520_000, 6);
    a.write("Photo.png", &laptop_version);
    b.write("Photo.png", &desktop_version);
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert!(
        a.files()["Photo.png"] == laptop_version,
        "neither file is overwritten"
    );
    assert!(b.files()["Photo.png"] == desktop_version);
    assert_eq!(sync_a.status().conflicts, 1);
    assert_eq!(sync_b.status().conflicts, 1);
    let conflict = sync_a.conflicts().pop().expect("the conflict");
    assert_eq!(conflict.branches.len(), 2);
    // Each device keeps the other's version in its review store.
    let kept = |folder: &sync_support::Folder, version: &[u8]| {
        let mut stack = vec![folder.path(".noura/sync")];
        while let Some(next) = stack.pop() {
            for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
                if entry.path().is_dir() {
                    stack.push(entry.path());
                } else if std::fs::read(entry.path()).is_ok_and(|b| b == version) {
                    return true;
                }
            }
        }
        false
    };
    assert!(kept(&a, &desktop_version));
    assert!(kept(&b, &laptop_version));
}

/// Whether any file under `dir` holds `needle`.
fn holds(dir: &std::path::Path, needle: &[u8]) -> bool {
    std::fs::read_dir(dir).unwrap().flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            holds(&path, needle)
        } else {
            // Windows refuses a file another thread holds locked, as an
            // open index.
            std::fs::read(&path)
                .is_ok_and(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
        }
    })
}

/// No blob key sits in the folder, in the clear or as hexadecimal, while a
/// record waits in the outbox, after it went, or on the device that pulled
/// it: a copy of the folder opens no blob of the server's (§15, §21).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_blob_key_rests_in_the_folder() {
    let mut pair = pair().await;
    pair.a.write("Scan.pdf", &vec![7; 400_000]);
    pair.sync_a.capture().expect("signed into the outbox");
    let check = |sync: &app_core::sync::workspace::WorkspaceSync, folder: &sync_support::Folder| {
        let keys = sync.blob_keys_for_testing();
        assert!(!keys.is_empty(), "a key to look for");
        let journals = folder.path(".noura/sync");
        for key in keys {
            let raw = key.expose();
            assert!(!holds(&journals, raw));
            assert!(!holds(&journals, to_hex(raw).as_bytes()));
        }
    };
    check(&pair.sync_a, &pair.a);
    rounds(&mut pair).await;
    assert_eq!(pair.a.files(), pair.b.files());
    check(&pair.sync_a, &pair.a);
    check(&pair.sync_b, &pair.b);
}

/// A blob that went missing from the journal seals again after a restart
/// from the key in the record's payload (§14, §15).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_outbox_record_keeps_its_blob_key_in_its_payload() {
    let mut pair = pair().await;
    pair.a.write("Scan.pdf", &vec![7; 400_000]);
    pair.sync_a.capture().expect("signed into the outbox");
    let uploads = pair
        .a
        .path(".noura/sync")
        .join(pair.laptop.id().to_hex())
        .join("uploads");
    std::fs::remove_dir_all(&uploads).expect("the ciphertext goes");
    pair.sync_a.lose_saves();
    pair.sync_a = app_core::sync::workspace::WorkspaceSync::open(
        pair.a.engine.clone(),
        pair.laptop.credentials.clone(),
        pair.laptop.secrets(),
        &pair.a.registry,
    )
    .expect("the folder opens");
    rounds(&mut pair).await;
    assert_eq!(pair.a.files(), pair.b.files());
}
