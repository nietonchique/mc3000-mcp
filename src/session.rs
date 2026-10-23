//! Bounded, redacted in-memory audit log (`server._session_log`).
//!
//! The Python implementation appended every logged event to an unbounded list
//! and redacted only the top-level `address` argument. This module keeps the
//! same event shape (`{"command", "arguments", "result"}`) with two hardening
//! changes from the rewrite plan (section 4):
//!
//! - the log is a bounded queue: the oldest event is dropped when the
//!   capacity is reached, so process lifetime cannot grow memory without
//!   bound;
//! - device identifiers (`address`) and confirmation tokens
//!   (`confirmation_token`) are redacted at every nesting depth, in
//!   arguments and results alike, before anything is stored.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde_json::Value;

/// Replacement stored for a redacted device identifier.
pub const REDACTED_ADDRESS: &str = "<redacted-device-id>";
/// Replacement stored for a redacted confirmation token.
pub const REDACTED_TOKEN: &str = "<redacted-confirmation-token>";
/// Default capacity of the bounded event queue.
pub const DEFAULT_SESSION_LOG_CAPACITY: usize = 512;

/// Keys whose string values must never be stored in plain text.
const REDACTED_KEYS: [&str; 2] = ["address", "confirmation_token"];

/// One recorded tool event; serialized keys match the Python session log.
#[derive(Debug, Clone, PartialEq)]
struct SessionEvent {
    command: String,
    arguments: Value,
    result: Value,
}

/// Bounded, redacted audit log shared by the MCP service and the
/// `charger://session-log/latest` resource.
#[derive(Debug)]
pub struct SessionLog {
    events: Mutex<VecDeque<SessionEvent>>,
    capacity: usize,
}

impl SessionLog {
    /// Create a log that keeps at most `capacity` events (oldest dropped
    /// first). A capacity of zero keeps nothing.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            events: Mutex::new(VecDeque::new()),
            capacity,
        }
    }

    /// Record one event, redacting arguments and results and evicting the
    /// oldest event when the queue is full.
    pub fn record(&self, command: &str, arguments: &Value, result: &Value) {
        let event = SessionEvent {
            command: command.to_owned(),
            arguments: redact(arguments),
            result: redact(result),
        };
        let mut events = Self::lock(&self.events);
        if self.capacity == 0 {
            return;
        }
        if events.len() >= self.capacity {
            events.pop_front();
        }
        events.push_back(event);
    }

    /// Snapshot of every stored event, oldest first, as JSON objects with the
    /// Python keys `command`, `arguments`, and `result`.
    #[must_use]
    pub fn events(&self) -> Vec<Value> {
        let events = Self::lock(&self.events);
        events
            .iter()
            .map(|event| {
                serde_json::json!({
                    "command": event.command,
                    "arguments": event.arguments,
                    "result": event.result,
                })
            })
            .collect()
    }

    /// Number of stored events.
    #[must_use]
    pub fn len(&self) -> usize {
        Self::lock(&self.events).len()
    }

    /// Whether no events are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events().is_empty()
    }

    fn lock(
        events: &Mutex<VecDeque<SessionEvent>>,
    ) -> std::sync::MutexGuard<'_, VecDeque<SessionEvent>> {
        match events.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl Default for SessionLog {
    fn default() -> Self {
        Self::new(DEFAULT_SESSION_LOG_CAPACITY)
    }
}

/// Copy `value` with every `address`/`confirmation_token` string replaced by
/// its redaction marker, at any nesting depth.
#[must_use]
pub fn redact(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (key, item) in map {
                if REDACTED_KEYS.contains(&key.as_str()) && item.is_string() {
                    let marker = if key == "address" {
                        REDACTED_ADDRESS
                    } else {
                        REDACTED_TOKEN
                    };
                    out.insert(key.clone(), Value::String(marker.to_owned()));
                    continue;
                }
                out.insert(key.clone(), redact(item));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_SESSION_LOG_CAPACITY, REDACTED_ADDRESS, REDACTED_TOKEN, SessionLog};
    use serde_json::json;

    #[test]
    fn redacts_addresses_and_tokens_at_depth() {
        let recorded = json!({
            "address": "AA:BB:CC:DD:EE:FF",
            "confirmation_token": "START_SLOT_1",
            "profile": {"address": "AA:BB", "note": "keep"},
            "results": [{"confirmation_token": "SET_BASIC"}],
            "note": "plain",
        });
        let redacted = super::redact(&recorded);
        assert_eq!(redacted["address"], REDACTED_ADDRESS);
        assert_eq!(redacted["confirmation_token"], REDACTED_TOKEN);
        assert_eq!(redacted["profile"]["address"], REDACTED_ADDRESS);
        assert_eq!(redacted["profile"]["note"], "keep");
        assert_eq!(redacted["results"][0]["confirmation_token"], REDACTED_TOKEN);
        assert_eq!(redacted["note"], "plain");
    }

    #[test]
    fn keeps_events_in_order_with_shape() {
        let log = SessionLog::new(4);
        log.record(
            "charger.start",
            &json!({"slot": 1}),
            &json!({"sent": "start"}),
        );
        log.record(
            "charger.stop_all",
            &json!({}),
            &json!({"slots": [0, 1, 2, 3]}),
        );
        let events = log.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["command"], "charger.start");
        assert_eq!(events[1]["command"], "charger.stop_all");
        assert_eq!(events[0]["result"]["sent"], "start");
    }

    #[test]
    fn evicts_oldest_when_full() {
        let log = SessionLog::new(3);
        for index in 0..5 {
            log.record(
                "charger.validate_profile",
                &json!({"index": index}),
                &json!({"ok": true}),
            );
        }
        let events = log.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["arguments"]["index"], 2);
        assert_eq!(events[2]["arguments"]["index"], 4);
        assert!(log.len() <= DEFAULT_SESSION_LOG_CAPACITY);
    }

    #[test]
    fn zero_capacity_keeps_nothing() {
        let log = SessionLog::new(0);
        log.record("charger.start", &json!({}), &json!({}));
        assert!(log.is_empty());
    }

    #[test]
    fn redaction_applies_to_stored_events() {
        let log = SessionLog::new(8);
        log.record(
            "charger.start",
            &json!({"slot": 2, "confirmation_token": "START_SLOT_2"}),
            &json!({"sent": "start", "address": "AA:BB:CC:DD:EE:02"}),
        );
        let event = &log.events()[0];
        assert_eq!(event["arguments"]["confirmation_token"], REDACTED_TOKEN);
        assert_eq!(event["result"]["address"], REDACTED_ADDRESS);
        assert_eq!(event["arguments"]["slot"], 2);
    }
}
