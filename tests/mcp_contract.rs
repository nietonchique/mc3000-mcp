//! rmcp client integration contract tests against the public MCP server
//! surface (`tests/test_server.py` coverage, ported to the Rust rewrite).
//!
//! Every test drives a real rmcp client over an in-memory duplex transport
//! into [`McpService`] backed by [`FakeTransport`] — no hardware, no mocks of
//! the server layer. Covered: initialize, tools/list, every tool family,
//! resources, prompts, dry-run, missing/wrong/correct confirmation tokens,
//! tokenless emergency stops, redaction and the bounded session log, typed
//! malformed-input errors, and fake-backend write behavior.

use std::sync::Arc;

use mc3000_mcp::mcp::McpService;
use mc3000_mcp::model::{Slot, SlotMask};
use mc3000_mcp::profile::{ProfileParams, build_profile};
use mc3000_mcp::protocol::{self, Opcode, command_get_status, with_checksum};
use mc3000_mcp::session::{REDACTED_ADDRESS, REDACTED_TOKEN, SessionLog};
use mc3000_mcp::transport::{FakeTransport, ScanDevice};
use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, GetPromptRequestParams,
    ReadResourceRequestParams,
};
use rmcp::service::{RoleClient, RunningService, ServiceError};
use serde_json::{Value, json};

/// Fake charger address used across tests.
const ADDRESS: &str = "AA:BB:CC:DD:EE:02";

/// Every Python tool name, in definition order.
const ALL_TOOLS: [&str; 27] = [
    "mc3000_scan",
    "mc3000_connect",
    "mc3000_disconnect",
    "mc3000_status",
    "mc3000_start",
    "mc3000_stop",
    "mc3000_get_version",
    "mc3000_get_basic",
    "mc3000_set_basic",
    "mc3000_get_voltage_curve",
    "mc3000_mc5000_status",
    "mc3000_build_profile",
    "mc3000_apply_profile",
    "mc3000_send_raw",
    "charger.scan_devices",
    "charger.connect",
    "charger.get_status",
    "charger.read_slots",
    "charger.get_voltage_curve",
    "charger.export_voltage_curve",
    "charger.list_profiles",
    "charger.validate_profile",
    "charger.apply_profile",
    "charger.start",
    "charger.stop_slot",
    "charger.stop_all",
    "charger.export_session_log",
];

/// Fixture slot; panics only on a corrupted fixture value.
fn slot(value: u8) -> Slot {
    match Slot::try_from(value) {
        Ok(parsed) => parsed,
        Err(error) => panic!("fixture slot {value}: {error}"),
    }
}

/// Scan result fixture.
fn device(address: &str) -> ScanDevice {
    ScanDevice {
        address: address.to_owned(),
        name: Some("Charger".to_owned()),
        rssi: Some(-42),
    }
}

/// MC3000 slot-status notification with working status code 1.
fn status_frame(value: u8) -> Vec<u8> {
    let mut frame = command_get_status(slot(value)).to_vec();
    frame[6] = 1;
    with_checksum(&frame)
}

/// MC5000 extended slot-status notification (Python fixture layout).
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

/// Firmware 1.23 / hardware 2.20 version notification.
fn version_frame() -> Vec<u8> {
    let mut frame = vec![0u8; protocol::FRAME_LEN];
    frame[0] = 0x0F;
    frame[1] = Opcode::Version.as_u8();
    frame[14] = 1;
    frame[15] = 23;
    frame[16] = 22;
    with_checksum(&frame)
}

/// Basic-settings notification with the key beep enabled.
fn basic_frame() -> Vec<u8> {
    let mut frame = vec![0u8; protocol::FRAME_LEN];
    frame[0] = 0x0F;
    frame[1] = Opcode::GetBasic.as_u8();
    frame[3] = 1;
    frame[7] = 0x2A;
    frame[8] = 0xF8;
    with_checksum(&frame)
}

/// Set-basic acknowledgement.
fn set_basic_ack() -> Vec<u8> {
    let mut frame = vec![0u8; protocol::FRAME_LEN];
    frame[0] = 0x0F;
    frame[1] = Opcode::SetBasic.as_u8();
    frame[2] = 1;
    with_checksum(&frame)
}

/// Start/stop acknowledgement for one opcode.
fn ack_frame(opcode: Opcode) -> Vec<u8> {
    let mut frame = vec![0u8; protocol::FRAME_LEN];
    frame[0] = 0x0F;
    frame[1] = opcode.as_u8();
    frame[2] = 1;
    with_checksum(&frame)
}

/// Complete 245-byte voltage-curve notification: slot `value`, 1 s interval,
/// samples 4100/4110/4120 mV, empty zero tail.
fn curve_bytes(value: u8) -> Vec<u8> {
    let mut curve = vec![0u8; protocol::CURVE_LEN];
    curve[0] = 0x0F;
    curve[1] = Opcode::VoltageCurve.as_u8();
    curve[2] = value;
    curve[3] = 0;
    curve[4] = 5;
    for (index, &mv) in [4100_u16, 4110, 4120].iter().enumerate() {
        let position = 5 + index * 2;
        curve[position] = u8::try_from(mv / 256).unwrap_or_default();
        curve[position + 1] = u8::try_from(mv % 256).unwrap_or_default();
    }
    curve
}

/// A 40-byte profile frame carrying only the slot mask the client reads.
fn raw_profile_bytes(slot_mask: u8) -> Vec<u8> {
    let mut profile = vec![0u8; protocol::PROFILE_LEN];
    profile[0] = 0x0F;
    profile[1] = Opcode::SetProfile.as_u8();
    profile[2] = slot_mask;
    profile
}

/// Default-profile frame for one slot (what a fixture client would apply).
fn fixture_profile(slot_index: u8) -> Vec<u8> {
    let params = ProfileParams {
        slot_mask: match SlotMask::try_from(1_u8 << slot_index) {
            Ok(mask) => mask,
            Err(error) => panic!("fixture slot mask: {error}"),
        },
        ..ProfileParams::default()
    };
    match build_profile(&params) {
        Ok(frame) => frame.to_vec(),
        Err(error) => panic!("fixture profile build: {error}"),
    }
}

/// Start an MCP server over a duplex transport and return the rmcp client.
async fn harness(fake: &FakeTransport) -> RunningService<RoleClient, ()> {
    harness_with_log(fake, Arc::new(SessionLog::default())).await
}

/// Harness variant with a custom (e.g. capacity-bounded) session log.
async fn harness_with_log(
    fake: &FakeTransport,
    log: Arc<SessionLog>,
) -> RunningService<RoleClient, ()> {
    let (server_side, client_side) = tokio::io::duplex(64 * 1024);
    let service = McpService::with_session(Arc::new(fake.clone()), log);
    tokio::spawn(async move {
        // The server loop ends when the client (and its transport) drops.
        if let Ok(running) = service.serve(server_side).await {
            let _ = running.waiting().await;
        }
    });
    match ().serve(client_side).await {
        Ok(client) => client,
        Err(error) => panic!("client initialization failed: {error}"),
    }
}

/// Call a tool; `Value::Null` sends no arguments.
async fn call(
    client: &RunningService<RoleClient, ()>,
    name: &str,
    arguments: Value,
) -> Result<CallToolResult, ServiceError> {
    let params = match arguments {
        Value::Null => CallToolRequestParams::new(name.to_owned()),
        Value::Object(map) => CallToolRequestParams::new(name.to_owned()).with_arguments(map),
        other => panic!("arguments must be an object: {other}"),
    };
    client.call_tool(params).await
}

/// Call a tool expecting success; returns the parsed JSON payload.
async fn call_ok(client: &RunningService<RoleClient, ()>, name: &str, arguments: Value) -> Value {
    match call(client, name, arguments).await {
        Ok(result) => payload(&result),
        Err(error) => panic!("{name} failed: {error}"),
    }
}

/// Parse the single pretty-JSON text content of a successful tool result.
fn payload(result: &CallToolResult) -> Value {
    assert_ne!(
        result.is_error,
        Some(true),
        "unexpected tool error result: {:?}",
        result.content
    );
    result_text(result, "successful result");
    let text = match result.content.first() {
        Some(ContentBlock::Text(text)) => text.text.clone(),
        other => panic!("expected text content, got {other:?}"),
    };
    match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => panic!("result text is not JSON ({error}): {text}"),
    }
}

/// The raw text of the first content block.
fn result_text<'a>(result: &'a CallToolResult, what: &'a str) -> &'a str {
    match result.content.first() {
        Some(ContentBlock::Text(text)) => &text.text,
        other => panic!("{what}: expected text content, got {other:?}"),
    }
}

/// Extract the message of a JSON-RPC-level tool error.
fn mcp_error_message(error: ServiceError) -> String {
    match error {
        ServiceError::McpError(data) => data.message.to_string(),
        other => panic!("expected an MCP error, got {other:?}"),
    }
}

/// Assert the tool returned an error-level result carrying `needle`.
fn assert_tool_error(result: &CallToolResult, needle: &str) {
    assert_eq!(result.is_error, Some(true), "expected an error result");
    let text = result_text(result, "error result");
    assert!(
        text.contains(needle),
        "error text {text:?} does not contain {needle:?}"
    );
}

/// Await one tool call and expect a JSON-RPC-level error containing `needle`.
async fn call_err(
    client: &RunningService<RoleClient, ()>,
    name: &str,
    arguments: Value,
    needle: &str,
) {
    match call(client, name, arguments).await {
        Ok(result) => {
            if result.is_error == Some(true) {
                assert_tool_error(&result, needle);
            } else {
                panic!("{name} unexpectedly succeeded: {:?}", payload(&result));
            }
        }
        Err(error) => {
            let message = mcp_error_message(error);
            assert!(
                message.contains(needle),
                "error message {message:?} does not contain {needle:?}"
            );
        }
    }
}

/// Read one resource and return its text payload.
async fn read_resource_text(client: &RunningService<RoleClient, ()>, uri: &str) -> String {
    let result = match client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
    {
        Ok(result) => result,
        Err(error) => panic!("resource {uri} read failed: {error}"),
    };
    match result.contents.first() {
        Some(rmcp::model::ResourceContents::TextResourceContents { text, .. }) => text.clone(),
        other => panic!("expected text resource contents, got {other:?}"),
    }
}

#[tokio::test]
async fn initialize_reports_name_version_and_capabilities() {
    let fake = FakeTransport::new();
    let client = harness(&fake).await;
    let Some(info) = client.peer_info() else {
        panic!("initialize handshake did not store server info");
    };
    let Some(server_info) = info.server_info.as_ref() else {
        panic!("initialize response carries no serverInfo");
    };
    assert_eq!(server_info.name, "mc3000-mcp");
    assert_eq!(server_info.version, env!("CARGO_PKG_VERSION"));
    let capabilities = &info.capabilities;
    assert!(capabilities.tools.is_some(), "tools capability missing");
    assert!(
        capabilities.resources.is_some(),
        "resources capability missing"
    );
    assert!(capabilities.prompts.is_some(), "prompts capability missing");
}

#[tokio::test]
async fn tools_list_contains_exactly_the_python_tools() {
    let fake = FakeTransport::new();
    let client = harness(&fake).await;
    let listed = match client.list_tools(None).await {
        Ok(result) => result,
        Err(error) => panic!("tools/list failed: {error}"),
    };
    let mut names: Vec<&str> = listed.tools.iter().map(|tool| tool.name.as_ref()).collect();
    names.sort_unstable();
    let mut expected: Vec<&str> = ALL_TOOLS.to_vec();
    expected.sort_unstable();
    assert_eq!(names, expected, "tools/list must match the Python surface");
}

#[tokio::test]
async fn tool_schemas_carry_python_descriptions_and_defaults() {
    let fake = FakeTransport::new();
    let client = harness(&fake).await;
    let listed = match client.list_tools(None).await {
        Ok(result) => result,
        Err(error) => panic!("tools/list failed: {error}"),
    };
    let by_name = |name: &str| -> rmcp::model::Tool {
        match listed.tools.iter().find(|tool| tool.name == name) {
            Some(tool) => tool.clone(),
            None => panic!("missing tool {name}"),
        }
    };

    let scan = by_name("mc3000_scan");
    assert_eq!(
        scan.description.as_deref(),
        Some(
            "Scan BLE devices and return devices whose advertised name matches the official MC3000 app filters."
        ),
    );
    let timeout = &scan.input_schema["properties"]["timeout"];
    assert_eq!(timeout["default"], json!(5.0));

    let start = by_name("mc3000_start");
    assert!(
        start.input_schema["required"]
            .as_array()
            .is_some_and(|required| required.contains(&json!("slot"))),
        "mc3000_start must require slot"
    );
    let slot_schema = &start.input_schema["properties"]["slot"];
    assert_eq!(slot_schema["type"], json!("integer"));

    let charger_start = by_name("charger.start");
    assert!(
        charger_start.input_schema["required"]
            .as_array()
            .is_some_and(|required| required.contains(&json!("confirmation_token"))),
        "charger.start must require confirmation_token"
    );

    let build = by_name("mc3000_build_profile");
    let properties = &build.input_schema["properties"];
    assert_eq!(properties["slot_mask"]["minimum"], json!(1));
    assert_eq!(properties["slot_mask"]["maximum"], json!(15));
    assert_eq!(properties["capacity_mah"]["default"], json!(2000));
    assert_eq!(properties["charge_current_ma"]["default"], json!(1000));
    assert_eq!(properties["temp_cutoff"]["default"], json!(45));
    assert_eq!(properties["breakin"]["default"], json!(false));
    assert!(
        build.input_schema["required"]
            .as_array()
            .is_some_and(|required| required.contains(&json!("slot_mask"))),
        "slot_mask must be required"
    );

    let apply = by_name("charger.apply_profile");
    assert_eq!(
        apply.input_schema["properties"]["dry_run"]["default"],
        json!(true)
    );
    let charger_curve = by_name("charger.get_voltage_curve");
    assert_eq!(
        charger_curve.input_schema["properties"]["timeout"]["default"],
        json!(8.0)
    );
    let send_raw = by_name("mc3000_send_raw");
    assert_eq!(
        send_raw.input_schema["properties"]["timeout"]["default"],
        json!(3.0)
    );
}

#[tokio::test]
async fn scan_returns_fake_devices_and_matched_names() {
    let fake = FakeTransport::with_scan_devices(vec![
        device(ADDRESS),
        ScanDevice {
            address: "AA:BB:CC:DD:EE:77".to_owned(),
            name: Some("Keyboard".to_owned()),
            rssi: None,
        },
    ]);
    let client = harness(&fake).await;
    let payload = call_ok(&client, "mc3000_scan", json!({"timeout": 0.1})).await;
    let devices = payload["devices"].as_array().cloned().unwrap_or_default();
    assert_eq!(devices.len(), 1, "non-charger names are filtered");
    assert_eq!(devices[0]["address"], ADDRESS);
    assert_eq!(devices[0]["name"], "Charger");
    assert_eq!(
        payload["matched_names"],
        json!(["SimpleBLEPeripheral", "Charger", "HitecCharger"])
    );

    // `charger.scan_devices` is the same scan under the production name.
    let payload = call_ok(&client, "charger.scan_devices", json!({"timeout": 0.1})).await;
    assert_eq!(payload["devices"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn connect_finds_device_and_defaults_to_mc3000() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    let payload = call_ok(&client, "mc3000_connect", json!({"timeout": 0.5})).await;
    assert_eq!(payload["connected"], json!(true));
    assert_eq!(payload["address"], ADDRESS);
    assert_eq!(payload["model"], "mc3000");
    assert_eq!(fake.current_address().as_deref(), Some(ADDRESS));
    assert!(fake.subscribed(), "FFE1 notifications must be subscribed");

    // Explicit address connects too, and `charger.connect` shares the path.
    let payload = call_ok(
        &client,
        "charger.connect",
        json!({"address": ADDRESS, "timeout": 0.5}),
    )
    .await;
    assert_eq!(payload["address"], ADDRESS);

    // Disconnect reports the Python shape.
    let payload = call_ok(&client, "mc3000_disconnect", Value::Null).await;
    assert_eq!(payload, json!({"connected": false}));
    assert_eq!(fake.current_address(), None);
}

#[tokio::test]
async fn status_family_reads_fake_slots() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    match call(&client, "mc3000_connect", json!({"address": ADDRESS})).await {
        Ok(_) => {}
        Err(error) => panic!("connect failed: {error}"),
    }
    // Injected replies must be queued after connect: connect clears pending.
    // Replies are matched by opcode, so poll one slot against its own frame.
    fake.inject(&status_frame(1));

    // Single slot.
    let payload = call_ok(&client, "mc3000_status", json!({"slot": 1})).await;
    assert_eq!(payload["kind"], "status");
    assert_eq!(payload["status"]["slot"], 1);
    assert_eq!(payload["status"]["status"], "charge");

    // All four slots, production and legacy names.
    fake.inject(&status_frame(0));
    fake.inject(&status_frame(1));
    fake.inject(&status_frame(2));
    fake.inject(&status_frame(3));
    let payload = call_ok(&client, "mc3000_status", Value::Null).await;
    assert_eq!(payload["slots"].as_array().map(Vec::len), Some(4));

    fake.inject(&status_frame(0));
    fake.inject(&status_frame(1));
    fake.inject(&status_frame(2));
    fake.inject(&status_frame(3));
    let payload = call_ok(&client, "charger.get_status", Value::Null).await;
    assert_eq!(payload["slots"].as_array().map(Vec::len), Some(4));

    fake.inject(&status_frame(3));
    let payload = call_ok(&client, "charger.read_slots", json!({"slot": 3})).await;
    assert_eq!(payload["status"]["slot"], 3);
}

#[tokio::test]
async fn version_basic_and_curve_reads() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    call_ok(&client, "mc3000_connect", json!({"address": ADDRESS})).await;
    fake.inject(&version_frame());
    fake.inject(&basic_frame());
    fake.inject(&curve_bytes(0));

    let payload = call_ok(
        &client,
        "mc3000_get_version",
        json!({"mac": "AA:BB:CC:DD:EE:02"}),
    )
    .await;
    assert_eq!(payload["kind"], "version");
    assert_eq!(payload["firmware"], "1.23");
    assert_eq!(payload["hardware"], "2.20");

    let payload = call_ok(&client, "mc3000_get_basic", Value::Null).await;
    assert_eq!(payload["kind"], "basic");
    assert_eq!(payload["basic"]["temp_unit_name"], "celsius");
    assert_eq!(payload["basic"]["system_beep"], true);
    assert_eq!(payload["basic"]["input_mv"], 11000);

    let payload = call_ok(&client, "mc3000_get_voltage_curve", json!({"slot": 0})).await;
    assert_eq!(payload["kind"], "voltage_curve");
    assert_eq!(payload["slot"], 0);
    assert_eq!(payload["point_count"], 3);
    assert_eq!(
        payload["time_series"][1],
        json!({"index": 1, "time_seconds": 5.0, "voltage_mv": 4110})
    );
    let source = payload.get("source").cloned().unwrap_or(Value::Null);
    assert_eq!(source, Value::Null, "legacy curve tool has no source field");

    // The charger.* curve adds the app-style provenance fields.
    fake.inject(&curve_bytes(0));
    let payload = call_ok(&client, "charger.get_voltage_curve", json!({"slot": 0})).await;
    assert_eq!(payload["kind"], "voltage_curve");
    assert_eq!(payload["source"], "charger_voltage_curve_command_0x56");
}

#[tokio::test]
async fn curve_reassembles_from_fragments() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    call_ok(&client, "mc3000_connect", json!({"address": ADDRESS})).await;
    let curve = curve_bytes(2);
    fake.inject(&curve[..20]);
    fake.inject(&curve[20..]);

    let payload = call_ok(&client, "mc3000_get_voltage_curve", json!({"slot": 2})).await;
    assert_eq!(payload["kind"], "voltage_curve");
    assert_eq!(payload["slot"], 2);
    assert_eq!(payload["points_mv"].as_array().map(Vec::len), Some(120));
    assert_eq!(payload["point_count"], 3);
}

#[tokio::test]
async fn curve_export_json_csv_and_bad_format() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    call_ok(&client, "mc3000_connect", json!({"address": ADDRESS})).await;
    fake.inject(&curve_bytes(0));

    let payload = call_ok(
        &client,
        "charger.export_voltage_curve",
        json!({"slot": 0, "format": "csv"}),
    )
    .await;
    assert_eq!(payload["format"], "csv");
    assert_eq!(payload["slot"], 0);
    let csv = payload["content"].as_str().unwrap_or_default();
    assert!(csv.starts_with("index,time_seconds,voltage_mv,voltage_v\n"));
    assert!(csv.contains("1,5.0,4110,4.110"), "csv row missing: {csv}");

    fake.inject(&curve_bytes(0));
    let payload = call_ok(
        &client,
        "charger.export_voltage_curve",
        json!({"slot": 0, "format": "json"}),
    )
    .await;
    assert_eq!(payload["format"], "json");
    assert_eq!(
        payload["content"]["source"],
        "charger_voltage_curve_command_0x56"
    );
    assert_eq!(payload["content"]["point_count"], 3);

    // The default format is json.
    fake.inject(&curve_bytes(0));
    let payload = call_ok(&client, "charger.export_voltage_curve", json!({"slot": 0})).await;
    assert_eq!(payload["format"], "json");

    // The curve is read before the format check (Python behavior), so the
    // bad-format call needs its own reply.
    fake.inject(&curve_bytes(0));
    call_err(
        &client,
        "charger.export_voltage_curve",
        json!({"slot": 0, "format": "xlsx"}),
        "format must be 'json' or 'csv'",
    )
    .await;
}

#[tokio::test]
async fn build_profile_matches_library_frames() {
    let fake = FakeTransport::new();
    let client = harness(&fake).await;
    let payload = call_ok(&client, "mc3000_build_profile", json!({"slot_mask": 1})).await;
    let params = ProfileParams {
        slot_mask: SlotMask::try_from(1_u8).unwrap_or_default(),
        ..ProfileParams::default()
    };
    let frame = match build_profile(&params) {
        Ok(frame) => frame,
        Err(error) => panic!("library profile build failed: {error}"),
    };
    assert_eq!(payload["profile_hex"], json!(hex::encode_upper(frame)));
    let chunks = match protocol::split_profile(&frame) {
        Ok(chunks) => chunks,
        Err(error) => panic!("split failed: {error}"),
    };
    assert_eq!(payload["chunk_1_hex"], json!(hex::encode_upper(chunks.0)));
    assert_eq!(payload["chunk_2_hex"], json!(hex::encode_upper(chunks.1)));
    assert_eq!(
        payload["profile_hex"].as_str().map(str::len),
        Some(protocol::PROFILE_LEN * 2)
    );
}

#[tokio::test]
async fn charger_apply_profile_dry_run_by_default() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    call_ok(&client, "mc3000_connect", json!({"address": ADDRESS})).await;

    let payload = call_ok(
        &client,
        "charger.apply_profile",
        json!({"slot": 3, "profile_id": "nimh-aa-conservative-charge"}),
    )
    .await;
    assert_eq!(payload["dry_run"], json!(true));
    assert_eq!(payload["applied"], json!(false));
    assert_eq!(payload["validation"]["ok"], json!(true));
    assert!(
        payload["profile_hex"].is_string() && payload["chunk_1_hex"].is_string(),
        "dry run reports the built chunks: {payload}"
    );
    assert!(
        fake.writes().is_empty(),
        "dry run must not touch the backend"
    );
}

#[tokio::test]
async fn charger_apply_profile_missing_wrong_correct_token() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    call_ok(&client, "mc3000_connect", json!({"address": ADDRESS})).await;
    let args = json!({
        "slot": 3,
        "profile_id": "nimh-aa-conservative-charge",
        "dry_run": false,
    });

    call_err(
        &client,
        "charger.apply_profile",
        args.clone(),
        "dangerous write requires confirmation_token=APPLY_PROFILE_SLOT_3",
    )
    .await;
    assert!(fake.writes().is_empty());

    call_err(
        &client,
        "charger.apply_profile",
        json!({
            "slot": 3,
            "profile_id": "nimh-aa-conservative-charge",
            "dry_run": false,
            "confirmation_token": "APPLY_PROFILE_SLOT_2",
        }),
        "dangerous write requires confirmation_token=APPLY_PROFILE_SLOT_3",
    )
    .await;
    assert!(fake.writes().is_empty());

    let payload = call_ok(
        &client,
        "charger.apply_profile",
        json!({
            "slot": 3,
            "profile_id": "nimh-aa-conservative-charge",
            "dry_run": false,
            "confirmation_token": "APPLY_PROFILE_SLOT_3",
        }),
    )
    .await;
    assert_eq!(payload["dry_run"], json!(false));
    assert_eq!(payload["applied"], json!(true));
    assert_eq!(payload["started"], json!(false));
    assert_eq!(payload["validation"]["ok"], json!(true));

    // Backend writes: stop slot 3, then the two profile chunks, never a start.
    let writes = fake.writes();
    assert_eq!(writes.len(), 3, "stop + two chunks expected: {writes:?}");
    assert_eq!(writes[0][1], Opcode::Stop.as_u8());
    assert_eq!(writes[0][2], 8, "stop frames carry the slot mask byte");
    assert_eq!(writes[1][1], Opcode::SetProfile.as_u8());
    assert_eq!(
        writes[2][1], 2,
        "chunk 2 byte 1 is the cycle-mode code (D>C = 2)"
    );
}

#[tokio::test]
async fn charger_apply_profile_invalid_validation_returns_result() {
    let fake = FakeTransport::new();
    let client = harness(&fake).await;
    let payload = call_ok(
        &client,
        "charger.apply_profile",
        json!({"slot": 3, "profile": {"profile_id": "bad", "chemistry": "Mystery"}}),
    )
    .await;
    assert_eq!(payload["applied"], json!(false));
    assert_eq!(payload["validation"]["ok"], json!(false));
    assert!(
        payload["validation"]["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty()),
        "validation errors must be listed: {payload}"
    );
    assert!(fake.writes().is_empty());
}

#[tokio::test]
async fn charger_start_missing_wrong_correct_token() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    call_ok(&client, "mc3000_connect", json!({"address": ADDRESS})).await;

    // Missing token: schema-required field, reported as a tool error result.
    match call(&client, "charger.start", json!({"slot": 3})).await {
        Ok(result) => assert_tool_error(&result, "confirmation_token"),
        Err(error) => panic!("unexpected transport failure: {error}"),
    }
    call_err(
        &client,
        "charger.start",
        json!({"slot": 3, "confirmation_token": "START_SLOT_0"}),
        "starting hardware requires confirmation_token=START_SLOT_3",
    )
    .await;
    assert!(fake.writes().is_empty());

    let payload = call_ok(
        &client,
        "charger.start",
        json!({"slot": 3, "confirmation_token": "START_SLOT_3"}),
    )
    .await;
    assert_eq!(payload, json!({"sent": "start", "slot": 3}));
    let writes = fake.writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0][1], Opcode::Start.as_u8());
    assert_eq!(writes[0][2], 8, "start frames carry the slot mask byte");
}

#[tokio::test]
async fn legacy_start_and_set_basic_require_tokens() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    call_ok(&client, "mc3000_connect", json!({"address": ADDRESS})).await;
    fake.inject(&set_basic_ack());

    call_err(
        &client,
        "mc3000_start",
        json!({"slot": 1}),
        "starting hardware requires confirmation_token=START_SLOT_1",
    )
    .await;
    call_err(
        &client,
        "mc3000_start",
        json!({"slot": 1, "confirmation_token": "START_SLOT_2"}),
        "starting hardware requires confirmation_token=START_SLOT_1",
    )
    .await;
    assert!(fake.writes().is_empty());

    let payload = call_ok(
        &client,
        "mc3000_start",
        json!({"slot": 1, "confirmation_token": "START_SLOT_1"}),
    )
    .await;
    assert_eq!(payload, json!({"sent": "start", "slot": 1}));

    call_err(
        &client,
        "mc3000_set_basic",
        json!({"system_beep": true}),
        "device settings write requires confirmation_token=SET_BASIC",
    )
    .await;
    let before = fake.writes().len();
    let payload = call_ok(
        &client,
        "mc3000_set_basic",
        json!({"system_beep": true, "confirmation_token": "SET_BASIC"}),
    )
    .await;
    assert_eq!(payload["kind"], "ack");
    assert_eq!(payload["opcode"], Opcode::SetBasic.as_u8());
    assert_eq!(payload["ok"], json!(true));
    let writes = fake.writes();
    assert_eq!(writes.len(), before + 1);
    assert_eq!(writes[writes.len() - 1][1], Opcode::SetBasic.as_u8());
}

#[tokio::test]
async fn legacy_apply_profile_single_slot_dry_run_and_token() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    call_ok(&client, "mc3000_connect", json!({"address": ADDRESS})).await;
    let profile = fixture_profile(0);
    let profile_hex = hex::encode_upper(&profile);

    // Dry run is the default and touches nothing.
    let payload = call_ok(
        &client,
        "mc3000_apply_profile",
        json!({"profile_hex": profile_hex}),
    )
    .await;
    assert_eq!(payload["dry_run"], json!(true));
    assert_eq!(payload["applied"], json!(false));
    assert!(fake.writes().is_empty());

    // start=true is rejected in favor of the separate start tool.
    call_err(
        &client,
        "mc3000_apply_profile",
        json!({"profile_hex": profile_hex, "start": true}),
        "does not start slots",
    )
    .await;

    // Multi-slot masks are rejected: exactly one selected slot.
    call_err(
        &client,
        "mc3000_apply_profile",
        json!({"slot_mask": 3}),
        "exactly one selected slot",
    )
    .await;
    call_err(
        &client,
        "mc3000_apply_profile",
        json!({"slot_mask": 15}),
        "exactly one selected slot",
    )
    .await;

    // Live write needs the token of the slot named by the frame's mask.
    call_err(
        &client,
        "mc3000_apply_profile",
        json!({"profile_hex": profile_hex, "dry_run": false}),
        "dangerous write requires confirmation_token=APPLY_PROFILE_SLOT_0",
    )
    .await;
    assert!(fake.writes().is_empty());

    let payload = call_ok(
        &client,
        "mc3000_apply_profile",
        json!({
            "profile_hex": profile_hex,
            "dry_run": false,
            "confirmation_token": "APPLY_PROFILE_SLOT_0",
        }),
    )
    .await;
    assert_eq!(payload["dry_run"], json!(false));
    assert_eq!(payload["applied"], json!(true));
    assert_eq!(payload["started"], json!(false));
    let writes = fake.writes();
    assert_eq!(writes.len(), 3, "stop slot 0 + two chunks: {writes:?}");
    assert_eq!(writes[0][1], Opcode::Stop.as_u8());
    assert_eq!(writes[0][2], 1, "slot 0 mask byte is 1");

    // Raw profile fields work too (single slot mask), token slot from the mask.
    let payload = call_ok(
        &client,
        "mc3000_apply_profile",
        json!({
            "slot_mask": 4,
            "dry_run": false,
            "confirmation_token": "APPLY_PROFILE_SLOT_2",
        }),
    )
    .await;
    assert_eq!(payload["applied"], json!(true));
}

#[tokio::test]
async fn legacy_apply_rejects_unknown_profile_fields() {
    let fake = FakeTransport::new();
    let client = harness(&fake).await;
    call_err(
        &client,
        "mc3000_apply_profile",
        json!({"slot_mask": 1, "capacitty": 1}),
        "unexpected profile argument 'capacitty'",
    )
    .await;
    call_err(
        &client,
        "mc3000_build_profile",
        json!({"slot_mask": 1, "nope": 2}),
        "unexpected profile argument 'nope'",
    )
    .await;
}

#[tokio::test]
async fn send_raw_requires_token_and_blocks_restore() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    call_ok(&client, "mc3000_connect", json!({"address": ADDRESS})).await;

    call_err(
        &client,
        "mc3000_send_raw",
        json!({"hex": "0F0500"}),
        "raw write requires confirmation_token=SEND_RAW",
    )
    .await;
    assert!(fake.writes().is_empty());

    let payload = call_ok(
        &client,
        "mc3000_send_raw",
        json!({"hex": "0F0500", "confirmation_token": "SEND_RAW"}),
    )
    .await;
    assert_eq!(payload, json!({"sent_hex": "0F0500"}));
    assert_eq!(fake.writes(), vec![vec![0x0F, 0x05, 0x00]]);

    // frame_hex spelling is accepted as the legacy alias.
    let payload = call_ok(
        &client,
        "mc3000_send_raw",
        json!({"frame_hex": "0FFE03", "confirmation_token": "SEND_RAW"}),
    )
    .await;
    assert_eq!(payload, json!({"sent_hex": "0FFE03"}));

    // Restore factory / calibration opcodes are blocked.
    call_err(
        &client,
        "mc3000_send_raw",
        json!({"hex": hex::encode_upper(ack_frame(Opcode::RestoreFactory)), "confirmation_token": "SEND_RAW"}),
        "restore factory/calibration opcodes are blocked",
    )
    .await;
    call_err(
        &client,
        "mc3000_send_raw",
        json!({"hex": hex::encode_upper(ack_frame(Opcode::RestoreCalibration)), "confirmation_token": "SEND_RAW"}),
        "restore factory/calibration opcodes are blocked",
    )
    .await;

    // start=true is rejected: raw writes never start slots.
    call_err(
        &client,
        "mc3000_send_raw",
        json!({"hex": "0F0500", "start": true, "confirmation_token": "SEND_RAW"}),
        "no longer interprets profiles",
    )
    .await;
}

#[tokio::test]
async fn send_raw_is_never_profile_interpreted() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    call_ok(&client, "mc3000_connect", json!({"address": ADDRESS})).await;
    fake.inject(&status_frame(0));

    // A 40-byte profile is one raw write: no stops, no chunks, no starts.
    let profile = raw_profile_bytes(0x0F);
    let payload = call_ok(
        &client,
        "mc3000_send_raw",
        json!({"hex": hex::encode_upper(&profile), "confirmation_token": "SEND_RAW"}),
    )
    .await;
    assert_eq!(
        payload,
        json!({"sent_hex": hex::encode_upper(&profile)}),
        "raw frames are echoed, not applied"
    );
    let writes = fake.writes();
    assert_eq!(writes.len(), 1, "exactly one raw write expected");
    assert_eq!(writes[0].len(), protocol::PROFILE_LEN);

    // wait=true returns the next parsed notification instead of the echo.
    let payload = call_ok(
        &client,
        "mc3000_send_raw",
        json!({
            "hex": "0F0500",
            "wait": true,
            "timeout": 0.5,
            "confirmation_token": "SEND_RAW",
        }),
    )
    .await;
    assert_eq!(payload["kind"], "status");
}

#[tokio::test]
async fn emergency_stops_are_tokenless() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    call_ok(&client, "mc3000_connect", json!({"address": ADDRESS})).await;

    let payload = call_ok(&client, "mc3000_stop", json!({"slot": 1})).await;
    assert_eq!(payload, json!({"sent": "stop", "slot": 1}));

    let payload = call_ok(&client, "charger.stop_slot", json!({"slot": 0})).await;
    assert_eq!(payload, json!({"sent": "stop", "slot": 0}));

    let payload = call_ok(&client, "charger.stop_all", Value::Null).await;
    assert_eq!(payload, json!({"sent": "stop_all", "slots": [0, 1, 2, 3]}));

    let writes = fake.writes();
    assert_eq!(writes.len(), 6, "two single stops + four stop_all writes");
    assert!(
        writes.iter().all(|write| write[1] == Opcode::Stop.as_u8()),
        "only stop frames expected: {writes:?}"
    );
}

#[tokio::test]
async fn mc5000_stays_read_only() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    let payload = call_ok(
        &client,
        "mc3000_connect",
        json!({"address": ADDRESS, "model": "mc5000"}),
    )
    .await;
    assert_eq!(payload["model"], "mc5000");
    fake.inject(&mc5000_reply(1));

    // Read-only probes work.
    let payload = call_ok(&client, "mc3000_mc5000_status", json!({"slot": 1})).await;
    assert_eq!(payload["kind"], "status");
    assert_eq!(payload["status"]["battery_type"], "NiMH");
    // Reads themselves appear as transport writes; forget them before the
    // refusal checks so only forbidden writes could appear below.
    fake.clear_writes();

    // Every write path refuses before touching the backend.
    call_err(
        &client,
        "mc3000_start",
        json!({"slot": 1, "confirmation_token": "START_SLOT_1"}),
        "read-only",
    )
    .await;
    call_err(&client, "charger.stop_all", Value::Null, "read-only").await;
    call_err(
        &client,
        "mc3000_send_raw",
        json!({"hex": "0F0500", "confirmation_token": "SEND_RAW"}),
        "read-only",
    )
    .await;
    assert!(
        fake.writes().is_empty(),
        "no bytes may reach a read-only model: {:?}",
        fake.writes()
    );
}

#[tokio::test]
async fn status_before_connect_is_a_typed_error() {
    let fake = FakeTransport::new();
    let client = harness(&fake).await;
    call_err(
        &client,
        "mc3000_status",
        json!({"slot": 0}),
        "MC3000 is not connected",
    )
    .await;
}

#[tokio::test]
async fn resources_list_read_and_unknown() {
    let fake = FakeTransport::new();
    let client = harness(&fake).await;

    let listed = match client.list_resources(None).await {
        Ok(result) => result,
        Err(error) => panic!("resources/list failed: {error}"),
    };
    let uris: Vec<&str> = listed.resources.iter().map(|r| r.uri.as_str()).collect();
    assert_eq!(
        uris,
        vec![
            "charger://profiles",
            "charger://profile-schema",
            "charger://safety-limits",
            "charger://device-capabilities",
            "charger://session-log/latest",
        ]
    );
    for resource in &listed.resources {
        assert_eq!(resource.mime_type.as_deref(), Some("application/json"));
    }

    let schema_text = read_resource_text(&client, "charger://profile-schema").await;
    assert!(schema_text.contains("Charger Battery Profile"));

    let profiles_text = read_resource_text(&client, "charger://profiles").await;
    let profiles_json: Value = match serde_json::from_str(&profiles_text) {
        Ok(value) => value,
        Err(error) => panic!("profiles resource is not JSON: {error}"),
    };
    assert!(
        profiles_json.get("nimh-aa-conservative-charge").is_some(),
        "bundled profiles must be listed: {profiles_json}"
    );

    let limits_text = read_resource_text(&client, "charger://safety-limits").await;
    assert!(limits_text.contains("NiMH"));

    let capabilities_text = read_resource_text(&client, "charger://device-capabilities").await;
    assert!(capabilities_text.contains("dangerous_tools_require_confirmation"));

    match client
        .read_resource(ReadResourceRequestParams::new("charger://missing"))
        .await
    {
        Ok(_) => panic!("unknown resource must fail"),
        Err(error) => {
            let message = mcp_error_message(error);
            assert!(
                message.contains("unknown resource: charger://missing"),
                "message: {message}"
            );
        }
    }
}

#[tokio::test]
async fn prompts_list_get_and_unknown() {
    let fake = FakeTransport::new();
    let client = harness(&fake).await;

    let listed = match client.list_prompts(None).await {
        Ok(result) => result,
        Err(error) => panic!("prompts/list failed: {error}"),
    };
    let names: Vec<&str> = listed.prompts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "charger_regular_charge",
            "charger_refresh_nimh",
            "charger_break_in_nimh",
            "charger_lipo_storage",
            "charger_emergency_stop",
        ]
    );

    let prompt = match client
        .get_prompt(GetPromptRequestParams::new("charger_emergency_stop"))
        .await
    {
        Ok(result) => result,
        Err(error) => panic!("prompts/get failed: {error}"),
    };
    assert_eq!(prompt.messages.len(), 1);
    let text = match prompt.messages.first().map(|m| &m.content) {
        Some(ContentBlock::Text(text)) => text.text.clone(),
        other => panic!("expected text prompt content, got {other:?}"),
    };
    assert!(text.contains("stop_all"));
    assert!(text.contains("do not wait for profile validation"));

    match client
        .get_prompt(GetPromptRequestParams::new("missing"))
        .await
    {
        Ok(_) => panic!("unknown prompt must fail"),
        Err(error) => {
            let message = mcp_error_message(error);
            assert!(
                message.contains("unknown prompt: missing"),
                "message: {message}"
            );
        }
    }
}

#[tokio::test]
async fn session_log_redacts_tokens_and_addresses() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    call_ok(&client, "mc3000_connect", json!({"address": ADDRESS})).await;
    call_ok(
        &client,
        "charger.start",
        json!({"slot": 2, "confirmation_token": "START_SLOT_2"}),
    )
    .await;
    let payload = call_ok(
        &client,
        "charger.validate_profile",
        json!({
            "address": "AA:BB:CC:DD:EE:99",
            "profile_id": "nimh-aa-conservative-charge",
        }),
    )
    .await;
    assert_eq!(payload["ok"], json!(true));

    let payload = call_ok(&client, "charger.export_session_log", Value::Null).await;
    let events = payload["events"].as_array().cloned().unwrap_or_default();
    assert_eq!(events.len(), 2, "start + validate events: {events:?}");
    assert_eq!(events[0]["command"], "charger.start");
    assert_eq!(
        events[0]["arguments"]["confirmation_token"], REDACTED_TOKEN,
        "tokens must be redacted in the log"
    );
    assert_eq!(events[0]["result"]["sent"], "start");
    assert_eq!(events[1]["command"], "charger.validate_profile");
    assert_eq!(
        events[1]["arguments"]["address"], REDACTED_ADDRESS,
        "device identifiers must be redacted in the log"
    );

    // The session-log/latest resource mirrors the same events.
    let latest = match client
        .read_resource(ReadResourceRequestParams::new(
            "charger://session-log/latest",
        ))
        .await
    {
        Ok(result) => result,
        Err(error) => panic!("session-log read failed: {error}"),
    };
    let text = match latest.contents.first() {
        Some(rmcp::model::ResourceContents::TextResourceContents { text, .. }) => text.clone(),
        other => panic!("expected text resource contents, got {other:?}"),
    };
    assert!(
        text.contains(REDACTED_TOKEN),
        "resource view must be redacted too"
    );
    assert!(!text.contains("START_SLOT_2"), "raw token leaked: {text}");
}

#[tokio::test]
async fn session_log_is_bounded() {
    let fake = FakeTransport::new();
    let client = harness_with_log(&fake, Arc::new(SessionLog::new(2))).await;
    for _ in 0..4 {
        call_ok(
            &client,
            "charger.validate_profile",
            json!({"profile_id": "nimh-aa-conservative-charge"}),
        )
        .await;
    }
    let payload = call_ok(&client, "charger.export_session_log", Value::Null).await;
    let events = payload["events"].as_array().cloned().unwrap_or_default();
    assert_eq!(events.len(), 2, "oldest events must be evicted: {events:?}");
}

#[tokio::test]
async fn malformed_inputs_are_typed() {
    let fake = FakeTransport::with_scan_devices(vec![device(ADDRESS)]);
    let client = harness(&fake).await;
    call_ok(&client, "mc3000_connect", json!({"address": ADDRESS})).await;

    // Missing required slot: schema validation reports a tool error result.
    match call(&client, "mc3000_start", Value::Null).await {
        Ok(result) => assert_tool_error(&result, "failed to deserialize parameters"),
        Err(error) => panic!("unexpected transport failure: {error}"),
    }

    // Missing slot_mask on a profile build is a typed error.
    call_err(
        &client,
        "mc3000_build_profile",
        Value::Null,
        "slot_mask is required to build a profile frame",
    )
    .await;

    // Out-of-range slot survives deserialization and fails typed.
    call_err(
        &client,
        "mc3000_start",
        json!({"slot": 9, "confirmation_token": "START_SLOT_9"}),
        "slot must be 0..3",
    )
    .await;

    call_err(
        &client,
        "mc3000_get_version",
        json!({"mac": "zz"}),
        "invalid MAC address: 'zz'",
    )
    .await;

    call_err(
        &client,
        "mc3000_build_profile",
        json!({"slot_mask": 16}),
        "slot_mask must be a bitmask 0..15 (slot1=1, slot2=2, slot3=4, slot4=8)",
    )
    .await;

    call_err(
        &client,
        "mc3000_build_profile",
        json!({"slot_mask": 1, "cycle_mode": "nope"}),
        "unknown cycle mode: nope",
    )
    .await;

    call_err(
        &client,
        "mc3000_send_raw",
        json!({"hex": "zz", "confirmation_token": "SEND_RAW"}),
        "invalid hex string: zz",
    )
    .await;

    call_err(
        &client,
        "mc3000_send_raw",
        json!({"confirmation_token": "SEND_RAW"}),
        "requires 'hex' or 'frame_hex'",
    )
    .await;

    call_err(
        &client,
        "mc3000_apply_profile",
        Value::Null,
        "slot_mask is required to build a profile frame",
    )
    .await;

    // Unknown tool.
    match call(&client, "does_not_exist", Value::Null).await {
        Ok(result) => assert_tool_error(&result, "tool not found"),
        Err(error) => {
            let message = mcp_error_message(error);
            assert!(message.contains("tool not found"), "message: {message}");
        }
    }
}

#[tokio::test]
async fn validate_profile_precedence_profile_then_id_then_raw() {
    let fake = FakeTransport::new();
    let client = harness(&fake).await;

    // 1. Explicit `profile` wins over `profile_id`.
    let payload = call_ok(
        &client,
        "charger.validate_profile",
        json!({
            "profile_id": "nimh-aa-conservative-charge",
            "profile": {
                "profile_id": "nimh-aa-direct",
                "chemistry": "NiMH",
                "capacity_mah": 2600,
                "cell_count": 1,
                "nominal_voltage_mv": 1200,
                "operation": "charge",
                "charge_current_ma": 260,
                "discharge_current_ma": 200,
                "charge_voltage_mv": 1650,
                "discharge_cutoff_mv": 1000,
                "timeout_minutes": 960,
                "temperature_cutoff_c": 45,
                "temperature_policy": "supervised",
                "source": "test",
                "risk_level": "low",
            },
        }),
    )
    .await;
    assert_eq!(payload["ok"], json!(true));
    assert_eq!(
        payload["normalized_profile"]["profile_id"],
        "nimh-aa-direct"
    );

    // 2. Known `profile_id` copies the bundled profile.
    let payload = call_ok(
        &client,
        "charger.validate_profile",
        json!({"profile_id": "nimh-aa-conservative-charge"}),
    )
    .await;
    assert_eq!(payload["ok"], json!(true));
    assert_eq!(
        payload["normalized_profile"]["profile_id"],
        "nimh-aa-conservative-charge"
    );

    // 3. Unknown `profile_id` falls back to raw fields (here: none) and fails.
    let payload = call_ok(
        &client,
        "charger.validate_profile",
        json!({"profile_id": "no-such-profile"}),
    )
    .await;
    assert_eq!(payload["ok"], json!(false));

    // 3b. Raw fields validate like a profile object.
    let payload = call_ok(
        &client,
        "charger.validate_profile",
        json!({
            "profile_id": "nimh-aa-raw",
            "chemistry": "NiMH",
            "capacity_mah": 2600,
            "cell_count": 1,
            "nominal_voltage_mv": 1200,
            "operation": "charge",
            "charge_current_ma": 260,
            "discharge_current_ma": 200,
            "charge_voltage_mv": 1650,
            "discharge_cutoff_mv": 1000,
            "timeout_minutes": 960,
            "temperature_cutoff_c": 45,
            "temperature_policy": "supervised",
            "source": "test",
            "risk_level": "low",
        }),
    )
    .await;
    assert_eq!(payload["ok"], json!(true));

    // list_profiles exposes the bundled set under both shapes.
    let payload = call_ok(&client, "charger.list_profiles", Value::Null).await;
    let profiles = payload["profiles"].as_object().cloned().unwrap_or_default();
    assert_eq!(profiles.len(), 4, "four bundled profiles: {profiles:?}");
    assert!(profiles.contains_key("liion-1s-storage"));
}

#[tokio::test]
async fn dry_run_apply_via_raw_safety_fields() {
    let fake = FakeTransport::new();
    let client = harness(&fake).await;
    // Raw fields are the third precedence tier of charger.apply_profile.
    let payload = call_ok(
        &client,
        "charger.apply_profile",
        json!({
            "slot": 1,
            "profile_id": "nimh-aa-conservative-charge",
            "capacity_mah": 2400,
            "dry_run": true,
        }),
    )
    .await;
    // `profile_id` wins over raw fields (tier 2), so the applied capacity is 2600.
    assert_eq!(
        payload["validation"]["normalized_profile"]["capacity_mah"],
        2600
    );
    assert_eq!(payload["applied"], json!(false));
    assert!(fake.writes().is_empty());
}
