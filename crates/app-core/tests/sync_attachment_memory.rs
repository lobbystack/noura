//! Peak memory while an attachment syncs (seam 4, sync-v2.md §21): the
//! upload and the download of a large blob keep the process's peak heap,
//! client and server together, under four times the 1 MiB segment size.
//! The test here moves a 48 MiB file; the scheduled run moves 2 GiB.
//!
//! A counting allocator wraps the system one, so this file holds one test
//! at a time and each runs alone in the binary's process.

mod sync_support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

use sync_protocol::blob::SEGMENT_LEN;
use sync_support::{Pair, pair};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call goes to the system allocator unchanged; the counters
// only observe sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            if new_size >= layout.size() {
                let now = CURRENT.fetch_add(new_size - layout.size(), Ordering::Relaxed)
                    + (new_size - layout.size());
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                CURRENT.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Starts a new measurement and returns the heap in use now.
fn baseline() -> usize {
    let now = CURRENT.load(Ordering::Relaxed);
    PEAK.store(now, Ordering::Relaxed);
    now
}

/// How far the heap rose above `base` since [`baseline`].
fn rise(base: usize) -> usize {
    PEAK.load(Ordering::Relaxed).saturating_sub(base)
}

/// Writes a `len`-byte file to `path` in 1 MiB pieces, so the test itself
/// never holds it.
fn write_large(path: &std::path::Path, len: u64) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut piece = vec![0u8; SEGMENT_LEN];
    let mut written = 0u64;
    let mut seed = 7u32;
    while written < len {
        for byte in piece.iter_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *byte = (seed >> 24) as u8;
        }
        let take = (len - written).min(piece.len() as u64) as usize;
        file.write_all(&piece[..take]).unwrap();
        written += take as u64;
    }
    file.flush().unwrap();
}

/// The BLAKE3 hash of the file at `path`, streamed.
fn hash_file(path: &std::path::Path) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_reader(std::fs::File::open(path).unwrap())
        .unwrap();
    hasher.finalize()
}

async fn sync_a_large_file(len: u64) {
    let Pair {
        server,
        proxy: _proxy,
        laptop,
        service: _proxied,
        a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = pair().await;
    // The laptop talks to the server directly here: the test proxy keeps
    // whole bodies, which would count against the client.
    let mut service = laptop.connect_through(&server.base).await;
    write_large(&a.path("Video/Talk.mov"), len);
    let limit = 4 * SEGMENT_LEN;

    let base = baseline();
    sync_a
        .round(&mut service, 0)
        .await
        .expect("the upload round");
    let upload = rise(base);

    let base = baseline();
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("the download round");
    let download = rise(base);

    assert_eq!(
        hash_file(&a.path("Video/Talk.mov")),
        hash_file(&b.path("Video/Talk.mov")),
        "the file arrives byte for byte"
    );
    eprintln!("peak heap rise for {len} bytes: upload {upload}, download {download}");
    assert!(upload < limit, "the upload rose {upload} bytes");
    assert!(download < limit, "the download rose {download} bytes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_48_mib_attachment_syncs_in_bounded_memory() {
    sync_a_large_file(48 * 1024 * 1024).await;
}

/// The scheduled run at the largest file a blob carries. It writes 2 GiB
/// to each of two folders and the server's blob store, so it runs only on
/// request: `cargo test -p app-core --test sync_attachment_memory --
/// --ignored`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "scheduled: moves 2 GiB"]
async fn a_2_gib_attachment_syncs_in_bounded_memory() {
    sync_a_large_file(sync_protocol::blob::MAX_BLOB_FILE).await;
}
