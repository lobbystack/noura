//! What starts Orbs without you, while Noura is open on this device:
//! schedule triggers, the events plugins emit, and the runs a quit left
//! `running`. A thread of its own checks every 30 seconds, and at once
//! when an event arrives.
//!
//! The design borrows from IronClaw's and ZeroClaw's schedulers (see
//! `docs/provenance/orbs-design.md`): a key names each firing, so a run
//! starts once for it whatever restarts or rebuilt indexes come between;
//! one run at a time works per Orb; and the schedules missed while Noura
//! was closed catch up as one run, at the last time they missed. Noura
//! keeps the keys in run records, which are workspace files, so a deleted
//! index rebuilds them.
//!
//! When you haven't used Noura for [`INACTIVE_AFTER`], Noura holds every
//! Orb's triggers until you say whether to keep them running, as Grok Bot
//! does. The hold and the time you were last active are this device's, in
//! its app data.

use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, Weak},
    time::{Duration, Instant},
};

use jiff::{SignedDuration, Timestamp};
use plugin_host::{EventDeclaration, PluginEvents, ServiceError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use workspace_format::orb::{self, StartedBy, Trigger};

use super::{INTERRUPTED, Listed, ORB_PLUGIN, OrbJob, Start, list, runs, update_record};
use crate::ai::{Ai, Ask, lock, service_error};

/// How often the runner checks the triggers while Noura is open.
const TICK: Duration = Duration::from_secs(30);

/// How long after a workspace opens the runner waits before it takes up
/// the runs a quit left and catches up on schedules, so the plugins whose
/// tools those runs offer are running.
const STARTUP_GRACE: Duration = Duration::from_secs(10);

/// How long without you before Noura holds every Orb's triggers.
pub(crate) const INACTIVE_AFTER: SignedDuration = SignedDuration::from_hours(7 * 24);

/// The most runs triggers start for one Orb in an hour.
const RUNS_PER_HOUR: usize = 20;

/// How long an event waits for an Orb that is still working.
const EVENT_WAIT: SignedDuration = SignedDuration::from_hours(1);

/// The most events that wait at once.
const MAX_EVENTS: usize = 64;

/// The file in the app data folder that holds when you last used Noura
/// and whether Orbs' triggers are held.
const ACTIVITY_FILE: &str = "ai/orbs.json";

/// When you last used Noura on this device, and since when Noura holds the
/// Orbs' triggers, as RFC 3339 instants.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Activity {
    #[serde(default)]
    last_active: Option<String>,
    #[serde(default)]
    held_since: Option<String>,
}

/// An event a plugin emitted, waiting for the Orbs its triggers start.
#[derive(Debug, Clone)]
struct Pending {
    /// `<plugin>.<event>`.
    name: String,
    /// What happened, as the plugin's manifest says.
    title: String,
    object: Option<String>,
    at: Timestamp,
    /// The Orbs it started, or that can't take it.
    done: HashSet<String>,
}

/// What the triggers keep between checks.
#[derive(Default)]
pub(crate) struct State {
    events: VecDeque<Pending>,
    /// The trigger keys this session started, as their records may not
    /// list yet.
    started: HashSet<String>,
    /// When triggers started each Orb's runs, for the hourly cap.
    recent: std::collections::HashMap<String, VecDeque<Timestamp>>,
    /// When the open workspace opened, for the startup grace.
    opened: Option<Instant>,
    /// The runs a quit left were taken up.
    recovered: bool,
    activity: Activity,
    /// Where `activity` lives.
    file: PathBuf,
}

/// Wakes the runner thread, and tells it to stop.
#[derive(Default)]
pub(crate) struct Signal {
    state: Mutex<SignalState>,
    wake: Condvar,
}

#[derive(Default)]
struct SignalState {
    /// Counts the workspaces opened. A runner of an earlier one stops.
    generation: u64,
    stop: bool,
    nudged: bool,
}

impl Signal {
    /// Wake the runner now.
    pub(crate) fn nudge(&self) {
        lock(&self.state).nudged = true;
        self.wake.notify_all();
    }

    /// Stop every runner.
    pub(crate) fn stop(&self) {
        lock(&self.state).stop = true;
        self.wake.notify_all();
    }

    fn next_generation(&self) -> u64 {
        let mut state = lock(&self.state);
        state.generation += 1;
        state.generation
    }

    /// Wait until the next tick or a nudge. Returns false once the runner
    /// of `generation` should stop.
    fn wait(&self, generation: u64) -> bool {
        let mut state = lock(&self.state);
        if !state.nudged && !state.stop && state.generation == generation {
            state = self
                .wake
                .wait_timeout(state, TICK)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        state.nudged = false;
        !state.stop && state.generation == generation
    }
}

impl State {
    /// The triggers' state, with the activity this device's app data holds.
    pub(crate) fn load(app_data: &Path) -> Self {
        let file = app_data.join(ACTIVITY_FILE);
        let activity = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            activity,
            file,
            ..Self::default()
        }
    }

    fn save_activity(&self) {
        let written = (|| {
            if let Some(folder) = self.file.parent() {
                std::fs::create_dir_all(folder)?;
            }
            let bytes = serde_json::to_vec_pretty(&self.activity).map_err(std::io::Error::other)?;
            let partial = self.file.with_extension("json.partial");
            std::fs::write(&partial, bytes)?;
            std::fs::rename(&partial, &self.file)
        })();
        if let Err(error) = written {
            tracing::warn!(%error, "when you last used Noura wasn't saved");
        }
    }
}

/// What one plugin's events reach: the Orbs service.
struct Events {
    ai: Weak<Ai>,
    plugin: String,
    declared: Vec<EventDeclaration>,
}

impl PluginEvents for Events {
    fn emit(&self, event: &str, object: Option<&str>) -> Result<(), ServiceError> {
        let Some(declaration) = self.declared.iter().find(|known| known.id == event) else {
            return Err(ServiceError::Failed(format!(
                "The manifest declares no event \u{201c}{event}\u{201d}."
            )));
        };
        // An event during an Orb's tool call is that Orb's own doing, and
        // starts no Orb, so two Orbs can't start each other in a loop.
        if let Some(run) = super::current_cause() {
            tracing::info!(%run, event = %format!("{}.{event}", self.plugin), "an Orb's own event started no Orb");
            return Ok(());
        }
        if let Some(ai) = self.ai.upgrade() {
            ai.queue_event(
                format!("{}.{event}", self.plugin),
                declaration.title.clone(),
                object.map(str::to_owned),
            );
        }
        Ok(())
    }
}

impl Ai {
    /// What the plugin `plugin`, which declares `declared`, reaches through
    /// the `events` interface.
    pub(crate) fn orb_events(
        self: &Arc<Self>,
        plugin: &str,
        declared: Vec<EventDeclaration>,
    ) -> Arc<dyn PluginEvents> {
        Arc::new(Events {
            ai: Arc::downgrade(self),
            plugin: plugin.to_owned(),
            declared,
        })
    }

    /// The event `name` happened, about `object`: the Orbs whose triggers
    /// name it start at the runner's next check, which comes at once.
    fn queue_event(&self, name: String, title: String, object: Option<String>) {
        if self.orbs_held() {
            tracing::info!(%name, "Orbs' triggers are held, so the event started none");
            return;
        }
        {
            let mut state = lock(&self.orb_triggers);
            if state.events.len() >= MAX_EVENTS {
                state.events.pop_front();
            }
            state.events.push_back(Pending {
                name,
                title,
                object,
                at: self.system.now(),
                done: HashSet::new(),
            });
        }
        self.orb_signal.nudge();
    }

    /// Start the runner for the workspace that just opened, after Noura
    /// checks how long you were away. The runner of the workspace before
    /// stops.
    pub(crate) fn start_orb_runner(self: &Arc<Self>) {
        {
            let mut state = lock(&self.orb_triggers);
            state.events.clear();
            state.started.clear();
            state.recent.clear();
            state.recovered = false;
            state.opened = Some(Instant::now());
        }
        lock(&self.orb_threads).clear();
        self.check_inactivity();
        self.note_activity();
        let generation = self.orb_signal.next_generation();
        let signal = self.orb_signal.clone();
        let ai = Arc::downgrade(self);
        let started = std::thread::Builder::new()
            .name("orbs".to_owned())
            .spawn(move || {
                while signal.wait(generation) {
                    let Some(ai) = ai.upgrade() else {
                        return;
                    };
                    let settled = lock(&ai.orb_triggers)
                        .opened
                        .is_some_and(|opened| opened.elapsed() >= STARTUP_GRACE);
                    ai.check_orbs_with(settled);
                }
            });
        match started {
            Ok(thread) => {
                let mut runner = lock(&self.orb_runner);
                runner.retain(|thread| !thread.is_finished());
                runner.push(thread);
            }
            Err(error) => tracing::warn!(%error, "the Orbs' runner didn't start"),
        }
    }

    /// Check every trigger now: take up the runs a quit left, catch up on
    /// missed schedules, and start the Orbs that waiting events name.
    pub(crate) fn check_orbs(self: &Arc<Self>) {
        self.check_orbs_with(true);
    }

    /// Check the triggers: only the waiting events, or everything once the
    /// plugins had time to start.
    fn check_orbs_with(self: &Arc<Self>, everything: bool) {
        let _checking = lock(&self.orb_checking);
        let Ok(engine) = self.engine() else {
            return;
        };
        if everything && !std::mem::replace(&mut lock(&self.orb_triggers).recovered, true) {
            self.recover_orb_runs(&engine);
        }
        self.check_inactivity();
        let held = self.orbs_held();
        let orbs = match list(&engine) {
            Ok(orbs) => orbs,
            Err(error) => {
                tracing::warn!(code = %error.code, "Noura couldn't list the Orbs");
                return;
            }
        };
        let records = match runs(&engine, None) {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(code = %error.code, "Noura couldn't list the Orbs' runs");
                return;
            }
        };
        let keys: HashSet<String> = records
            .iter()
            .filter_map(|run| run.trigger_key.clone())
            .collect();
        let ready: Vec<(&orb::Orb, Option<&str>)> = orbs
            .iter()
            .filter_map(|listed| match listed {
                Listed::Orb(orb, changed) if !orb.paused => {
                    Some((orb.as_ref(), changed.as_deref()))
                }
                _ => None,
            })
            .collect();
        if !held {
            self.fire_events(&engine, &ready, &keys);
        } else {
            lock(&self.orb_triggers).events.clear();
        }
        if everything && !held {
            self.fire_schedules(&engine, &ready, &records, &keys);
        }
    }

    /// Start the Orbs that the waiting events name.
    fn fire_events(
        self: &Arc<Self>,
        engine: &Arc<local_core::WorkspaceEngine>,
        orbs: &[(&orb::Orb, Option<&str>)],
        keys: &HashSet<String>,
    ) {
        let now = self.system.now();
        let events: Vec<Pending> = {
            let mut state = lock(&self.orb_triggers);
            state
                .events
                .retain(|event| now.duration_since(event.at) < EVENT_WAIT);
            state.events.iter().cloned().collect()
        };
        for event in events {
            let mut done = event.done.clone();
            let revision = event
                .object
                .as_deref()
                .and_then(|id| engine.read_object(id).ok())
                .map(|object| object.revision.chars().take(16).collect::<String>());
            let occurrence = match (&event.object, &revision) {
                (Some(object), Some(revision)) => format!("{object}@{revision}"),
                (Some(object), None) => object.clone(),
                (None, _) => event.at.to_string(),
            };
            for (orb, _) in orbs {
                if done.contains(&orb.id) {
                    continue;
                }
                let Some(trigger) = orb.triggers.iter().find(|trigger| {
                    matches!(trigger, Trigger::Event(name) if name.to_string() == event.name)
                }) else {
                    done.insert(orb.id.clone());
                    continue;
                };
                let key = orb::trigger_key(&orb.id, trigger, &occurrence);
                if keys.contains(&key) || lock(&self.orb_triggers).started.contains(&key) {
                    done.insert(orb.id.clone());
                    continue;
                }
                if self.orb_busy(&orb.id) {
                    // It takes the event once its run ends.
                    continue;
                }
                done.insert(orb.id.clone());
                let message =
                    super::event_message(engine, orb, &event.title, event.object.as_deref());
                self.fire(
                    engine,
                    Start {
                        orb: (*orb).clone(),
                        plugin: ORB_PLUGIN.to_owned(),
                        started_by: StartedBy::Event,
                        trigger: Some(trigger.canonical()),
                        key: Some(key),
                        object_id: event.object.clone(),
                        slot: None,
                        message,
                    },
                );
            }
            let mut state = lock(&self.orb_triggers);
            let everyone = orbs.iter().all(|(orb, _)| done.contains(&orb.id));
            state.events.retain_mut(|known| {
                if known.at != event.at || known.name != event.name || known.object != event.object
                {
                    return true;
                }
                known.done.clone_from(&done);
                !everyone
            });
        }
    }

    /// Start the schedules whose time came: each once for its last time
    /// since its previous run, or since its Orb's file last changed.
    fn fire_schedules(
        self: &Arc<Self>,
        engine: &Arc<local_core::WorkspaceEngine>,
        orbs: &[(&orb::Orb, Option<&str>)],
        records: &[orb::OrbRun],
        keys: &HashSet<String>,
    ) {
        let now = self.system.now();
        let zone = self.system.time_zone();
        for (orb, changed) in orbs {
            for trigger in &orb.triggers {
                let Trigger::Schedule(schedule) = trigger else {
                    continue;
                };
                let canonical = trigger.canonical();
                let last = records
                    .iter()
                    .filter(|run| {
                        run.orb_id == orb.id && run.trigger.as_deref() == Some(&canonical)
                    })
                    .filter_map(|run| run.slot.as_deref()?.parse::<Timestamp>().ok())
                    .max();
                let changed = changed.and_then(|changed| changed.parse::<Timestamp>().ok());
                let Some(since) = last.max(changed) else {
                    continue;
                };
                let Some(slot) = schedule.missed(since, now, &zone) else {
                    continue;
                };
                let key = orb::trigger_key(&orb.id, trigger, &slot.to_string());
                if keys.contains(&key)
                    || lock(&self.orb_triggers).started.contains(&key)
                    || self.orb_busy(&orb.id)
                {
                    continue;
                }
                let local = slot
                    .to_zoned(zone.clone())
                    .strftime("%Y-%m-%d %H:%M")
                    .to_string();
                let message = super::heartbeat(engine, orb).map_or_else(
                    || {
                        format!(
                            "**Schedule** `{}` · {local}\n\nYour scheduled time came.",
                            schedule.pattern()
                        )
                    },
                    |checklist| {
                        format!(
                            "**Schedule** `{}` · {local}\n\n{checklist}",
                            schedule.pattern()
                        )
                    },
                );
                self.fire(
                    engine,
                    Start {
                        orb: (*orb).clone(),
                        plugin: ORB_PLUGIN.to_owned(),
                        started_by: StartedBy::Schedule,
                        trigger: Some(canonical),
                        key: Some(key),
                        object_id: None,
                        slot: Some(slot.to_string()),
                        message,
                    },
                );
            }
        }
    }

    /// Start a run a trigger fired, unless the Orb started its hourly
    /// share of them already.
    fn fire(self: &Arc<Self>, engine: &Arc<local_core::WorkspaceEngine>, start: Start) {
        let now = self.system.now();
        {
            let mut state = lock(&self.orb_triggers);
            let recent = state.recent.entry(start.orb.id.clone()).or_default();
            recent.retain(|at| now.duration_since(*at) < SignedDuration::from_hours(1));
            if recent.len() >= RUNS_PER_HOUR {
                tracing::warn!("an Orb's triggers started their hourly share of runs already");
                return;
            }
            recent.push_back(now);
            if let Some(key) = &start.key {
                state.started.insert(key.clone());
            }
        }
        if let Err(error) = self.start_orb_run(engine, start) {
            tracing::warn!(?error, "a trigger's run didn't start");
        }
    }

    /// Take up the runs that a quit left `running`: answer again once, or
    /// mark interrupted the runs a quit stopped during a tool call, whose
    /// call Noura never runs again by itself.
    fn recover_orb_runs(self: &Arc<Self>, engine: &Arc<local_core::WorkspaceEngine>) {
        let Ok(records) = runs(engine, None) else {
            return;
        };
        for record in records
            .into_iter()
            .filter(|run| run.status == orb::RunStatus::Running)
        {
            if lock(&self.state).runs.contains_key(&record.id) {
                continue;
            }
            let read = engine
                .recover_interrupted_chat(&record.chat_id)
                .or_else(|_| engine.read_chat(&record.chat_id));
            let messages = read.map(|read| read.messages).unwrap_or_default();
            let orb = super::read(engine, &record.orb_id).ok();
            let resumable = orb.is_some()
                && record.resumed == 0
                && super::asked(&messages, &record.id)
                && !super::call_unfinished(&messages, &record.id);
            let interrupted = |ai: &Self| {
                let finished = ai.system.now().to_string();
                if let Err(error) = update_record(
                    engine,
                    &record.id,
                    &[
                        (
                            "status",
                            Value::String(orb::RunStatus::Interrupted.as_str().to_owned()),
                        ),
                        ("error_code", Value::String(INTERRUPTED.to_owned())),
                        ("finished", Value::String(finished)),
                    ],
                    None,
                ) {
                    tracing::warn!(code = %error.code, "an interrupted Orb run kept its record");
                }
            };
            let Some(orb) = orb.filter(|_| resumable) else {
                interrupted(self);
                continue;
            };
            let Ok(provider) = self.orb_provider(&orb) else {
                interrupted(self);
                continue;
            };
            if let Err(error) = update_record(
                engine,
                &record.id,
                &[("resumed", Value::from(record.resumed + 1))],
                None,
            ) {
                tracing::warn!(code = %error.code, "an Orb run wasn't taken up");
                continue;
            }
            let offered = self
                .toolbox()
                .map(|toolbox| toolbox.tools())
                .unwrap_or_default();
            let tools = super::allowed_tools(&orb.tools, &offered, self.web_ready());
            let job = OrbJob {
                orb_id: orb.id.clone(),
                title: orb.title.clone(),
                model: orb.model.clone(),
                effort: orb.effort,
                rounds: 0,
                tool_calls: record.tool_calls,
                log: record.log.lines().map(str::to_owned).collect(),
                settling: false,
                settled: false,
            };
            if let Err(error) = self.begin_orb(
                &record.id,
                ORB_PLUGIN,
                &record.chat_id,
                &provider,
                Ask::Resume,
                tools,
                job,
            ) {
                tracing::warn!(?error, "an Orb run wasn't taken up");
            }
        }
    }

    /// Whether Noura holds every Orb's triggers because you were away.
    pub(crate) fn orbs_held(&self) -> bool {
        lock(&self.orb_triggers).activity.held_since.is_some()
    }

    /// Hold every Orb's triggers when you haven't used Noura on this device
    /// for [`INACTIVE_AFTER`].
    fn check_inactivity(&self) {
        let now = self.system.now();
        let held = {
            let mut state = lock(&self.orb_triggers);
            if state.activity.held_since.is_some() {
                return;
            }
            let away = state
                .activity
                .last_active
                .as_deref()
                .and_then(|last| last.parse::<Timestamp>().ok())
                .is_some_and(|last| now.duration_since(last) >= INACTIVE_AFTER);
            if away {
                state.activity.held_since = Some(now.to_string());
                state.save_activity();
            }
            away
        };
        if held {
            tracing::info!("Orbs' triggers are held until you say whether to keep them running");
            self.changed();
        }
    }

    /// You used Noura now. Noura writes it down at most once a minute.
    pub(crate) fn note_activity(&self) {
        let now = self.system.now();
        let mut state = lock(&self.orb_triggers);
        let recent = state
            .activity
            .last_active
            .as_deref()
            .and_then(|last| last.parse::<Timestamp>().ok())
            .is_some_and(|last| {
                let since = now.duration_since(last);
                since >= SignedDuration::ZERO && since < SignedDuration::from_mins(1)
            });
        if !recent {
            state.activity.last_active = Some(now.to_string());
            state.save_activity();
        }
    }

    /// Your answer after a while away: keep the triggers running, so the
    /// schedules catch up, or pause every Orb that has triggers.
    pub(crate) fn keep_orbs_running(self: &Arc<Self>, keep: bool) -> Result<(), ServiceError> {
        if !keep {
            let engine = self.engine()?;
            for listed in list(&engine).map_err(service_error)? {
                let Listed::Orb(orb, _) = listed else {
                    continue;
                };
                if orb.paused || orb.triggers.is_empty() {
                    continue;
                }
                self.set_orb_paused(&orb.id, true, &orb.revision)?;
            }
        }
        {
            let mut state = lock(&self.orb_triggers);
            state.activity.held_since = None;
            state.activity.last_active = Some(self.system.now().to_string());
            state.save_activity();
        }
        self.changed();
        self.orb_signal.nudge();
        Ok(())
    }
}
