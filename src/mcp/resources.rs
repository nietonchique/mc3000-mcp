//! MCP resources (`server.RESOURCES`): the same five `charger://` URIs with
//! `application/json` payloads built from the pure layers and the session log.

use rmcp::model::Resource;
use serde_json::Value;

use crate::session::SessionLog;

/// Resource URIs in the Python definition order.
pub const RESOURCE_URIS: [&str; 5] = [
    "charger://profiles",
    "charger://profile-schema",
    "charger://safety-limits",
    "charger://device-capabilities",
    "charger://session-log/latest",
];

/// Resource descriptors for `resources/list`: name is the URI without the
/// `charger://` prefix, MIME type is `application/json` (Python behavior).
#[must_use]
pub fn list() -> Vec<Resource> {
    RESOURCE_URIS
        .iter()
        .map(|uri| {
            Resource::new(*uri, uri.strip_prefix("charger://").unwrap_or(*uri))
                .with_mime_type("application/json")
        })
        .collect()
}

/// Build the payload of one resource; `None` for an unknown URI. The session
/// log is passed in so `charger://session-log/latest` always reflects the
/// current process state.
#[must_use]
pub fn read(uri: &str, session: &SessionLog) -> Option<Value> {
    match uri {
        "charger://profiles" => Some(Value::Object(crate::safety::profiles())),
        "charger://profile-schema" => Some(crate::safety::profile_schema()),
        "charger://safety-limits" => Some(crate::safety::safety_limits()),
        "charger://device-capabilities" => Some(serde_json::json!({
            "devices": ["SKYRC MC3000", "BLE-compatible MC3000 clones/rebrands"],
            "slots": 4,
            "transport": "Bluetooth Low Energy Nordic-UART-like FFE0/FFE1",
            "dangerous_tools_require_confirmation": true,
        })),
        "charger://session-log/latest" => Some(Value::Array(session.events())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{list, read};
    use crate::session::SessionLog;
    use serde_json::json;

    #[test]
    fn list_matches_python_uris_and_names() {
        let resources = list();
        assert_eq!(resources.len(), 5);
        assert_eq!(resources[0].uri, "charger://profiles");
        assert_eq!(resources[0].name, "profiles");
        assert_eq!(resources[1].uri, "charger://profile-schema");
        assert_eq!(resources[1].name, "profile-schema");
        assert_eq!(resources[4].uri, "charger://session-log/latest");
        assert_eq!(resources[4].name, "session-log/latest");
        for resource in &resources {
            assert_eq!(resource.mime_type.as_deref(), Some("application/json"));
        }
    }

    #[test]
    fn read_covers_all_uris_and_rejects_unknown() {
        let session = SessionLog::default();
        for uri in super::RESOURCE_URIS {
            assert!(read(uri, &session).is_some(), "missing payload for {uri}");
        }
        assert!(read("charger://missing", &session).is_none());
    }

    #[test]
    fn session_log_resource_reflects_recorded_events() {
        let session = SessionLog::default();
        session.record(
            "charger.start",
            &json!({"slot": 0}),
            &json!({"sent": "start"}),
        );
        let payload = read("charger://session-log/latest", &session).unwrap_or_default();
        assert_eq!(payload[0]["command"], "charger.start");
    }
}
