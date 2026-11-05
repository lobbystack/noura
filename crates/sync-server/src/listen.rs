//! The accept loop. Unlike `axum::serve` it bounds what an open connection
//! can hold: how many connections stay open at once, how long request
//! headers and a request body take to arrive, and how long a shutdown waits
//! for the requests in flight. Each connection holds its place until its
//! socket closes, also after an upgrade to a realtime channel.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::ConnectInfo;
use http_body::{Frame, SizeHint};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::JoinSet;
use tokio::time::{Sleep, sleep, timeout};
use tower_service::Service;

/// How long the loop waits after a failed accept, such as when the process
/// runs out of file descriptors.
const ACCEPT_RETRY: Duration = Duration::from_secs(1);

/// What one server allows its connections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionLimits {
    /// How long a request's headers take to arrive. It also ends a
    /// connection that sends nothing.
    pub header_read: Duration,
    /// How long a request's body takes to arrive, from its headers.
    pub body_read: Duration,
    /// How many connections stay open at once, realtime channels included.
    /// Further ones wait in the listener's backlog.
    pub connections: usize,
    /// How long a shutdown waits for requests in flight, which covers a
    /// pull's 25-second wait.
    pub grace: Duration,
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            header_read: Duration::from_secs(20),
            body_read: Duration::from_secs(60),
            connections: 1_024,
            grace: Duration::from_secs(30),
        }
    }
}

/// A request body that didn't arrive within [`ConnectionLimits::body_read`].
#[derive(Debug)]
struct BodyLate;

impl fmt::Display for BodyLate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the request body took too long to arrive")
    }
}

impl Error for BodyLate {}

/// Whether reading a body failed because its time ran out.
pub(crate) fn is_late(error: &(dyn Error + 'static)) -> bool {
    let mut next = Some(error);
    while let Some(error) = next {
        if error.is::<BodyLate>() {
            return true;
        }
        next = error.source();
    }
    false
}

/// A request body with a time limit over all of its frames, so a slow
/// sender can't keep one going a byte at a time.
struct Deadline {
    body: Incoming,
    late: Pin<Box<Sleep>>,
}

impl http_body::Body for Deadline {
    type Data = Bytes;
    type Error = Box<dyn Error + Send + Sync>;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let this = self.get_mut();
        if let Poll::Ready(frame) = Pin::new(&mut this.body).poll_frame(cx) {
            return Poll::Ready(frame.map(|frame| frame.map_err(Into::into)));
        }
        match this.late.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Some(Err(Box::new(BodyLate)))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

/// A socket that holds a connection's place until it closes.
struct Held {
    stream: TcpStream,
    _place: OwnedSemaphorePermit,
}

impl AsyncRead for Held {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Held {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

/// Serves one connection until it ends, or finishes its request and ends
/// once `stopping` turns true.
async fn connection(
    router: Router,
    socket: Held,
    peer: SocketAddr,
    limits: ConnectionLimits,
    mut stopping: watch::Receiver<bool>,
) {
    let service = service_fn(move |request: hyper::Request<Incoming>| {
        let mut request = request.map(|body| {
            Body::new(Deadline {
                body,
                late: Box::pin(sleep(limits.body_read)),
            })
        });
        request.extensions_mut().insert(ConnectInfo(peer));
        let mut router = router.clone();
        async move { router.call(request).await }
    });
    let serving = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(limits.header_read)
        .serve_connection(TokioIo::new(socket), service)
        .with_upgrades();
    tokio::pin!(serving);
    tokio::select! {
        _ = serving.as_mut() => return,
        _ = stopping.wait_for(|stop| *stop) => serving.as_mut().graceful_shutdown(),
    }
    let _ = serving.await;
}

/// Serves `router` on `listener` until `shutdown` resolves, then waits up to
/// [`ConnectionLimits::grace`] for the requests in flight and drops the rest.
pub async fn serve(
    router: Router,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
    limits: ConnectionLimits,
) {
    let places = Arc::new(Semaphore::new(limits.connections));
    let (stop, stopping) = watch::channel(false);
    let mut connections = JoinSet::new();
    tokio::pin!(shutdown);
    loop {
        let place = tokio::select! {
            biased;
            () = &mut shutdown => break,
            place = places.clone().acquire_owned() => place.expect("the semaphore stays open"),
        };
        let (stream, peer) = tokio::select! {
            biased;
            () = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!("can't accept a connection: {error}");
                    drop(place);
                    tokio::select! {
                        biased;
                        () = &mut shutdown => break,
                        () = sleep(ACCEPT_RETRY) => continue,
                    }
                }
            },
        };
        let _ = stream.set_nodelay(true);
        let socket = Held {
            stream,
            _place: place,
        };
        connections.spawn(connection(
            router.clone(),
            socket,
            peer,
            limits,
            stopping.clone(),
        ));
        while connections.try_join_next().is_some() {}
    }
    drop(listener);
    let _ = stop.send(true);
    let finished = async { while connections.join_next().await.is_some() {} };
    if timeout(limits.grace, finished).await.is_err() {
        tracing::warn!("a shutdown dropped requests still in flight");
    }
}
