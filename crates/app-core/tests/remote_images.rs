//! Seam 1: a remote image in a note waits for Load image. Opening the note
//! sends no request; Load image fetches it once, within today's limits,
//! and a failed load can be tried again. Local, private and loopback
//! addresses are refused, which a local test server that counts requests
//! shows; the tests that load from it let the fetcher reach loopback.

mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use app_core::{App, NoteLinks, RemoteImage, RemoteImageState};
use support::{Fixture, open_from_tree, settle};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage bytes";

/// A local HTTP server that counts the requests it answers.
struct Server {
    port: u16,
    requests: Arc<AtomicUsize>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let counted = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    continue;
                }
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                        break;
                    }
                }
                let served = counted.fetch_add(1, Ordering::SeqCst);
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let response = respond(&path, served, port);
                let _ = stream.write_all(&response);
                let mut rest = Vec::new();
                let _ = stream.set_read_timeout(Some(Duration::from_millis(10)));
                let _ = reader.read_to_end(&mut rest);
            }
        });
        Self { port, requests }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

/// The server's answer to its `served`th request, for `path`.
fn respond(path: &str, served: usize, port: u16) -> Vec<u8> {
    let reply = |status: &str, headers: &str, body: &[u8]| {
        let mut response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    };
    if let Some(left) = path.strip_prefix("/redirect/") {
        let left: usize = left.parse().unwrap();
        let next = if left == 0 {
            "/pic.png".to_string()
        } else {
            format!("/redirect/{}", left - 1)
        };
        return reply(
            "302 Found",
            &format!("Location: http://127.0.0.1:{port}{next}\r\n"),
            b"",
        );
    }
    match path {
        "/pic.png" => reply("200 OK", "Content-Type: image/png\r\n", PNG),
        "/page" => reply("200 OK", "Content-Type: text/html\r\n", b"<p>hi</p>"),
        "/big.png" => format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            20 * 1024 * 1024 + 1
        )
        .into_bytes(),
        "/flaky.png" if served == 0 => reply("500 Internal Server Error", "", b""),
        "/flaky.png" => reply("200 OK", "Content-Type: image/png\r\n", PNG),
        _ => reply("404 Not Found", "", b""),
    }
}

fn note_links(fixture: &Fixture, app: &mut App) -> NoteLinks {
    let _ = fixture;
    let tab = open_from_tree(app, "Plan.md");
    app.note_links(tab).expect("the note is open")
}

/// Wait until loading `url` ends, and return how it ended.
fn finished(links: &NoteLinks, url: &str) -> RemoteImageState {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let state = links.remote_image(url);
        if state != RemoteImageState::Loading {
            return state;
        }
        assert!(Instant::now() < deadline, "the load never ended");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The test server's image, loaded.
fn png() -> RemoteImageState {
    RemoteImageState::Loaded(Arc::new(RemoteImage {
        bytes: PNG.to_vec(),
        content_type: "image/png".into(),
    }))
}

fn fixture_with(note: &str) -> Fixture {
    Fixture::new(&[("Plan.md", note.as_bytes())])
}

#[test]
fn opening_a_note_sends_no_request_and_load_image_fetches_once() {
    let server = Server::start();
    let url = server.url("/pic.png");
    let fixture = fixture_with(&format!("![remote]({url})\n"));
    let mut app = fixture.app();
    app.allow_loopback_images_for_testing();
    let links = note_links(&fixture, &mut app);
    settle(&mut app);

    assert_eq!(links.remote_image(&url), RemoteImageState::NotLoaded);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(server.requests(), 0, "opening the note asked for nothing");

    links.load_remote_image(&url, || {});
    links.load_remote_image(&url, || {});
    assert_eq!(finished(&links, &url), png());
    links.load_remote_image(&url, || {});
    assert_eq!(finished(&links, &url), png());
    assert_eq!(server.requests(), 1, "one load, one request");
}

#[test]
fn a_failed_load_can_be_tried_again() {
    let server = Server::start();
    let url = server.url("/flaky.png");
    let fixture = fixture_with("text\n");
    let mut app = fixture.app();
    app.allow_loopback_images_for_testing();
    let links = note_links(&fixture, &mut app);

    links.load_remote_image(&url, || {});
    assert!(matches!(
        finished(&links, &url),
        RemoteImageState::Failed(_)
    ));

    links.load_remote_image(&url, || {});
    assert_eq!(finished(&links, &url), png());
    assert_eq!(server.requests(), 2);
}

#[test]
fn loads_keep_to_todays_limits() {
    let server = Server::start();
    let fixture = fixture_with("text\n");
    let mut app = fixture.app();
    app.allow_loopback_images_for_testing();
    let links = note_links(&fixture, &mut app);

    for (path, loads) in [
        // `/redirect/N` takes N + 1 redirects to reach the image.
        ("/redirect/3", true),
        ("/redirect/4", true),
        ("/redirect/5", false),
        ("/page", false),
        ("/big.png", false),
        ("/missing.png", false),
    ] {
        let url = server.url(path);
        links.load_remote_image(&url, || {});
        let state = finished(&links, &url);
        assert_eq!(
            matches!(state, RemoteImageState::Loaded(_)),
            loads,
            "{path}: {state:?}"
        );
    }

    let before = server.requests();
    for refused in [
        format!("ftp://127.0.0.1:{}/pic.png", server.port),
        format!("http://user:secret@127.0.0.1:{}/pic.png", server.port),
    ] {
        links.load_remote_image(&refused, || {});
        assert!(
            matches!(finished(&links, &refused), RemoteImageState::Failed(_)),
            "{refused}"
        );
    }
    assert_eq!(server.requests(), before, "refused addresses send nothing");
}

#[test]
fn local_and_loopback_addresses_are_refused() {
    let server = Server::start();
    let fixture = fixture_with("text\n");
    let mut app = fixture.app();
    let links = note_links(&fixture, &mut app);

    for url in [
        server.url("/pic.png"),
        format!("http://localhost:{}/pic.png", server.port),
        format!("http://[::1]:{}/pic.png", server.port),
        "http://192.168.0.1/pic.png".to_string(),
        "http://10.1.2.3/pic.png".to_string(),
    ] {
        links.load_remote_image(&url, || {});
        assert!(
            matches!(finished(&links, &url), RemoteImageState::Failed(_)),
            "{url}"
        );
    }
    assert_eq!(server.requests(), 0);
}
