//! Seam 3: each plugin's store has a memory cap and each call a deadline.
//! A plugin that loops forever, allocates past its cap or panics returns an
//! error to its caller while other plugins keep answering. Its next call
//! runs in a new store, and a plugin that keeps failing stops for the
//! session until you start it again. Reading a plugin's health never waits
//! for its call, and a call that waits too long in all fails naming the
//! limit it passed.

mod support;

use std::{
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use local_core::{
    HostRequests, TransportRequest, TransportResponse, WebAccessFuture, WebAccessLimits,
    WebTransport,
};
use plugin_host::{FAILURE_WINDOW, Limits, MAX_FAILURES, PluginError, RunningPlugin, Services};
use support::plugins;

const DEADLINE: Duration = Duration::from_millis(500);

fn limits() -> Limits {
    Limits {
        memory_bytes: 32 << 20,
        call_deadline: DEADLINE,
        ..Limits::default()
    }
}

fn misbehave() -> Option<(Arc<RunningPlugin>, [tempfile::TempDir; 2])> {
    let component = plugins::built("misbehave_plugin")?;
    Some(plugins::start(
        "misbehave",
        &component,
        &["name = \"views\"", "name = \"commands\""],
        Services::default(),
        limits(),
    ))
}

#[test]
fn a_plugin_that_loops_forever_stops_at_its_deadline_while_another_plugin_keeps_answering() {
    let (Some((looping, _folders)), Some(probe)) = (misbehave(), plugins::built("probe_plugin"))
    else {
        return;
    };
    let (other, _other_folders) = plugins::running(&probe, &["name = \"views\""], None);
    let started = Instant::now();

    let call = std::thread::spawn(move || looping.render("loop"));
    let mut answers = 0;
    while !call.is_finished() {
        let nodes = other.render("hello").expect("the other plugin answers");
        assert_eq!(plugins::texts(&nodes), ["Probe is running."]);
        answers += 1;
        std::thread::sleep(Duration::from_millis(20));
    }
    let result = call.join().unwrap();

    assert_eq!(result, Err(PluginError::Deadline { deadline: DEADLINE }));
    assert!(started.elapsed() >= DEADLINE, "{:?}", started.elapsed());
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert!(answers > 1, "the other plugin answered {answers} times");
}

/// A component whose start function loops forever, so any of its code
/// that runs shows.
const LOOPS_AT_START: &str = r#"
(component
  (core module $m
    (func $spin (loop $again (br $again)))
    (start $spin))
  (core instance $i (instantiate $m)))
"#;

#[test]
fn installing_and_starting_a_plugin_runs_none_of_its_code_until_its_first_call() {
    let component = wat::parse_str(LOOPS_AT_START).unwrap();
    let started = Instant::now();

    let (plugin, _folders) = plugins::start(
        "loops",
        &component,
        &["name = \"views\""],
        Services::default(),
        limits(),
    );

    assert!(
        started.elapsed() < DEADLINE,
        "installing took {:?}",
        started.elapsed()
    );
    assert_eq!(plugin.health().recent_failures, 0);
    assert_eq!(
        plugin.pages(),
        Err(PluginError::Deadline { deadline: DEADLINE })
    );
}

#[test]
fn a_plugin_that_allocates_past_its_cap_fails_with_the_cap() {
    let Some((plugin, _folders)) = misbehave() else {
        return;
    };

    let result = plugin.render("allocate");

    assert_eq!(
        result,
        Err(PluginError::MemoryCap {
            cap_bytes: 32 << 20
        })
    );
}

#[test]
fn a_plugin_that_panics_fails_and_its_next_call_runs_in_a_new_store() {
    let Some((plugin, _folders)) = misbehave() else {
        return;
    };

    let result = plugin.render("panic");

    assert!(matches!(result, Err(PluginError::Call(_))), "{result:?}");
    let nodes = plugin.render("misbehave").expect("a new store answers");
    assert_eq!(plugins::texts(&nodes), ["Still fine."]);
    assert_eq!(plugin.health().recent_failures, 1);
    assert!(!plugin.health().stopped);
}

#[test]
fn a_command_that_loops_returns_an_error_to_the_caller() {
    let Some((plugin, _folders)) = misbehave() else {
        return;
    };

    let result = plugin.run_command("loop", "");

    assert_eq!(result, Err(PluginError::Deadline { deadline: DEADLINE }));
}

#[test]
fn reading_a_plugins_health_doesnt_wait_for_its_call() {
    let Some((plugin, _folders)) = misbehave() else {
        return;
    };
    let looping = plugin.clone();
    let call = std::thread::spawn(move || looping.render("loop"));
    std::thread::sleep(DEADLINE / 5);

    let asked = Instant::now();
    let health = plugin.health();
    let took = asked.elapsed();

    assert!(
        !call.is_finished(),
        "the call ended before the health check"
    );
    assert!(took < DEADLINE / 5, "the health check took {took:?}");
    assert_eq!(health.recent_failures, 0);
    assert_eq!(
        call.join().unwrap(),
        Err(PluginError::Deadline { deadline: DEADLINE })
    );
    assert_eq!(plugin.health().recent_failures, 1);
}

#[test]
fn a_plugin_that_keeps_failing_stops_until_it_is_started_again() {
    let Some((plugin, _folders)) = misbehave() else {
        return;
    };

    for _ in 0..MAX_FAILURES {
        assert!(matches!(plugin.render("panic"), Err(PluginError::Call(_))));
    }

    assert!(plugin.health().stopped);
    assert_eq!(plugin.render("misbehave"), Err(PluginError::Stopped));
    assert_eq!(plugin.pages(), Err(PluginError::Stopped));

    plugin.restart();

    assert_eq!(
        plugins::texts(&plugin.render("misbehave").unwrap()),
        ["Still fine."]
    );
    assert_eq!(plugin.health().recent_failures, 0);
    assert!(FAILURE_WINDOW >= Duration::from_secs(10));
}

/// Answers every request after a pause longer than the call deadline.
struct SlowServer;

impl WebTransport for SlowServer {
    fn resolve<'a>(&'a self, _host: &'a str, _port: u16) -> WebAccessFuture<'a, Vec<IpAddr>> {
        Box::pin(async { Ok(vec!["93.184.215.14".parse().unwrap()]) })
    }

    fn send(&self, _request: TransportRequest) -> WebAccessFuture<'_, TransportResponse> {
        Box::pin(async {
            std::thread::sleep(DEADLINE * 3);
            Ok(TransportResponse {
                status: 200,
                headers: Vec::new(),
                body: b"late".to_vec(),
            })
        })
    }
}

/// The probe under `limits`, sending its requests to a [`SlowServer`].
fn slow_probe(probe: &[u8], limits: Limits) -> (Arc<RunningPlugin>, [tempfile::TempDir; 2]) {
    let requests = HostRequests::new(Arc::new(SlowServer), WebAccessLimits::default()).unwrap();
    plugins::start(
        "probe",
        probe,
        &[
            "name = \"views\"",
            "name = \"network\"\nhosts = [\"api.example.com\"]",
        ],
        Services {
            network: Some(Arc::new(requests)),
            ..Services::default()
        },
        limits,
    )
}

#[test]
fn time_spent_waiting_for_the_network_doesnt_count_toward_the_deadline() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, _folders) = slow_probe(&probe, limits());

    let nodes = plugin
        .render("fetch:https://api.example.com/slow")
        .expect("the call outlives its deadline while it waits");

    assert_eq!(
        plugins::texts(&nodes),
        ["status 200 url https://api.example.com/slow body late"]
    );
}

#[test]
fn a_call_that_waits_past_its_whole_call_limit_fails_naming_that_limit() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    // The server answers after three deadlines, past the call's limit of
    // two, though the plugin's own work stays far under one.
    let max_call_time = DEADLINE * 2;
    let (plugin, _folders) = slow_probe(
        &probe,
        Limits {
            max_call_time,
            ..limits()
        },
    );

    let result = plugin.render("fetch:https://api.example.com/slow");

    assert_eq!(
        result,
        Err(PluginError::Deadline {
            deadline: max_call_time
        })
    );
}
