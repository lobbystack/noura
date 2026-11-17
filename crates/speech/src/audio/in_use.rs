//! Whether another app records the microphone, as for a call, on Windows
//! and Linux, adapted from anarlog's `list_mic_using_apps` in
//! `crates/detect/src/list/windows.rs` and `crates/detect/src/list/linux.rs`.
//! Noura's own recordings don't count. macOS asks Core Audio in
//! `system.rs`.

/// Whether an app other than Noura has an active capture session on any
/// microphone, from WASAPI's audio sessions. `None` when Windows doesn't
/// answer.
#[cfg(target_os = "windows")]
pub(super) fn microphone_in_use() -> Option<bool> {
    use wasapi::{DeviceEnumerator, Direction, SessionState, deinitialize, initialize_mta};

    initialize_mta().ok().ok()?;
    let answer = (|| {
        let enumerator = DeviceEnumerator::new().ok()?;
        let devices = enumerator.get_device_collection(Direction::Capture).ok()?;
        let ours = std::process::id();
        let mut in_use = false;
        for device in &devices {
            let Ok(device) = device else {
                continue;
            };
            let Ok(sessions) = device
                .get_iaudiosessionmanager()
                .and_then(|manager| manager.get_audiosessionenumerator())
            else {
                continue;
            };
            let Ok(count) = sessions.get_count() else {
                continue;
            };
            for index in 0..count {
                let Ok(session) = sessions.get_session(index) else {
                    continue;
                };
                if !matches!(session.get_state(), Ok(SessionState::Active)) {
                    continue;
                }
                if session
                    .get_process_id()
                    .is_ok_and(|process| process != 0 && process != ours)
                {
                    in_use = true;
                }
            }
        }
        Some(in_use)
    })();
    deinitialize();
    answer
}

/// Whether an app other than Noura records from a source, from the sound
/// server's source outputs over the PulseAudio protocol, which PipeWire
/// serves too. A corked output is paused and doesn't count. `None` when
/// no sound server answers.
#[cfg(target_os = "linux")]
pub(super) fn microphone_in_use() -> Option<bool> {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use libpulse_binding::callbacks::ListResult;
    use libpulse_binding::context::{Context, FlagSet, State};
    use libpulse_binding::mainloop::standard::{IterateResult, Mainloop};

    const TIMEOUT: Duration = Duration::from_secs(2);

    /// Run the main loop once, waiting a little when nothing happened.
    fn iterate(mainloop: &mut Mainloop) -> Option<()> {
        match mainloop.iterate(false) {
            IterateResult::Success(0) => std::thread::sleep(Duration::from_millis(10)),
            IterateResult::Success(_) => {}
            IterateResult::Quit(_) | IterateResult::Err(_) => return None,
        }
        Some(())
    }

    #[derive(Default)]
    struct Query {
        in_use: bool,
        done: bool,
        failed: bool,
    }

    let mut mainloop = Mainloop::new()?;
    let mut context = Context::new(&mainloop, "Noura")?;
    context.connect(None, FlagSet::NOFLAGS, None).ok()?;
    let deadline = Instant::now() + TIMEOUT;
    let ready = loop {
        match context.get_state() {
            State::Ready => break true,
            State::Failed | State::Terminated => break false,
            _ => {}
        }
        if Instant::now() >= deadline || iterate(&mut mainloop).is_none() {
            break false;
        }
    };
    if !ready {
        context.disconnect();
        return None;
    }
    let ours = std::process::id().to_string();
    let query = Rc::new(RefCell::new(Query::default()));
    let heard = query.clone();
    let mut operation = context
        .introspect()
        .get_source_output_info_list(move |result| {
            let mut query = heard.borrow_mut();
            match result {
                ListResult::Item(output) => {
                    let process = output.proplist.get_str("application.process.id");
                    if !output.corked && process.as_deref() != Some(ours.as_str()) {
                        query.in_use = true;
                    }
                }
                ListResult::End => query.done = true,
                ListResult::Error => {
                    query.failed = true;
                    query.done = true;
                }
            }
        });
    let answered = loop {
        if query.borrow().done {
            break !query.borrow().failed;
        }
        if Instant::now() >= deadline || iterate(&mut mainloop).is_none() {
            break false;
        }
    };
    if !query.borrow().done {
        operation.cancel();
    }
    drop(operation);
    context.disconnect();
    let in_use = query.borrow().in_use;
    answered.then_some(in_use)
}
