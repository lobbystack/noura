//! A remote image load ignores proxy settings. A proxy would resolve the
//! image's host itself, so a public name that leads to a private address
//! would never meet the fetcher's address checks.
//!
//! This test sets proxy variables for its whole process, so it lives in a
//! test binary of its own.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use local_core::RemoteImageFetcher;

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
    // SAFETY: this binary runs one test, and nothing else reads the
    // environment while it changes.
    unsafe {
        for name in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            std::env::set_var(name, &proxy_url);
        }
        for name in ["NO_PROXY", "no_proxy"] {
            std::env::remove_var(name);
        }
    }

    // `.invalid` never resolves, so without the proxy the load fails
    // before any request.
    let result = RemoteImageFetcher::new().fetch("http://images.example.invalid/pic.png");

    assert!(result.is_err());
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "the load went through the proxy"
    );
}
