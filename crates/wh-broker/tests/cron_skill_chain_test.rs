//! Acceptance tests for Story 5-7: Cron Triggers Skill Invocation End-to-End.
//!
//! Tests validate the full cron -> agent -> skill -> result -> summary chain.

use std::collections::HashMap;
use std::sync::Arc;

use prost_types::Timestamp;
use tokio::sync::mpsc;
use wh_broker::cron::chain::{ChainEvent, NotificationType};
use wh_broker::cron::dispatcher::CronEventDispatcher;
use wh_broker::cron::orchestrator::CronSkillChain;
use wh_broker::cron::skill_handler::CronSkillHandler;
use wh_broker::cron::CronEventMessage;

/// Helper: set up a full chain for testing.
fn setup_test_chain(
    job_name: &str,
    skill_name: &str,
) -> (CronSkillChain, mpsc::Sender<ChainEvent>) {
    let (event_tx, event_rx) = mpsc::channel(100);
    let mut dispatcher = CronEventDispatcher::new();
    let handler = Arc::new(CronSkillHandler {
        skill_name: skill_name.into(),
        agent_id: "test-agent".into(),
        event_sender: event_tx.clone(),
    });
    dispatcher.register_handler(job_name, handler);

    let chain = CronSkillChain::new(dispatcher, event_rx, "test-agent");
    (chain, event_tx)
}

/// AC1: Full chain completes without human intervention.
#[tokio::test]
async fn test_ac1_full_chain_completes_without_human_intervention() {
    let (mut chain, _tx) = setup_test_chain("echo-cron", "echo");

    let event = CronEventMessage {
        job_name: "echo-cron".into(),
        action: "event".into(),
        schedule: "*/1 * * * *".into(),
        triggered_at: Timestamp {
            seconds: 0,
            nanos: 0,
        },
        payload: [("input".into(), "hello".into())].into_iter().collect(),
        target_stream: "test-stream".into(),
    };

    let outcome = chain.process_cron_event(event).await.unwrap();

    // Chain completed successfully without any human intervention
    assert!(outcome.success);
    assert!(!outcome.summary_text.is_empty());

    // Verify TextMessage summary is in the chain
    let last_event = outcome.events.last().unwrap();
    match last_event {
        ChainEvent::TextMessagePublished { content, .. } => {
            assert!(content.contains("CRON CHAIN OK"));
            assert!(content.contains("echo-cron"));
            assert!(content.contains("echo"));
        }
        _ => panic!("expected TextMessagePublished as final event"),
    }
}

/// AC2: Chain log contains all event types in order with monotonic timestamps.
#[tokio::test]
async fn test_ac2_chain_events_in_order_with_timestamps() {
    let (mut chain, _tx) = setup_test_chain("echo-cron", "echo");

    let event = CronEventMessage {
        job_name: "echo-cron".into(),
        action: "event".into(),
        schedule: "*/1 * * * *".into(),
        triggered_at: Timestamp {
            seconds: 0,
            nanos: 0,
        },
        payload: HashMap::new(),
        target_stream: "test-stream".into(),
    };

    let outcome = chain.process_cron_event(event).await.unwrap();

    // Verify exactly 5 events
    assert_eq!(outcome.events.len(), 5, "expected 5 chain events");

    // Verify event types in order
    let expected_types = [
        "CronEvent",
        "SkillInvocation",
        "SkillProgress",
        "SkillResult",
        "TextMessage",
    ];
    for (i, expected_type) in expected_types.iter().enumerate() {
        assert_eq!(
            outcome.events[i].type_name(),
            *expected_type,
            "event {} should be {} but was {}",
            i,
            expected_type,
            outcome.events[i].type_name()
        );
    }

    // Verify monotonically increasing timestamps
    for window in outcome.events.windows(2) {
        assert!(
            window[1].timestamp_ms() >= window[0].timestamp_ms(),
            "timestamps not monotonic: {} at {} < {} at {}",
            window[0].type_name(),
            window[0].timestamp_ms(),
            window[1].type_name(),
            window[1].timestamp_ms()
        );
    }

    // Verify format_chain_log produces readable output
    let log = chain.format_chain_log();
    assert_eq!(log.len(), 5);
    assert!(log[0].starts_with("CronEvent"));
    assert!(log[1].starts_with("SkillInvocation"));
    assert!(log[2].starts_with("SkillProgress"));
    assert!(log[3].starts_with("SkillResult"));
    assert!(log[4].starts_with("TextMessage"));
}

/// AC3: Skill failure produces error TextMessage and SurfaceNotification.
#[tokio::test]
async fn test_ac3_skill_failure_produces_error_text_and_notification() {
    let (mut chain, _tx) = setup_test_chain("failing-cron", "fail-echo");
    let (notif_tx, mut notif_rx) = mpsc::channel(10);
    chain.set_notification_sender(notif_tx);

    let event = CronEventMessage {
        job_name: "failing-cron".into(),
        action: "event".into(),
        schedule: "*/1 * * * *".into(),
        triggered_at: Timestamp {
            seconds: 0,
            nanos: 0,
        },
        payload: HashMap::new(),
        target_stream: "test-stream".into(),
    };

    let outcome = chain.process_cron_event(event).await.unwrap();

    // Chain completed but with failure
    assert!(!outcome.success);

    // Verify error TextMessage
    let last_event = outcome.events.last().unwrap();
    match last_event {
        ChainEvent::TextMessagePublished { content, .. } => {
            assert!(content.contains("CRON CHAIN FAILED"));
            assert!(content.contains("failing-cron"));
            assert!(content.contains("fail-echo"));
            assert!(content.contains("SKILL_RESULT_ERROR"));
        }
        _ => panic!("expected TextMessagePublished as final event"),
    }

    // Verify SurfaceNotification
    let notification = notif_rx.recv().await.unwrap();
    assert_eq!(
        notification.notification_type,
        NotificationType::SkillFailure
    );
    assert!(notification.message.contains("CRON CHAIN FAILED"));
    assert_eq!(
        notification.metadata.get("job_name").unwrap(),
        "failing-cron"
    );
    assert_eq!(
        notification.metadata.get("skill_name").unwrap(),
        "fail-echo"
    );
    assert_eq!(
        notification.metadata.get("error_code").unwrap(),
        "SKILL_RESULT_ERROR"
    );
}

/// AC2 supplement: all required event types are present in chain log.
#[tokio::test]
async fn test_ac2_all_event_types_present_in_chain_log() {
    let (mut chain, _tx) = setup_test_chain("echo-cron", "echo");

    let event = CronEventMessage {
        job_name: "echo-cron".into(),
        action: "event".into(),
        schedule: "*/1 * * * *".into(),
        triggered_at: Timestamp {
            seconds: 0,
            nanos: 0,
        },
        payload: HashMap::new(),
        target_stream: "test-stream".into(),
    };

    let outcome = chain.process_cron_event(event).await.unwrap();

    let type_names: Vec<&str> = outcome.events.iter().map(|e| e.type_name()).collect();
    assert!(type_names.contains(&"CronEvent"));
    assert!(type_names.contains(&"SkillInvocation"));
    assert!(type_names.contains(&"SkillProgress"));
    assert!(type_names.contains(&"SkillResult"));
    assert!(type_names.contains(&"TextMessage"));
}

/// AC1 supplement: invocation has correct fields.
#[tokio::test]
async fn test_ac1_skill_invocation_has_correct_fields() {
    let (mut chain, _tx) = setup_test_chain("echo-cron", "echo");

    let event = CronEventMessage {
        job_name: "echo-cron".into(),
        action: "event".into(),
        schedule: "*/1 * * * *".into(),
        triggered_at: Timestamp {
            seconds: 0,
            nanos: 0,
        },
        payload: [("input".into(), "test-data".into())].into_iter().collect(),
        target_stream: "test-stream".into(),
    };

    let outcome = chain.process_cron_event(event).await.unwrap();

    // Find the SkillInvocationPublished event
    let inv_event = outcome
        .events
        .iter()
        .find(|e| matches!(e, ChainEvent::SkillInvocationPublished { .. }))
        .expect("expected SkillInvocationPublished in chain");

    match inv_event {
        ChainEvent::SkillInvocationPublished {
            invocation_id,
            skill_name,
            timestamp_ms,
        } => {
            assert!(!invocation_id.is_empty(), "invocation_id must not be empty");
            assert_eq!(skill_name, "echo");
            assert!(*timestamp_ms > 0, "timestamp_ms must be positive");
        }
        _ => unreachable!(),
    }
}

/// AC3 supplement: chain without notification sender still succeeds gracefully.
#[tokio::test]
async fn test_ac3_chain_without_notification_sender_graceful_degradation() {
    let (mut chain, _tx) = setup_test_chain("failing-cron", "fail-echo");
    // NO notification_sender set

    let event = CronEventMessage {
        job_name: "failing-cron".into(),
        action: "event".into(),
        schedule: "*/1 * * * *".into(),
        triggered_at: Timestamp {
            seconds: 0,
            nanos: 0,
        },
        payload: HashMap::new(),
        target_stream: "test-stream".into(),
    };

    let outcome = chain.process_cron_event(event).await.unwrap();
    assert!(!outcome.success);
    // Chain still completed with all 5 events despite no notification sender
    assert_eq!(outcome.events.len(), 5);
}
