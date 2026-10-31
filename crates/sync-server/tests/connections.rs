//! What one connection can hold of the server (§26): partial headers and
//! bodies time out, connections stop at a limit, realtime channels among
//! them, and a shutdown stops waiting for requests in flight.

mod support;

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use support::{Client, seed_store};
use sync_server::listen::ConnectionLimits;
use sync_server::{AppState, RateGate};
use sync_store_sqlite::SqliteBackend;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn store() -> (Arc<sync_store::Store<SqliteBackend>>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    sync_server::migrate(&data).await.expect("migrated");
    let store = sync_server::open_store(&sync_server::Config::new(
        data,
        "127.0.0.1:0".parse().expect("an address"),
    ))
    .await
    .expect("the store opens");
    (Arc::new(store), dir)
}

/// Serves on a loopback port with `limits` until `shutdown` resolves.
async fn serve(
    limits: ConnectionLimits,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> (
    SocketAddr,
    tokio::task::JoinHandle<()>,
    Arc<sync_store::Store<SqliteBackend>>,
    tempfile::TempDir,
) {
    let (store, dir) = store().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bound");
    let address = listener.local_addr().expect("an address");
    let origin = sync_server::PublicOrigin::parse(&format!("http://localhost:{}", address.port()))
        .expect("an origin");
    let state = AppState {
        store: store.clone(),
        accounts: Arc::new(sync_server::Accounts::local(origin)),
        blobs: Arc::new(sync_server::blobs::Objects::memory()),
        rate: Arc::new(RateGate::unlimited()),
    };
    let served = tokio::spawn(sync_server::listen::serve(
        sync_server::http::router(state),
        listener,
        shutdown,
        limits,
    ));
    (address, served, store, dir)
}

/// What the server sends until it closes, within `wait`, or `None` when it
/// leaves the connection open that long.
async fn until_closed(stream: &mut TcpStream, wait: Duration) -> Option<Vec<u8>> {
    let mut sent = Vec::new();
    tokio::time::timeout(wait, async {
        let mut buffer = [0; 1024];
        loop {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => sent.extend_from_slice(&buffer[..read]),
            }
        }
    })
    .await
    .ok()?;
    Some(sent)
}

const LONG: Duration = Duration::from_secs(300);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_headers_and_silent_connections_time_out() {
    let limits = ConnectionLimits {
        header_read: Duration::from_millis(300),
        ..ConnectionLimits::default()
    };
    let (address, _served, _store, _dir) = serve(limits, std::future::pending()).await;

    let mut partial = TcpStream::connect(address).await.expect("connected");
    partial
        .write_all(b"GET /v2/limits HTTP/1.1\r\nHost: loc")
        .await
        .expect("written");
    assert!(
        until_closed(&mut partial, Duration::from_secs(5))
            .await
            .is_some(),
        "the server drops a request whose headers stall"
    );

    let mut silent = TcpStream::connect(address).await.expect("connected");
    assert!(
        until_closed(&mut silent, Duration::from_secs(5))
            .await
            .is_some(),
        "the server drops a connection that sends nothing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_body_that_stalls_gets_an_answer_and_a_slow_one_runs_out_of_time() {
    let limits = ConnectionLimits {
        body_read: Duration::from_millis(600),
        ..ConnectionLimits::default()
    };
    let (address, _served, _store, _dir) = serve(limits, std::future::pending()).await;

    let mut stream = TcpStream::connect(address).await.expect("connected");
    stream
        .write_all(
            b"POST /v2/device/token HTTP/1.1\r\nHost: localhost\r\nContent-Length: 64\r\n\r\nabc",
        )
        .await
        .expect("written");
    let started = Instant::now();
    // A byte now and then keeps every read short, and the limit holds over
    // the whole body.
    let mut answer = None;
    while started.elapsed() < Duration::from_secs(5) {
        if stream.write_all(b"d").await.is_err() {
            break;
        }
        let mut buffer = [0; 512];
        if let Ok(Ok(read)) =
            tokio::time::timeout(Duration::from_millis(100), stream.read(&mut buffer)).await
        {
            answer = Some(String::from_utf8_lossy(&buffer[..read]).into_owned());
            break;
        }
    }
    let answer = answer.expect("an answer before the client gives up");
    assert!(answer.starts_with("HTTP/1.1 500"), "{answer}");
    assert!(started.elapsed() >= Duration::from_millis(500));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connections_stop_at_the_limit_with_channels_counted() {
    let limits = ConnectionLimits {
        connections: 2,
        ..ConnectionLimits::default()
    };
    let (address, _served, store, _dir) = serve(limits, std::future::pending()).await;
    let account = seed_store(&store, 1).await;
    let created = account.creation();
    let base = format!("http://localhost:{}", address.port());
    let client = Client::new(base, Some(account.token(0)));
    let reply = client.post("/v2/workspaces", created.body.clone()).await;
    assert!(reply.status.is_success());
    // The test client's pooled connection holds the first place until it
    // drops.
    drop(client);

    let url = format!(
        "ws://localhost:{}/v2/workspaces/{}/realtime",
        address.port(),
        created.workspace.to_base64url()
    );
    let mut request =
        tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(url)
            .expect("a request");
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", account.token(0))
            .parse()
            .expect("a header"),
    );
    let mut sockets = Vec::new();
    for _ in 0..2 {
        let deadline = Instant::now() + Duration::from_secs(5);
        sockets.push(loop {
            let opened = tokio::time::timeout(
                Duration::from_secs(1),
                tokio_tungstenite::connect_async(request.clone()),
            )
            .await;
            match opened {
                Ok(Ok((socket, _))) => break socket,
                _ if Instant::now() < deadline => {}
                _ => panic!("a place frees up"),
            }
        });
    }

    let mut waiting = TcpStream::connect(address).await.expect("in the backlog");
    waiting
        .write_all(b"GET /v2/limits HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("written");
    assert_eq!(
        until_closed(&mut waiting, Duration::from_millis(400)).await,
        None,
        "no place for a third connection while two channels stay open"
    );
    drop(sockets);
    let answer = until_closed(&mut waiting, Duration::from_secs(5))
        .await
        .expect("a place frees up");
    assert!(answer.starts_with(b"HTTP/1.1 200"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shutdown_stops_waiting_for_a_held_request() {
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let limits = ConnectionLimits {
        header_read: LONG,
        body_read: LONG,
        grace: Duration::from_millis(300),
        ..ConnectionLimits::default()
    };
    let shutdown = async {
        let _ = stopped.await;
    };
    let (address, served, _store, _dir) = serve(limits, shutdown).await;
    let mut held = TcpStream::connect(address).await.expect("connected");
    held.write_all(
        b"POST /v2/device/token HTTP/1.1\r\nHost: localhost\r\nContent-Length: 64\r\n\r\nabc",
    )
    .await
    .expect("written");
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop.send(()).expect("still serving");
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .expect("the wait ends")
        .expect("no panic");
}
