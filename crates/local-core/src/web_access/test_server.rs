//! A stand-in for the web, for the web access tests: hosts resolve as a
//! table says, each URL answers from a script, and the server records
//! every lookup and every hop it was sent. Nothing leaves the computer.

use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use super::{TransportRequest, TransportResponse, WebAccessFuture, WebTransport};

/// A public address every public test host resolves to.
pub(crate) const PUBLIC: &str = "93.184.215.14";

pub(crate) struct FakeServer {
    /// Each host and the addresses it resolves to.
    dns: Vec<(&'static str, Vec<IpAddr>)>,
    /// Each URL and what it answers.
    answers: Vec<(String, TransportResponse)>,
    /// How long each hop takes to answer.
    delay: Duration,
    lookups: Mutex<Vec<String>>,
    hops: Mutex<Vec<TransportRequest>>,
}

impl FakeServer {
    pub(crate) fn new(answers: Vec<(&str, TransportResponse)>) -> Arc<Self> {
        Self::slow(answers, Duration::ZERO)
    }

    /// A server whose hops each take `delay` to answer.
    pub(crate) fn slow(answers: Vec<(&str, TransportResponse)>, delay: Duration) -> Arc<Self> {
        let public: IpAddr = PUBLIC.parse().unwrap();
        Arc::new(Self {
            dns: vec![
                ("garden.example.com", vec![public]),
                ("notes.example.com", vec![public]),
                ("api.search.brave.com", vec![public]),
                ("intranet.example.com", vec!["10.0.0.5".parse().unwrap()]),
                ("loopback.example.com", vec!["127.0.0.1".parse().unwrap()]),
                (
                    "link-local.example.com",
                    vec!["169.254.10.20".parse().unwrap(), "fe80::1".parse().unwrap()],
                ),
                ("mixed.example.com", vec![public, "::1".parse().unwrap()]),
            ],
            answers: answers
                .into_iter()
                .map(|(url, answer)| (url.to_owned(), answer))
                .collect(),
            delay,
            lookups: Mutex::new(Vec::new()),
            hops: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn lookups(&self) -> Vec<String> {
        self.lookups.lock().unwrap().clone()
    }

    pub(crate) fn hops(&self) -> Vec<TransportRequest> {
        self.hops.lock().unwrap().clone()
    }
}

impl WebTransport for FakeServer {
    fn resolve<'a>(&'a self, host: &'a str, _port: u16) -> WebAccessFuture<'a, Vec<IpAddr>> {
        self.lookups.lock().unwrap().push(host.to_owned());
        let addresses = self
            .dns
            .iter()
            .find(|(known, _)| *known == host)
            .map(|(_, addresses)| addresses.clone())
            .unwrap_or_default();
        Box::pin(async move { Ok(addresses) })
    }

    fn send(&self, request: TransportRequest) -> WebAccessFuture<'_, TransportResponse> {
        let answer = self
            .answers
            .iter()
            .find(|(url, _)| url == request.url.as_str())
            .map(|(_, answer)| answer.clone())
            .unwrap_or(TransportResponse {
                status: 404,
                headers: Vec::new(),
                body: Vec::new(),
            });
        self.hops.lock().unwrap().push(request);
        let delay = self.delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            Ok(answer)
        })
    }
}

/// A page of `content_type` that holds `body`.
pub(crate) fn page(content_type: &str, body: &str) -> TransportResponse {
    TransportResponse {
        status: 200,
        headers: vec![("content-type".into(), content_type.into())],
        body: body.as_bytes().to_vec(),
    }
}

/// A redirect to `location`.
pub(crate) fn redirect(status: u16, location: &str) -> TransportResponse {
    TransportResponse {
        status,
        headers: vec![("location".into(), location.into())],
        body: Vec::new(),
    }
}
