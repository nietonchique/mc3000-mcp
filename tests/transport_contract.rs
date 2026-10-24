//! Fake-backend transport/client contract tests.
//!
//! These tests run entirely against [`FakeTransport`] (no hardware) and pin the
//! Python transport semantics plus the robustness improvements: official name
//! filtering, FFE0/FFE1 subscription, 245-byte curve reassembly, typed timeout
//! errors, the exact APK profile write order with delays, bounded notification
//! routing, disconnect cleanup, serialized requests, and read-only model
//! enforcement.

use std::sync::Arc;
use std::time::Duration;

use mc3000_mcp::client::{ChargerClient, ChargerError, ConnectInfo, SLOT_POLL_GAP};
use mc3000_mcp::model::{ChargerModel, Slot};
use mc3000_mcp::protocol::{
    self, BasicSettings, Notification, Opcode, StatusPayload, command_get_basic,
    command_get_status, command_start, command_stop, with_checksum,
};
use mc3000_mcp::transport::{FakeTransport, ScanDevice, Transport, TransportError, timeout_secs};

const ADDRESS: &str = "AA:BB:CC:DD:EE:02";

/// Fixture slot; panics only on a corrupted fixture value.
fn slot(value: u8) -> Slot {
    match Slot::try_from(value) {
        Ok(parsed) => parsed,
        Err(error) => panic!("fixture slot {value}: {error}"),
    }
}

/// Scan result fixture.
fn device(address: &str, name: &str) -> ScanDevice {
    ScanDevice {
        address: address.to_owned(),
        name: Some(name.to_owned()),
        rssi: Some(-42),
    }
}

/// MC3000 slot-status notification: opcode 0x55 with working status code 1.
fn status_frame(value: u8) -> Vec<u8> {
    let mut frame = command_get_status(slot(value)).to_vec();
    frame[6] = 1;
    with_checksum(&frame)
}

/// MC5000 extended slot-status notification for one slot (Python fixture
/// layout: `[0f, 03, 91, channel, ..., status, mode, error, battery_type]`).
fn mc5000_reply(value: u8) -> Vec<u8> {
    let mut frame = vec![0u8; 40];
    frame[0] = 0x0F;
    frame[1] = 0x03;
    frame[2] = Opcode::Mc5000SlotStatus.as_u8();
    frame[3] = 1 << value;
    frame[18] = 2; // status: charging
    frame[19] = 3; // mode: discharge
    frame[21] = 3; // battery type: NiMH
    frame
}

/// A complete 245-byte voltage-curve notification for one slot: interval 1 s,
/// first sample 1234 mV, empty zero tail.
fn curve_bytes(value: u8) -> Vec<u8> {
    let mut curve = vec![0u8; protocol::CURVE_LEN];
    curve[0] = 0x0F;
    curve[1] = Opcode::VoltageCurve.as_u8();
    curve[2] = value;
    curve[3] = 0;
    curve[4] = 1;
    curve[5] = 0x04;
    curve[6] = 0xD2;
    curve
}

/// A 40-byte profile frame carrying only the slot mask the client reads.
fn profile_bytes(slot_mask: u8) -> Vec<u8> {
    let mut profile = vec![0u8; protocol::PROFILE_LEN];
    profile[0] = 0x0F;
    profile[1] = Opcode::SetProfile.as_u8();
    profile[2] = slot_mask;
    profile
}

/// Set-basic acknowledgement frame (`[0f, 63, 01, ..., checksum]`).
fn set_basic_ack() -> Vec<u8> {
    let mut frame = vec![0u8; protocol::FRAME_LEN];
    frame[0] = 0x0F;
    frame[1] = Opcode::SetBasic.as_u8();
    frame[2] = 1;
    with_checksum(&frame)
}

/// Client connected through a fresh fake; both handles share state.
async fn connected_client(
    model: ChargerModel,
) -> (Arc<ChargerClient<FakeTransport>>, FakeTransport) {
    let fake = FakeTransport::new();
    let client = Arc::new(ChargerClient::new(fake.clone()));
    client
        .connect(ADDRESS, model)
        .await
        .unwrap_or_else(|error| panic!("connect: {error}"));
    (client, fake)
}

/// Unwrap an expectation, panicking with context (the crate denies `expect`).
fn expect_ok<T>(result: Result<T, ChargerError>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

/// Tokio-clock distance between two recorded instants.
fn gap(start: tokio::time::Instant, end: tokio::time::Instant) -> Duration {
    end.checked_duration_since(start).unwrap_or_default()
}

/// Assert a recorded gap lies within `[min, max]`.
fn assert_gap(actual: Duration, min: Duration, max: Duration, what: &str) {
    assert!(
        actual >= min && actual <= max,
        "{what}: gap {actual:?} outside [{min:?}, {max:?}]"
    );
}

#[test]
fn gatt_uuids_match_protocol_constants() {
    let service = match uuid::Uuid::parse_str(protocol::SERVICE_UUID) {
        Ok(parsed) => parsed,
        Err(error) => panic!("protocol service uuid literal: {error}"),
    };
    let characteristic = match uuid::Uuid::parse_str(protocol::CHAR_UUID) {
        Ok(parsed) => parsed,
        Err(error) => panic!("protocol characteristic uuid literal: {error}"),
    };
    assert_eq!(mc3000_mcp::transport::SERVICE_UUID, service);
    assert_eq!(mc3000_mcp::transport::CHARACTERISTIC_UUID, characteristic);
}

#[test]
fn timeout_validation_is_typed() {
    assert!(matches!(
        timeout_secs(-1.0),
        Err(TransportError::InvalidTimeout { value }) if value < 0.0
    ));
    assert!(matches!(
        timeout_secs(f64::NAN),
        Err(TransportError::InvalidTimeout { .. })
    ));
    assert!(matches!(
        timeout_secs(f64::INFINITY),
        Err(TransportError::InvalidTimeout { .. })
    ));
    assert_eq!(
        timeout_secs(0.5).unwrap_or_default(),
        Duration::from_millis(500)
    );
}

#[tokio::test]
async fn scan_filters_official_names() {
    let fake = FakeTransport::with_scan_devices(vec![
        device("AA:BB:CC:DD:EE:01", "Keyboard"),
        device("AA:BB:CC:DD:EE:02", "Charger"),
        device("AA:BB:CC:DD:EE:03", "SimpleBLEPeripheral-2"),
        device("AA:BB:CC:DD:EE:04", "My Scanner"),
    ]);
    let client = ChargerClient::new(fake);
    let found = expect_ok(client.scan(Duration::from_millis(10)).await, "scan");
    assert_eq!(found.len(), 2, "only official/substring names match");
    assert_eq!(found[0].address, "AA:BB:CC:DD:EE:02");
    assert_eq!(found[0].name.as_deref(), Some("Charger"));
    assert_eq!(found[1].address, "AA:BB:CC:DD:EE:03");
}

#[tokio::test]
async fn scan_failure_and_find_and_connect_errors() {
    let fake = FakeTransport::new();
    let client = ChargerClient::new(fake.clone());
    fake.set_scan_failure(Some(TransportError::Backend("rfkill blocked".to_owned())));
    match client.scan(Duration::from_millis(10)).await {
        Err(ChargerError::Transport(TransportError::Backend(message))) => {
            assert!(message.contains("rfkill"), "message: {message}");
        }
        other => panic!("expected backend scan failure, got {other:?}"),
    }
    // Empty scan: find_and_connect reports the typed no-device error.
    fake.set_scan_failure(None);
    match client
        .find_and_connect(None, Duration::from_millis(5), ChargerModel::Mc3000)
        .await
    {
        Err(ChargerError::Transport(TransportError::NoDeviceFound { names })) => {
            assert!(names.contains(&"Charger".to_owned()));
        }
        other => panic!("expected no-device failure, got {other:?}"),
    }
    // With a device present, find_and_connect connects to the first match.
    fake.set_scan_devices(vec![device("AA:BB:CC:DD:EE:02", "Charger")]);
    let info = expect_ok(
        client
            .find_and_connect(None, Duration::from_millis(5), ChargerModel::Mc3000)
            .await,
        "find_and_connect",
    );
    assert_eq!(
        info,
        ConnectInfo {
            connected: true,
            address: ADDRESS.to_owned(),
            model: ChargerModel::Mc3000,
        }
    );
}

#[tokio::test]
async fn connect_subscribes_and_tracks_model() {
    let fake = FakeTransport::new();
    let client = ChargerClient::new(fake.clone());
    assert!(!client.is_connected().await);
    let info = expect_ok(
        client.connect(ADDRESS, ChargerModel::Mc3000).await,
        "connect",
    );
    assert!(info.connected);
    assert!(fake.subscribed(), "FFE1 notifications must be subscribed");
    assert!(client.is_connected().await);
    assert_eq!(client.model(), Some(ChargerModel::Mc3000));
    assert_eq!(client.address().as_deref(), Some(ADDRESS));
    expect_ok(client.disconnect().await, "disconnect");
    assert!(!client.is_connected().await);
    assert_eq!(client.model(), None, "disconnect clears the model");
    assert_eq!(client.address(), None);
    assert!(
        client.transport().next_notification().await.is_none(),
        "no notification may survive a disconnect"
    );
}

#[tokio::test]
async fn request_requires_connection() {
    let fake = FakeTransport::new();
    let client = ChargerClient::new(fake);
    expect_ok(
        client.connect(ADDRESS, ChargerModel::Mc3000).await,
        "connect",
    );
    expect_ok(client.disconnect().await, "disconnect");
    match client
        .request(
            &command_get_basic(),
            Some(Opcode::GetBasic),
            Duration::from_millis(50),
        )
        .await
    {
        Err(ChargerError::Transport(TransportError::NotConnected)) => {}
        other => panic!("expected not-connected failure, got {other:?}"),
    }
}

#[tokio::test]
async fn wait_for_times_out_and_discards_mismatches() {
    let (client, fake) = connected_client(ChargerModel::Mc3000).await;
    // A basic reply does not satisfy a status wait: it is discarded.
    fake.inject(&command_get_basic());
    let error = match client.status(slot(0)).await {
        Ok(parsed) => panic!("expected timeout, got {parsed:?}"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        ChargerError::Transport(TransportError::ReplyTimeout(_))
    ));
    assert!(error.to_string().contains("timeout waiting"));
    // Mismatched frames are skipped; the next matching reply resolves.
    fake.inject(&[0x0F, 0x99]);
    fake.inject(&status_frame(1));
    let parsed = expect_ok(client.status(slot(1)).await, "status after mismatches");
    assert!(matches!(parsed, Notification::Status { .. }));
    // An unconstrained wait accepts anything, including unknown frames.
    fake.inject(&[0x0F, 0x99]);
    let parsed = expect_ok(
        client.wait_for(None, Duration::from_millis(50)).await,
        "wait_for any",
    );
    assert!(matches!(parsed, Notification::Unknown { .. }));
}

#[tokio::test]
async fn curve_fragments_reassemble_to_245_bytes() {
    let (client, fake) = connected_client(ChargerModel::Mc3000).await;
    let curve = curve_bytes(3);
    fake.inject(&curve[0..20]);
    fake.inject(&curve[20..]);
    let parsed = expect_ok(
        client.curve(slot(3), Duration::from_millis(200)).await,
        "curve",
    );
    match parsed {
        Notification::VoltageCurve {
            slot,
            interval_ms,
            points_mv,
            point_count,
            duration_seconds,
            time_series,
            ..
        } => {
            assert_eq!(slot, 3);
            assert_eq!(interval_ms, 1000);
            assert_eq!(points_mv.first(), Some(&1234));
            assert_eq!(points_mv.len(), 120);
            assert_eq!(point_count, 1, "zero tail is not counted");
            assert!(
                duration_seconds.abs() < f64::EPSILON,
                "single sample at t=0 has zero duration: {duration_seconds}"
            );
            assert_eq!(time_series.len(), 1);
            assert_eq!(time_series[0].voltage_mv, 1234);
        }
        other => panic!("expected voltage curve, got {other:?}"),
    }
}

#[tokio::test]
async fn full_curve_notification_parses_directly() {
    let (client, fake) = connected_client(ChargerModel::Mc3000).await;
    fake.inject(&curve_bytes(1));
    let parsed = expect_ok(
        client.curve(slot(1), Duration::from_millis(200)).await,
        "curve",
    );
    match parsed {
        Notification::VoltageCurve {
            slot, points_mv, ..
        } => {
            assert_eq!(slot, 1);
            assert_eq!(points_mv.first(), Some(&1234));
        }
        other => panic!("expected voltage curve, got {other:?}"),
    }
}

#[tokio::test]
async fn bounded_queue_drops_oldest() {
    let (client, fake) = connected_client(ChargerModel::Mc3000).await;
    for value in 0..200u8 {
        fake.inject(&[0x0F, 0x99, value]);
    }
    assert_eq!(client.dropped_notifications(), 72, "200 - capacity 128");
    let parsed = expect_ok(
        client.wait_for(None, Duration::from_millis(50)).await,
        "wait_for",
    );
    match parsed {
        Notification::Unknown {
            opcode: Some(0x99),
            raw_hex,
        } => {
            assert_eq!(raw_hex, "0F9948", "the oldest 72 entries were dropped");
        }
        other => panic!("expected unknown notification, got {other:?}"),
    }
}

#[tokio::test]
async fn poll_all_status_is_sequential_with_gaps() {
    let (client, fake) = connected_client(ChargerModel::Mc3000).await;
    for value in 0..=3u8 {
        fake.inject(&status_frame(value));
    }
    let results = expect_ok(client.poll_all_status().await, "poll_all_status");
    assert_eq!(results.len(), 4);
    for (parsed, value) in results.iter().zip(0..=3u8) {
        match parsed {
            Notification::Status {
                status: StatusPayload::Mc3000(status),
            } => assert_eq!(status.slot, value),
            other => panic!("expected MC3000 status, got {other:?}"),
        }
    }
    let writes = fake.writes();
    assert_eq!(writes.len(), 4);
    for (write, value) in writes.iter().zip(0..=3u8) {
        assert_eq!(write, &command_get_status(slot(value)).to_vec());
    }
    let instants = fake.write_instants();
    for pair in instants.windows(2) {
        assert_gap(
            gap(pair[0], pair[1]),
            SLOT_POLL_GAP.saturating_sub(Duration::from_millis(5)),
            SLOT_POLL_GAP + Duration::from_millis(50),
            "sequential slot polls keep the 50 ms gap",
        );
    }
}

#[tokio::test(start_paused = true)]
async fn send_profile_write_order_and_delays() {
    let (client, fake) = connected_client(ChargerModel::Mc3000).await;
    let profile = profile_bytes(0b0011);
    let report = expect_ok(client.send_profile(&profile, true).await, "send_profile");
    assert!(report.ok);
    assert_eq!(report.stopped_slots, vec![0, 1]);
    assert!(report.started);
    assert_eq!(report.profile_hex, hex::encode_upper(&profile));
    let writes = fake.writes();
    assert_eq!(writes.len(), 6);
    assert_eq!(writes[0], command_stop(slot(0)).to_vec());
    assert_eq!(writes[1], command_stop(slot(1)).to_vec());
    assert_eq!(writes[2], profile[..20].to_vec());
    assert_eq!(writes[3], profile[20..].to_vec());
    assert_eq!(writes[4], command_start(slot(0)).to_vec());
    assert_eq!(writes[5], command_start(slot(1)).to_vec());
    let instants = fake.write_instants();
    assert_gap(
        gap(instants[0], instants[1]),
        Duration::from_millis(490),
        Duration::from_millis(600),
        "stop 0 -> stop 1",
    );
    assert_gap(
        gap(instants[1], instants[2]),
        Duration::from_millis(490),
        Duration::from_millis(600),
        "stop 1 -> chunk 1",
    );
    assert_gap(
        gap(instants[2], instants[3]),
        Duration::from_millis(40),
        Duration::from_millis(150),
        "chunk 1 -> chunk 2",
    );
    assert_gap(
        gap(instants[3], instants[4]),
        Duration::from_secs(5),
        Duration::from_secs(6),
        "chunk 2 -> profile settle -> start 0",
    );
    assert_gap(
        gap(instants[4], instants[5]),
        Duration::ZERO,
        Duration::from_millis(20),
        "start 0 -> start 1",
    );
}

#[tokio::test(start_paused = true)]
async fn send_profile_without_start() {
    let (client, fake) = connected_client(ChargerModel::Mc3000).await;
    let profile = profile_bytes(0b0100);
    let report = expect_ok(client.send_profile(&profile, false).await, "send_profile");
    assert_eq!(report.stopped_slots, vec![2]);
    assert!(!report.started);
    let writes = fake.writes();
    assert_eq!(writes.len(), 3, "one stop plus the two profile chunks");
    assert_eq!(writes[0], command_stop(slot(2)).to_vec());
    assert_eq!(writes[1], profile[..20].to_vec());
    assert_eq!(writes[2], profile[20..].to_vec());
    let instants = fake.write_instants();
    assert_gap(
        gap(instants[1], instants[2]),
        Duration::from_millis(40),
        Duration::from_millis(150),
        "chunk gap",
    );
}

#[tokio::test]
async fn stop_notify_failure_cleanup_and_reconnect() {
    let (client, fake) = connected_client(ChargerModel::Mc3000).await;
    fake.inject(&status_frame(0)); // pending notification before teardown
    fake.set_stop_notify_failure(true);
    expect_ok(
        client.disconnect().await,
        "disconnect with failing unsubscribe",
    );
    assert!(!client.is_connected().await);
    assert_eq!(client.model(), None);
    assert!(
        client.transport().next_notification().await.is_none(),
        "pending notifications must be cleared even when unsubscribe fails"
    );
    // The failed unsubscribe must not poison the next link.
    fake.set_stop_notify_failure(false);
    expect_ok(
        client.connect(ADDRESS, ChargerModel::Mc3000).await,
        "reconnect",
    );
    fake.inject(&status_frame(2));
    let parsed = expect_ok(client.status(slot(2)).await, "status after reconnect");
    assert!(matches!(parsed, Notification::Status { .. }));
}

#[tokio::test]
async fn disconnect_clears_curve_reassembly() {
    let (client, fake) = connected_client(ChargerModel::Mc3000).await;
    let curve = curve_bytes(3);
    fake.inject(&curve[0..20]); // partial curve survives only until disconnect
    expect_ok(client.disconnect().await, "disconnect");
    expect_ok(
        client.connect(ADDRESS, ChargerModel::Mc3000).await,
        "connect",
    );
    fake.inject(&curve[20..]);
    match client.curve(slot(3), Duration::from_millis(100)).await {
        Err(ChargerError::Transport(TransportError::ReplyTimeout(_))) => {}
        other => panic!("expected timeout with a cleared curve buffer, got {other:?}"),
    }
    // A complete curve after the disconnect still parses.
    fake.inject(&curve);
    let parsed = expect_ok(
        client.curve(slot(3), Duration::from_millis(100)).await,
        "curve again",
    );
    assert!(matches!(parsed, Notification::VoltageCurve { .. }));
}

#[tokio::test]
async fn unknown_model_writes_rejected_reads_allowed() {
    let fake = FakeTransport::new();
    let client = ChargerClient::new(fake.clone());
    let profile = profile_bytes(0b0001);
    // No model selected: every mutating path is refused before any byte moves.
    assert!(matches!(
        client.write(&[0x0F, 0x05, 0x01]).await,
        Err(ChargerError::ModelUnknown)
    ));
    assert!(matches!(
        client.start(slot(0)).await,
        Err(ChargerError::ModelUnknown)
    ));
    assert!(matches!(
        client.stop(slot(0)).await,
        Err(ChargerError::ModelUnknown)
    ));
    assert!(matches!(
        client.send_profile(&profile, false).await,
        Err(ChargerError::ModelUnknown)
    ));
    assert!(matches!(
        client.set_basic(&BasicSettings::default()).await,
        Err(ChargerError::ModelUnknown)
    ));
    assert!(matches!(
        client
            .request(&command_start(slot(0)), None, Duration::from_millis(50))
            .await,
        Err(ChargerError::ModelUnknown)
    ));
    assert_eq!(fake.writes().len(), 0, "unknown model must not write");
    // Connected MC5000: writes stay refused, reads work.
    expect_ok(
        client.connect(ADDRESS, ChargerModel::Mc5000).await,
        "connect mc5000",
    );
    assert!(matches!(
        client.start(slot(0)).await,
        Err(ChargerError::ModelReadOnly {
            model: ChargerModel::Mc5000
        })
    ));
    assert!(matches!(
        client.write(&[0x0F, 0x05, 0x01]).await,
        Err(ChargerError::ModelReadOnly { .. })
    ));
    assert!(matches!(
        client.send_profile(&profile, false).await,
        Err(ChargerError::ModelReadOnly { .. })
    ));
    assert!(matches!(
        client
            .request(&command_stop(slot(1)), None, Duration::from_millis(50))
            .await,
        Err(ChargerError::ModelReadOnly { .. })
    ));
    assert_eq!(fake.writes().len(), 0, "read-only model must not write");
    fake.inject(&mc5000_reply(3));
    let parsed = expect_ok(client.mc5000_status(slot(3)).await, "mc5000_status");
    assert!(matches!(
        &parsed,
        Notification::Status { status: StatusPayload::Mc5000(status) } if status.slot == Some(3)
    ));
    fake.inject(&command_get_basic());
    expect_ok(client.basic().await, "basic read on mc5000");
}

#[tokio::test]
async fn set_basic_write_reaches_device_and_ack_matches() {
    let (client, fake) = connected_client(ChargerModel::Mc3000).await;
    fake.inject(&set_basic_ack());
    let parsed = expect_ok(
        client.set_basic(&BasicSettings::default()).await,
        "set_basic",
    );
    match parsed {
        Notification::Ack { opcode, ok, .. } => {
            assert_eq!(opcode, Opcode::SetBasic.as_u8());
            assert!(ok);
        }
        other => panic!("expected ack, got {other:?}"),
    }
    let writes = fake.writes();
    assert_eq!(writes.len(), 1);
}

#[tokio::test]
async fn remote_link_loss_drains_then_reconnect_delivers() {
    let (client, fake) = connected_client(ChargerModel::Mc3000).await;
    fake.inject(&status_frame(0));
    fake.inject(&status_frame(1));
    fake.drop_link();
    // Notifications buffered before a remote drop stay deliverable, then the
    // stream ends (this is the close-without-clear transition).
    assert_eq!(
        client.transport().next_notification().await.as_deref(),
        Some(status_frame(0).as_slice())
    );
    assert_eq!(
        client.transport().next_notification().await.as_deref(),
        Some(status_frame(1).as_slice())
    );
    assert_eq!(client.transport().next_notification().await, None);
    // A fresh connect must restore delivery (fake reopens internally; the
    // BLE-side reopen is pinned by the queue unit test in transport/mod.rs).
    expect_ok(
        client.connect(ADDRESS, ChargerModel::Mc3000).await,
        "reconnect",
    );
    fake.inject(&status_frame(2));
    let parsed = expect_ok(client.status(slot(2)).await, "status after reconnect");
    assert!(matches!(parsed, Notification::Status { .. }));
}

#[tokio::test(start_paused = true)]
async fn concurrent_requests_are_serialized() {
    let (client, fake) = connected_client(ChargerModel::Mc3000).await;
    // Request A takes the request lock and waits for a reply that never comes.
    let handle_a = {
        let client = Arc::clone(&client);
        tokio::spawn(async move { client.status(slot(0)).await })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        fake.writes().len(),
        1,
        "A must have written its status request"
    );
    // B must not interleave its write while A holds the lock.
    let handle_b = {
        let client = Arc::clone(&client);
        tokio::spawn(async move { client.mc5000_status(slot(3)).await })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        fake.writes().len(),
        1,
        "B must wait for the lock instead of stealing A's reply window"
    );
    // A times out and releases the lock; only then is B's reply injected, so
    // B (not A) consumes it.
    match handle_a.await {
        Ok(result) => match result {
            Err(ChargerError::Transport(TransportError::ReplyTimeout(_))) => {}
            other => panic!("expected A to time out, got {other:?}"),
        },
        Err(error) => panic!("join A: {error}"),
    }
    fake.inject(&mc5000_reply(3));
    let parsed_b = match handle_b.await {
        Ok(result) => result,
        Err(error) => panic!("join B: {error}"),
    };
    assert!(matches!(
        parsed_b,
        Ok(Notification::Status {
            status: StatusPayload::Mc5000(_)
        })
    ));
    let instants = fake.write_instants();
    assert_eq!(instants.len(), 2);
    assert!(
        gap(instants[0], instants[1]) >= Duration::from_millis(400),
        "B's write must happen after A's timeout, not before"
    );
}
