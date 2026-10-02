//! Helpers for tests that drive a session directly: a sink that fails on demand in front of the real
//! one, and the turn, provider and run a session needs.
#![allow(dead_code)]

use super::support::{Fixture, HANDSHAKE, emit, finished, init_event, pilot, text};
use dpm_claude::{
    AppSink, BeginError, Begun, Launch, NewRun, Provider, Receipt, Retry, RunSink, Session,
    SinkError, Turn, start_blocking,
};
use dpm_model::{ActivityInput, ActorId, RunEventId, RunState};
use serde_json::json;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    time::Duration,
};

pub fn never() -> AtomicBool {
    AtomicBool::new(false)
}

pub fn service() -> ActorId {
    ActorId::service("dpm-claude")
}

pub fn new_run(session: &str) -> NewRun {
    NewRun {
        recorder: service(),
        executor: pilot(),
        work_key: "TEST-A".into(),
        parent: None,
        session: session.into(),
        turn: "1".into(),
        provenance: None,
        sources: Vec::new(),
    }
}

pub fn turn(session: &str, deadline: Duration) -> Turn {
    Turn {
        session: session.into(),
        prompt: "check".into(),
        deadline,
        announce_wait: Duration::ZERO,
        batch: 10,
        grace: Duration::from_millis(200),
        retry: Retry {
            attempts: 5,
            backoff: Duration::ZERO,
            budget: Duration::from_secs(5),
        },
    }
}

pub fn provider(fixture: &Fixture, script: &Path, session: &str) -> Provider {
    provider_with_queue(fixture, script, session, 64)
}

pub fn provider_with_queue(
    fixture: &Fixture,
    script: &Path,
    session: &str,
    queue: usize,
) -> Provider {
    let launch = Launch {
        program: script.to_path_buf(),
        args: vec![format!("--session-id={session}").into()],
        directory: fixture.work_dir.clone(),
        remove_env: Vec::new(),
    };
    Provider::spawn_blocking(&launch, queue, 1 << 20).expect("spawn the provider")
}

/// A sink that fails on demand in front of the real one.
pub struct Flaky {
    pub inner: AppSink,
    pub busy: u32,
    pub refuse_records: bool,
    pub refuse_terminal: bool,
    /// Signals once, from inside the write, when this many records have been recorded.
    pub notify: Option<(usize, Sender<()>)>,
    /// Raised from inside the first successful write: the provider is demonstrably running.
    pub cancel_after_first: Option<Arc<AtomicBool>>,
    pub recorded: usize,
}

impl RunSink for Flaky {
    fn record_blocking(&mut self, entries: Vec<ActivityInput>) -> Result<Receipt, SinkError> {
        if self.refuse_records {
            return Err(SinkError::Refused {
                context: "injected",
                source: "the store refuses".into(),
            });
        }
        if self.busy > 0 {
            self.busy -= 1;
            return Err(SinkError::Busy {
                context: "injected",
                source: "the store is busy".into(),
            });
        }
        let count = entries.len();
        let receipt = self.inner.record_blocking(entries)?;
        self.recorded += count;
        if let Some(cancel) = self.cancel_after_first.take() {
            cancel.store(true, Ordering::SeqCst);
        }
        if self
            .notify
            .as_ref()
            .is_some_and(|(threshold, _)| self.recorded >= *threshold)
            && let Some((_, signal)) = self.notify.take()
        {
            signal.send(()).ok();
        }
        Ok(receipt)
    }

    fn transition_blocking(
        &mut self,
        state: RunState,
        event: RunEventId,
        detail: String,
    ) -> Result<(), SinkError> {
        if self.refuse_terminal {
            return Err(SinkError::Refused {
                context: "injected",
                source: "the terminal write is refused".into(),
            });
        }
        self.inner.transition_blocking(state, event, detail)
    }
}

pub fn flaky(
    fixture: &Fixture,
    session: &str,
    busy: u32,
    refuse_records: bool,
    refuse_terminal: bool,
) -> impl FnOnce(Option<&Session>) -> Result<Begun<Flaky>, BeginError> {
    hooked(fixture, session, move |sink| {
        sink.busy = busy;
        sink.refuse_records = refuse_records;
        sink.refuse_terminal = refuse_terminal;
    })
}

/// A sink in front of the real one, configured before the run begins.
pub fn hooked(
    fixture: &Fixture,
    session: &str,
    configure: impl FnOnce(&mut Flaky),
) -> impl FnOnce(Option<&Session>) -> Result<Begun<Flaky>, BeginError> {
    let mut app = fixture.app();
    let session = session.to_string();
    move |_| {
        let started = start_blocking(&mut app, &new_run(&session))
            .map_err(|error| BeginError::new("recording the run", error))?;
        let inner = AppSink::new(app, service(), started.run, started.lineage);
        let mut sink = Flaky {
            inner,
            busy: 0,
            refuse_records: false,
            refuse_terminal: false,
            notify: None,
            cancel_after_first: None,
            recorded: 0,
        };
        configure(&mut sink);
        Ok(Begun {
            sink,
            run: started.run,
        })
    }
}

pub fn five_events() -> String {
    [
        HANDSHAKE.to_string(),
        emit(&init_event()),
        (1..=5)
            .map(|index| emit(&text(&format!("e{index}"), &format!("step {index}"))))
            .collect::<String>(),
        emit(&finished(&json!([]))),
    ]
    .concat()
}
