use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use teal_security::{AuditContext, AuditError, AuditSink, Event, Outcome, Phase, Receipt};
use teal_computer::{
    ComputerBackend, ComputerElement, ComputerError, ComputerSnapshot, ComputerWindow,
    audited_backend,
};

#[derive(Default)]
struct RecordingSink(Mutex<Vec<Event>>);

impl AuditSink for RecordingSink {
    fn append(&self, event: &Event) -> Result<Receipt, AuditError> {
        let mut events = self.0.lock().unwrap();
        events.push(event.clone());
        Ok(Receipt {
            sequence: events.len() as u64,
            hash: format!("{:064x}", events.len()),
        })
    }
}

fn missing_window() -> ComputerWindow {
    ComputerWindow {
        process_id: u32::MAX,
        window_handle: "0".into(),
        title: "missing".into(),
        generation: 0,
    }
}

#[derive(Default)]
struct FakeComputer {
    calls: AtomicUsize,
    unknown: bool,
}

impl ComputerBackend for FakeComputer {
    fn list_windows(&self) -> Result<Vec<ComputerWindow>, ComputerError> {
        Ok(Vec::new())
    }

    fn snapshot(&self, window: &ComputerWindow) -> Result<ComputerSnapshot, ComputerError> {
        Ok(ComputerSnapshot {
            window: window.clone(),
            elements: Vec::<ComputerElement>::new(),
        })
    }

    fn invoke(&self, _: &ComputerWindow, _: &str) -> Result<(), ComputerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.unknown {
            Err(ComputerError::UnknownEffect(
                "fixture action may have occurred".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn set_value(&self, _: &ComputerWindow, _: &str, _: &str) -> Result<(), ComputerError> {
        Ok(())
    }

    fn raw_click(&self, _: &ComputerWindow, _: &str) -> Result<(), ComputerError> {
        Ok(())
    }

    fn raw_keys(&self, _: &ComputerWindow, _: &str, _: &str) -> Result<(), ComputerError> {
        Ok(())
    }
}

#[test]
fn computer_mutation_requires_a_sink_before_platform_access() {
    let inner = Arc::new(FakeComputer::default());
    let error = audited_backend(inner.clone(), AuditContext::default())
        .invoke(&missing_window(), "1.2.3")
        .unwrap_err();
    assert!(error.to_string().contains("audit sink is required"));
    assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn explicit_sink_records_successful_computer_actions_with_paired_identity() {
    let sink = Arc::new(RecordingSink::default());
    let inner = Arc::new(FakeComputer::default());
    let backend = audited_backend(inner.clone(), AuditContext::new(sink.clone()));
    backend.invoke(&missing_window(), "1.2.3").unwrap();
    assert_eq!(inner.calls.load(Ordering::SeqCst), 1);

    let events = sink.0.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].phase, Phase::Intent);
    assert_eq!(events[0].outcome, Outcome::Pending);
    assert_eq!(events[1].phase, Phase::Result);
    assert_eq!(events[1].outcome, Outcome::Succeeded);
    assert_eq!(events[0].correlation_id, events[1].correlation_id);
}

#[test]
fn unknown_computer_effect_is_audited_as_unknown_and_preserved() {
    let sink = Arc::new(RecordingSink::default());
    let inner = Arc::new(FakeComputer {
        calls: AtomicUsize::new(0),
        unknown: true,
    });
    let backend = audited_backend(inner.clone(), AuditContext::new(sink.clone()));

    let error = backend.invoke(&missing_window(), "1.2.3").unwrap_err();

    assert!(matches!(error, ComputerError::UnknownEffect(_)));
    assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
    let events = sink.0.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].phase, Phase::Intent);
    assert_eq!(events[1].phase, Phase::Result);
    assert_eq!(events[1].outcome, Outcome::Unknown);
    assert_eq!(events[0].correlation_id, events[1].correlation_id);
}
