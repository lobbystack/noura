//! Model downloads through `hf-hub` against a local server that speaks
//! the Hub's resolve URLs: a download resumes after the connection drops,
//! stops when the server stalls, and refuses a file whose SHA-256 doesn't
//! match.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};
use speech::catalog::ModelFile;
use speech::download::{DownloadOptions, Progress, download, part_path};

/// How the server misbehaves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Fine,
    /// The first GET sends half the file, then hangs up.
    DropOnce,
    /// Every GET sends a few bytes, then stalls.
    Stall,
}

struct Server {
    endpoint: String,
    /// The Range header of each GET.
    ranges: Arc<Mutex<Vec<Option<String>>>>,
}

fn serve(body: Vec<u8>, mode: Mode) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let seen = ranges.clone();
    let gets = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let body = body.clone();
            let seen = seen.clone();
            let gets = gets.clone();
            std::thread::spawn(move || answer(stream, &body, mode, &seen, &gets));
        }
    });
    Server { endpoint, ranges }
}

fn answer(
    mut stream: TcpStream,
    body: &[u8],
    mode: Mode,
    seen: &Mutex<Vec<Option<String>>>,
    gets: &AtomicUsize,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    loop {
        let mut request = String::new();
        if reader.read_line(&mut request).unwrap_or(0) == 0 {
            return;
        }
        let mut range = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                range = Some(value.trim().to_owned());
            }
        }
        if !request.contains("/resolve/") {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n");
            continue;
        }
        let start = range
            .as_deref()
            .and_then(|range| range.split('-').next())
            .and_then(|start| start.parse::<usize>().ok())
            .unwrap_or(0);
        let part = &body[start.min(body.len())..];
        if request.starts_with("HEAD") {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\netag: \"x\"\r\nx-repo-commit: abc\r\n\r\n",
                body.len()
            );
            continue;
        }
        seen.lock().unwrap().push(range.clone());
        let status = if range.is_some() {
            "206 Partial Content"
        } else {
            "200 OK"
        };
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\ncontent-length: {}\r\n\r\n",
            part.len()
        );
        let count = gets.fetch_add(1, Ordering::SeqCst);
        match mode {
            Mode::Fine => {
                let _ = stream.write_all(part);
            }
            Mode::DropOnce if count == 0 => {
                let _ = stream.write_all(&part[..part.len() / 2]);
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return;
            }
            Mode::DropOnce => {
                let _ = stream.write_all(part);
            }
            Mode::Stall => {
                let _ = stream.write_all(&part[..16.min(part.len())]);
                let _ = stream.flush();
                std::thread::sleep(Duration::from_secs(3));
                return;
            }
        }
        let _ = stream.flush();
        let mut rest = Vec::new();
        let _ = reader
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(50)));
        let _ = reader.read(&mut rest);
    }
}

fn file_for(body: &[u8], sha256: Option<&str>) -> ModelFile {
    let digest: String = Sha256::digest(body)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    ModelFile {
        repository: "handy-computer/tiny-gguf".into(),
        revision: "0123456789abcdef".into(),
        filename: "tiny.gguf".into(),
        size: body.len() as u64,
        sha256: sha256.map_or(digest, str::to_owned),
    }
}

fn body() -> Vec<u8> {
    (0..200_000u32).map(|n| (n % 251) as u8).collect()
}

fn options(server: &Server) -> DownloadOptions {
    DownloadOptions {
        endpoint: server.endpoint.clone(),
        stall: Duration::from_millis(500),
        retries: 2,
    }
}

#[test]
fn a_download_that_drops_resumes_where_it_stopped_and_checks_its_hash() {
    let body = body();
    let server = serve(body.clone(), Mode::DropOnce);
    let folder = tempfile::tempdir().unwrap();
    let destination = folder.path().join("models/tiny.gguf");
    let progress = Arc::new(Progress::default());

    download(
        &file_for(&body, None),
        &destination,
        &options(&server),
        &progress,
    )
    .unwrap();

    assert_eq!(std::fs::read(&destination).unwrap(), body);
    assert!(!part_path(&destination).exists());
    assert_eq!(progress.received.load(Ordering::SeqCst), body.len() as u64);
    let ranges = server.ranges.lock().unwrap().clone();
    assert_eq!(ranges.len(), 2, "{ranges:?}");
    assert_eq!(
        ranges[0].as_deref(),
        Some(format!("0-{}", body.len() - 1).as_str())
    );
    let resumed: usize = ranges[1]
        .as_deref()
        .unwrap()
        .split('-')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(resumed > 0 && resumed < body.len(), "{resumed}");
}

#[test]
fn a_stalled_download_stops_and_keeps_its_part_for_later() {
    let body = body();
    let server = serve(body.clone(), Mode::Stall);
    let folder = tempfile::tempdir().unwrap();
    let destination = folder.path().join("tiny.gguf");
    let progress = Arc::new(Progress::default());

    let error = download(
        &file_for(&body, None),
        &destination,
        &options(&server),
        &progress,
    )
    .unwrap_err();

    assert_eq!(error.code, "download_stalled");
    assert!(!destination.exists());
    assert!(part_path(&destination).exists());
}

#[test]
fn a_file_whose_hash_doesnt_match_never_becomes_the_model() {
    let body = body();
    let server = serve(body.clone(), Mode::Fine);
    let folder = tempfile::tempdir().unwrap();
    let destination = folder.path().join("tiny.gguf");
    let wrong = "0".repeat(64);

    let error = download(
        &file_for(&body, Some(&wrong)),
        &destination,
        &options(&server),
        &Arc::new(Progress::default()),
    )
    .unwrap_err();

    assert_eq!(error.code, "checksum_mismatch");
    assert!(!destination.exists());
    assert!(!part_path(&destination).exists());
}

#[test]
fn a_cancelled_download_stops_at_once() {
    let body = body();
    let server = serve(body.clone(), Mode::Fine);
    let folder = tempfile::tempdir().unwrap();
    let destination = folder.path().join("tiny.gguf");
    let progress = Arc::new(Progress::default());
    progress.cancel();

    let error = download(
        &file_for(&body, None),
        &destination,
        &options(&server),
        &progress,
    )
    .unwrap_err();

    assert_eq!(error.code, "cancelled");
    assert!(!destination.exists());
}
