// Story 5-5: Cron Job Declaration and CronEvent Publishing — Acceptance Tests
// TDD RED phase: these tests MUST fail until implementation is complete.

/// AC1: Given a .wh file declares a cron job, When wh deploy apply runs,
/// Then the cron job is registered and committed to git as cron/jobs.yaml
#[test]
fn ac1_cron_job_parsed_from_wh_file_and_persisted_to_git() {
    // Arrange: a .wh file with cron section
    let wh_content = r#"
apiVersion: wheelhouse.dev/v1
kind: Topology

streams:
  - name: main
    provider: local

cron:
  - name: daily-compaction
    schedule: "0 3 * * *"
    target: main
    action: compact
"#;

    // Act: parse .wh file and persist cron config
    // This will fail until wh_broker::cron module exists
    let configs =
        wh_broker::cron::parse_wh_cron_section(wh_content).expect("should parse cron section");

    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].name, "daily-compaction");
    assert_eq!(configs[0].schedule, "0 3 * * *");
    assert_eq!(configs[0].target, "main");
    assert_eq!(configs[0].action, "compact");

    // Persist to git
    let tmp_dir = tempfile::tempdir().expect("tmpdir");
    wh_broker::cron::save_cron_config(&configs, tmp_dir.path()).expect("should save cron config");

    // Verify cron/jobs.yaml exists
    let jobs_path = tmp_dir.path().join("cron").join("jobs.yaml");
    assert!(jobs_path.exists(), "cron/jobs.yaml should exist after save");

    // Load back and verify round-trip
    let loaded =
        wh_broker::cron::load_cron_config(tmp_dir.path()).expect("should load cron config");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].name, "daily-compaction");
}

/// AC1 validation: invalid cron expression rejected at parse time
#[test]
fn ac1_invalid_cron_expression_rejected() {
    let wh_content = r#"
apiVersion: wheelhouse.dev/v1
kind: Topology

streams:
  - name: main
    provider: local

cron:
  - name: bad-job
    schedule: "not-a-cron-expression"
    target: main
    action: event
"#;

    let result = wh_broker::cron::parse_wh_cron_section(wh_content);
    assert!(
        result.is_err(),
        "invalid cron expression should produce error"
    );
}

/// AC1 validation: cron targeting nonexistent stream rejected
#[test]
fn ac1_cron_targeting_nonexistent_stream_rejected() {
    let wh_content = r#"
apiVersion: wheelhouse.dev/v1
kind: Topology

streams:
  - name: main
    provider: local

cron:
  - name: orphan-job
    schedule: "0 3 * * *"
    target: nonexistent-stream
    action: event
"#;

    let result = wh_broker::cron::parse_wh_cron_section(wh_content);
    assert!(
        result.is_err(),
        "cron targeting nonexistent stream should produce error"
    );
}

/// AC2: Given the cron schedule fires, When the trigger time is reached,
/// Then a CronEvent is published with job_name, triggered_at, and schedule fields
#[tokio::test]
async fn ac2_cron_fires_and_publishes_cronevent_to_stream() {
    use std::time::Duration;
    use tokio::sync::mpsc;

    // Create a channel to capture published CronEvents
    let (tx, mut rx) = mpsc::channel(10);

    let config = wh_broker::cron::CronJobConfig {
        name: "test-job".to_string(),
        schedule: "* * * * * *".to_string(), // every second (if supported)
        target: "main".to_string(),
        action: "event".to_string(),
        payload: None,
    };

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();

    // Start scheduler in background
    let handle = tokio::spawn(async move {
        wh_broker::cron::CronScheduler::new(vec![config], cancel_clone, tx)
            .run()
            .await;
    });

    // Wait for at least one CronEvent
    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("should receive CronEvent within 5s")
        .expect("channel should not close");

    assert_eq!(event.job_name, "test-job");
    assert_eq!(event.action, "event");
    assert!(!event.job_name.is_empty());

    // Shutdown
    cancel.cancel();
    let _ = handle.await;
}

/// AC2: CronEvent contains correct fields including schedule
#[tokio::test]
async fn ac2_cronevent_has_correct_fields() {
    // This test verifies the CronEvent proto message has all required fields
    let event = wh_proto::wheelhouse::v1::CronEvent {
        job_name: "daily-compaction".to_string(),
        action: "compact".to_string(),
        schedule: "0 3 * * *".to_string(),
        triggered_at: Some(prost_types::Timestamp {
            seconds: 1710288000,
            nanos: 0,
        }),
        payload: std::collections::HashMap::new(),
    };

    assert_eq!(event.job_name, "daily-compaction");
    assert_eq!(event.action, "compact");
    assert_eq!(event.schedule, "0 3 * * *");
    assert!(event.triggered_at.is_some());
}

/// AC3: Given a cron job fails to publish, When the failure occurs,
/// Then the failure is logged at error level and the scheduler continues
#[tokio::test]
async fn ac3_publish_failure_logged_and_scheduler_continues() {
    use std::time::Duration;
    use tokio::sync::mpsc;

    // Create a closed channel to simulate publish failure
    let (tx, rx) = mpsc::channel(1);
    drop(rx); // Close receiver — sends will fail

    let config = wh_broker::cron::CronJobConfig {
        name: "failing-job".to_string(),
        schedule: "* * * * * *".to_string(),
        target: "main".to_string(),
        action: "event".to_string(),
        payload: None,
    };

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();

    // Start scheduler — it should NOT crash despite publish failure
    let handle = tokio::spawn(async move {
        wh_broker::cron::CronScheduler::new(vec![config], cancel_clone, tx)
            .run()
            .await;
    });

    // Give scheduler time to attempt a fire and survive
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Scheduler should still be running (not panicked/crashed)
    assert!(
        !handle.is_finished(),
        "scheduler should still be running despite publish failure"
    );

    cancel.cancel();
    let _ = handle.await;
}

/// AC3: Multiple cron jobs run independently — scheduler processes all jobs
#[tokio::test]
async fn ac3_multiple_cron_jobs_run_independently() {
    use std::time::Duration;
    use tokio::sync::mpsc;

    let (tx, mut rx) = mpsc::channel(10);

    // Use every-second schedule (6-field with seconds) — both jobs fire on each tick
    let configs = vec![
        wh_broker::cron::CronJobConfig {
            name: "job-a".to_string(),
            schedule: "* * * * * *".to_string(),
            target: "main".to_string(),
            action: "event".to_string(),
            payload: None,
        },
        wh_broker::cron::CronJobConfig {
            name: "job-b".to_string(),
            schedule: "* * * * * *".to_string(),
            target: "main".to_string(),
            action: "event".to_string(),
            payload: None,
        },
    ];

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();

    let handle = tokio::spawn(async move {
        wh_broker::cron::CronScheduler::new(configs, cancel_clone, tx)
            .run()
            .await;
    });

    // Wait for at least 2 events (one from each job on first fire)
    let mut events = Vec::new();
    for _ in 0..4 {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(event)) => events.push(event),
            _ => break,
        }
        if events.len() >= 2 {
            break;
        }
    }

    // Should have received events from both jobs
    assert!(
        events.len() >= 2,
        "should receive events from both jobs, got {}",
        events.len()
    );
    let names: Vec<&str> = events.iter().map(|e| e.job_name.as_str()).collect();
    assert!(names.contains(&"job-a"), "should have event from job-a");
    assert!(names.contains(&"job-b"), "should have event from job-b");

    cancel.cancel();
    let _ = handle.await;
}

/// Scheduler respects CancellationToken for clean shutdown
#[tokio::test]
async fn scheduler_shuts_down_on_cancellation_token() {
    use std::time::Duration;
    use tokio::sync::mpsc;

    let (tx, _rx) = mpsc::channel(10);

    let config = wh_broker::cron::CronJobConfig {
        name: "shutdown-test".to_string(),
        schedule: "0 3 * * *".to_string(),
        target: "main".to_string(),
        action: "event".to_string(),
        payload: None,
    };

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();

    let handle = tokio::spawn(async move {
        wh_broker::cron::CronScheduler::new(vec![config], cancel_clone, tx)
            .run()
            .await;
    });

    // Cancel immediately
    cancel.cancel();

    // Should complete within 2 seconds
    let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(
        result.is_ok(),
        "scheduler should shut down within 2 seconds after cancellation"
    );
}
