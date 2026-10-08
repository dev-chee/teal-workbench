use std::sync::{Arc, Mutex};

use teal_security::{AuditContext, AuditError, AuditSink, Event, Outcome, Phase, Receipt};
use teal_browser::{BrowserManager, BrowserSessionId};

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

#[tokio::test]
async fn browser_mutation_requires_a_sink_before_validating_or_acting() {
    let mut browser = BrowserManager::default();
    let error = browser
        .close(&BrowserSessionId("missing-session".into()))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("audit sink is required"));
}

#[tokio::test]
async fn explicit_sink_records_failed_browser_actions_with_paired_identity() {
    let sink = Arc::new(RecordingSink::default());
    let mut browser = BrowserManager::default().with_audit_context(AuditContext::new(sink.clone()));
    browser
        .close(&BrowserSessionId("missing-session".into()))
        .await
        .unwrap_err();

    let events = sink.0.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].phase, Phase::Intent);
    assert_eq!(events[0].outcome, Outcome::Pending);
    assert_eq!(events[1].phase, Phase::Result);
    assert_eq!(events[1].outcome, Outcome::Failed);
    assert_eq!(events[0].correlation_id, events[1].correlation_id);
}
