//! Seam 3: a plugin reaches only the HTTPS hosts its manifest declares.
//! The host sends each request through `local-core`'s web access
//! boundary, here over a test server that stands in for the declared
//! hosts, and refuses an undeclared host, a plain-HTTP URL, a redirect to
//! an undeclared host and a private address before any connection opens.

mod support;

use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
};

use local_core::{
    HostRequests, TransportRequest, TransportResponse, WebAccessFuture, WebAccessLimits,
    WebTransport,
};
use plugin_host::{Capability, Grant, Limits, Services};
use support::plugins;

/// Stands in for the hosts a plugin declares: it answers from a script
/// and records each address lookup and each connection.
#[derive(Default)]
struct TestServer {
    lookups: Mutex<Vec<String>>,
    connections: Mutex<Vec<String>>,
}

impl TestServer {
    fn lookups(&self) -> Vec<String> {
        self.lookups.lock().unwrap().clone()
    }

    fn connections(&self) -> Vec<String> {
        self.connections.lock().unwrap().clone()
    }
}

impl WebTransport for TestServer {
    fn resolve<'a>(&'a self, host: &'a str, _port: u16) -> WebAccessFuture<'a, Vec<IpAddr>> {
        self.lookups.lock().unwrap().push(host.to_owned());
        let address = match host {
            "intranet.example.com" => "10.0.0.5",
            _ => "93.184.215.14",
        };
        Box::pin(async move { Ok(vec![address.parse().unwrap()]) })
    }

    fn send(&self, request: TransportRequest) -> WebAccessFuture<'_, TransportResponse> {
        let url = request.url.to_string();
        self.connections.lock().unwrap().push(url.clone());
        let answer = match url.as_str() {
            "https://api.example.com/forecast" => TransportResponse {
                status: 200,
                headers: Vec::new(),
                body: b"sunny".to_vec(),
            },
            "https://api.example.com/moved" => TransportResponse {
                status: 302,
                headers: vec![("location".into(), "https://evil.example.net/".into())],
                body: Vec::new(),
            },
            _ => TransportResponse {
                status: 404,
                headers: Vec::new(),
                body: Vec::new(),
            },
        };
        Box::pin(async move { Ok(answer) })
    }
}

const NETWORK: &str = "name = \"network\"\nhosts = [\"api.example.com\", \"intranet.example.com\"]";

/// The probe with `views` and `capabilities`, sending its requests to a
/// fresh test server.
fn probe_with(
    probe: &[u8],
    capabilities: &[&str],
) -> (
    Arc<plugin_host::RunningPlugin>,
    Arc<TestServer>,
    [tempfile::TempDir; 2],
) {
    let server = Arc::new(TestServer::default());
    let requests = HostRequests::new(server.clone(), WebAccessLimits::default()).unwrap();
    let mut declared = vec!["name = \"views\""];
    declared.extend_from_slice(capabilities);
    let (plugin, folders) = plugins::start(
        "probe",
        probe,
        &declared,
        Services {
            network: Some(Arc::new(requests)),
            ..Services::default()
        },
        Limits::default(),
    );
    (plugin, server, folders)
}

fn fetch(plugin: &plugin_host::RunningPlugin, url: &str) -> String {
    let nodes = plugin
        .render(&format!("fetch:{url}"))
        .expect("the plugin renders");
    plugins::texts(&nodes).join("\n")
}

#[test]
fn a_declared_https_host_answers_through_the_web_access_boundary() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, server, _folders) = probe_with(&probe, &[NETWORK]);

    let answer = fetch(&plugin, "https://api.example.com/forecast");

    assert_eq!(
        answer,
        "status 200 url https://api.example.com/forecast body sunny"
    );
    assert_eq!(server.connections(), ["https://api.example.com/forecast"]);
}

#[test]
fn undeclared_hosts_plain_http_and_private_addresses_fail_before_any_connection() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, server, _folders) = probe_with(&probe, &[NETWORK]);

    for url in [
        "https://evil.example.net/",
        "http://api.example.com/forecast",
        "https://intranet.example.com/",
        "https://10.0.0.5/",
        "https://localhost/",
    ] {
        let answer = fetch(&plugin, url);
        assert!(
            answer.starts_with("error network-refused "),
            "{url}: {answer}"
        );
    }
    assert!(
        server.connections().is_empty(),
        "{:?}",
        server.connections()
    );
    // Only the declared host that resolves to a private address was looked
    // up, and its address stopped the request.
    assert_eq!(server.lookups(), ["intranet.example.com"]);
}

#[test]
fn a_redirect_to_an_undeclared_host_fails_before_connecting_to_it() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, server, _folders) = probe_with(&probe, &[NETWORK]);

    let answer = fetch(&plugin, "https://api.example.com/moved");

    assert!(answer.starts_with("error network-refused "), "{answer}");
    assert_eq!(server.connections(), ["https://api.example.com/moved"]);
    assert_eq!(server.lookups(), ["api.example.com"]);
}

#[test]
fn a_plugin_without_the_network_capability_reaches_no_host() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, server, _folders) = probe_with(&probe, &[]);

    let answer = fetch(&plugin, "https://api.example.com/forecast");

    assert_eq!(answer, "error capability-not-declared network");
    assert!(server.lookups().is_empty());
    assert!(server.connections().is_empty());
}

#[test]
fn a_revoked_host_is_refused_from_the_next_call_on() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, server, _folders) = probe_with(&probe, &[NETWORK]);
    assert!(fetch(&plugin, "https://api.example.com/forecast").starts_with("status 200"));
    let mut approved = plugin.manifest().capabilities.clone();

    Capability::revoke(&mut approved, &Grant::Host("api.example.com".into()));
    plugin.set_approved(&approved);

    let answer = fetch(&plugin, "https://api.example.com/forecast");
    assert!(answer.starts_with("error network-refused "), "{answer}");
    assert_eq!(server.connections().len(), 1);

    // Revoking the last host takes the capability away.
    Capability::revoke(&mut approved, &Grant::Host("intranet.example.com".into()));
    plugin.set_approved(&approved);
    assert_eq!(
        fetch(&plugin, "https://api.example.com/forecast"),
        "error capability-not-declared network"
    );
}
