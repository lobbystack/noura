//! A remote image load ignores proxy settings. A proxy would resolve the
//! image's host itself, so a public name that leads to a private address
//! would never meet the fetcher's address checks.
//!
//! The proxy variables must be set for a whole process, so the test runs
//! the load in a copy of this test binary with the variables set, and the
//! tests sharing this process never see them.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use local_core::RemoteImageFetcher;

/// Set in the copy that loads the image.
const CHILD: &str = "NOURA_REMOTE_IMAGE_PROXY_CHILD";

#[test]
fn a_remote_image_load_never_goes_through_a_proxy() {
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = proxy.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in proxy.incoming() {
            counted.fetch_add(1, Ordering::SeqCst);
            let Ok(mut stream) = stream else { continue };
            let _ = stream.read(&mut [0; 4096]);
            let _ = stream.write_all(
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let proxy_url = format!("http://{address}");
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--ignored",
            "--exact",
            "--nocapture",
            "remote_image_proxy::loads_an_image_with_proxy_variables_set",
        ])
        .env(CHILD, "1")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy");
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        child.env(name, &proxy_url);
    }
    let output = child.output().unwrap();
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{printed}");
    assert!(printed.contains("1 passed"), "the load ran: {printed}");

    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "the load went through the proxy"
    );
}

#[test]
#[ignore = "a_remote_image_load_never_goes_through_a_proxy runs it as a process of its own"]
fn loads_an_image_with_proxy_variables_set() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    // `.invalid` never resolves, so without the proxy the load fails
    // before any request.
    let result = RemoteImageFetcher::new().fetch("http://images.example.invalid/pic.png");
    assert!(result.is_err());
}
