//! Seam 3: a plugin reads the instant and this device's time zone from the
//! app's clock, since WASI's clocks aren't linked, asks the app to open
//! objects and its own pages in the window you acted in, and adds a
//! section to the sidebar beside one of its pages.

mod support;

use std::sync::{Arc, Mutex};

use plugin_host::{
    ClockReading, DeviceClock, Event, Navigation, Placement, PluginError, ServiceError, Services,
    Surface, View, ViewKind,
};
use support::plugins;

const VIEWS: [&str; 1] = ["name = \"views\""];

struct FixedClock;

impl DeviceClock for FixedClock {
    fn now(&self) -> ClockReading {
        ClockReading {
            instant: "2026-10-21T13:30:00Z".into(),
            time_zone: "America/Toronto".into(),
        }
    }
}

/// A navigation service that records what it was asked to open, or
/// refuses everything.
#[derive(Default)]
struct Opener {
    opened: Mutex<Vec<String>>,
    refuse: bool,
}

impl Opener {
    fn answer(&self, opened: String) -> Result<(), ServiceError> {
        if self.refuse {
            return Err(ServiceError::Failed(
                "Noura opens things only in answer to something you did.".into(),
            ));
        }
        self.opened.lock().unwrap().push(opened);
        Ok(())
    }
}

impl Navigation for Opener {
    fn open_object(&self, id: &str, placement: Placement) -> Result<(), ServiceError> {
        let placement = match placement {
            Placement::Preview => "preview",
            Placement::NewTab => "new-tab",
        };
        self.answer(format!("object {id} {placement}"))
    }

    fn open_page(&self, page: &str) -> Result<(), ServiceError> {
        self.answer(format!("page {page}"))
    }

    fn trash_object(&self, id: &str) -> Result<(), ServiceError> {
        self.answer(format!("trash {id}"))
    }
}

/// The probe with `views`, using `services`, and the folders it lives in.
fn probe_with(
    services: Services,
) -> Option<(Arc<plugin_host::RunningPlugin>, [tempfile::TempDir; 2])> {
    let probe = plugins::built("probe_plugin")?;
    Some(plugins::running_as("probe", &probe, &VIEWS, services))
}

#[test]
fn a_plugin_reads_the_instant_and_time_zone_from_the_apps_clock() {
    let Some((plugin, _folders)) = probe_with(Services {
        clock: Some(Arc::new(FixedClock)),
        ..Services::default()
    }) else {
        return;
    };

    assert_eq!(
        plugins::texts(&plugin.render("clock").unwrap()),
        ["now 2026-10-21T13:30:00Z America/Toronto"]
    );
}

#[test]
fn without_a_clock_of_the_apps_a_plugin_reads_this_computers() {
    let Some((plugin, _folders)) = probe_with(Services::default()) else {
        return;
    };

    let texts = plugins::texts(&plugin.render("clock").unwrap());
    let reading = texts[0].strip_prefix("now ").expect("a reading");
    let (instant, zone) = reading.split_once(' ').expect("an instant and a zone");
    let instant: jiff::Timestamp = instant.parse().expect("an RFC 3339 instant");
    let gap = jiff::Timestamp::now().duration_since(instant);
    assert!(gap.as_secs() < 60, "{reading}");
    assert!(!zone.is_empty(), "{reading}");
}

#[test]
fn a_zone_without_a_name_reads_as_its_offset_from_utc() {
    let instant: jiff::Timestamp = "2026-10-21T13:30:00Z".parse().unwrap();
    let zone = jiff::tz::TimeZone::fixed(jiff::tz::offset(-4));

    assert_eq!(
        ClockReading::at(instant, &zone),
        ClockReading {
            instant: "2026-10-21T13:30:00Z".into(),
            time_zone: "-04:00".into(),
        }
    );
    let named = jiff::tz::TimeZone::get("Asia/Kolkata").unwrap();
    assert_eq!(ClockReading::at(instant, &named).time_zone, "Asia/Kolkata");
}

#[test]
fn a_plugin_asks_the_app_to_open_objects_and_its_own_pages() {
    let opener = Arc::new(Opener::default());
    let navigation: Arc<dyn Navigation> = opener.clone();
    let Some((plugin, _folders)) = probe_with(Services {
        navigation: Some(navigation),
        ..Services::default()
    }) else {
        return;
    };

    for node in [
        "open-object:task_01jbcdefghjkmnpqrstvwxyz0a:new-tab",
        "open-object:task_01jbcdefghjkmnpqrstvwxyz0a:preview",
        "open-page:events",
    ] {
        plugin
            .handle_event("hello", &Event::Pressed(node.into()))
            .unwrap();
    }

    assert_eq!(
        *opener.opened.lock().unwrap(),
        [
            "object task_01jbcdefghjkmnpqrstvwxyz0a new-tab",
            "object task_01jbcdefghjkmnpqrstvwxyz0a preview",
            "page events",
        ]
    );
    assert_eq!(
        plugins::texts(&plugin.render("events").unwrap()),
        ["hello open ok", "hello open ok", "hello open ok"]
    );
}

#[test]
fn a_plugin_moves_only_objects_of_its_own_types_to_the_trash() {
    let opener = Arc::new(Opener::default());
    let navigation: Arc<dyn Navigation> = opener.clone();
    let Some((plugin, _folders)) = probe_with(Services {
        navigation: Some(navigation),
        ..Services::default()
    }) else {
        return;
    };

    // The probe registers no types, so even a task isn't its own.
    plugin
        .handle_event(
            "hello",
            &Event::Pressed("trash-object:task_01jbcdefghjkmnpqrstvwxyz0a".into()),
        )
        .unwrap();

    assert!(opener.opened.lock().unwrap().is_empty());
    assert_eq!(
        plugins::texts(&plugin.render("events").unwrap()),
        ["hello open error capability-not-declared object-types"]
    );
}

#[test]
fn an_open_the_app_refuses_or_cant_carry_out_fails_with_a_reason() {
    let refusing: Arc<dyn Navigation> = Arc::new(Opener {
        refuse: true,
        ..Opener::default()
    });
    let Some((refused, _refused_folders)) = probe_with(Services {
        navigation: Some(refusing),
        ..Services::default()
    }) else {
        return;
    };
    let Some((alone, _alone_folders)) = probe_with(Services::default()) else {
        return;
    };

    for plugin in [&refused, &alone] {
        plugin
            .handle_event("hello", &Event::Pressed("open-page:events".into()))
            .unwrap();
    }

    assert_eq!(
        plugins::texts(&refused.render("events").unwrap()),
        ["hello open error failed Noura opens things only in answer to something you did."]
    );
    assert_eq!(
        plugins::texts(&alone.render("events").unwrap()),
        ["hello open error failed Noura can't open anything for this plugin here."]
    );
}

#[test]
fn a_page_adds_a_sidebar_section_whose_events_reach_the_page() {
    let Some((plugin, _folders)) = probe_with(Services::default()) else {
        return;
    };

    let section = View::from_nodes(plugin.render_sidebar("hello").unwrap()).unwrap();
    assert_eq!(
        section.root().kind,
        ViewKind::Panel {
            label: Some("Probe sidebar".into())
        }
    );
    assert_eq!(
        plugin
            .render_surface(&Surface::Sidebar("hello".into()))
            .unwrap(),
        plugin.render_sidebar("hello").unwrap()
    );
    // A page without a section adds none.
    assert!(plugin.render_sidebar("events").unwrap().is_empty());

    plugin
        .handle_surface_event(
            &Surface::Sidebar("hello".into()),
            &Event::Pressed("side-press".into()),
        )
        .unwrap();
    assert_eq!(
        plugins::texts(&plugin.render("events").unwrap()),
        ["hello pressed side-press"]
    );
}

#[test]
fn a_sidebar_section_needs_the_views_capability() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, _folders) = plugins::running(&probe, &["name = \"commands\""], None);

    assert_eq!(
        plugin.render_sidebar("hello"),
        Err(PluginError::CapabilityNotDeclared {
            capability: "views"
        })
    );
}
