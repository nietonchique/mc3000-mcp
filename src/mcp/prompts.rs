//! MCP prompts (`server.PROMPTS`): five safe-charging prompts, each returned
//! as a single user text message.

use rmcp::model::{GetPromptResult, Prompt, PromptMessage, Role};

/// Every prompt name with its exact Python description text, in definition
/// order.
pub const PROMPTS: [(&str, &str); 5] = [
    (
        "charger_regular_charge",
        "Collect chemistry, capacity_mAh, slot, nominal voltage, charge current, cutoff voltage, \
         temperature policy, and source. Run charger.validate_profile, show dry-run, then ask \
         for explicit START_SLOT_<slot> confirmation before charger.start.",
    ),
    (
        "charger_refresh_nimh",
        "For NiMH refresh: verify NiMH chemistry/capacity/slot, use conservative currents, \
         validate, dry-run, require explicit confirmation, then monitor status and temperature.",
    ),
    (
        "charger_break_in_nimh",
        "Use only for known NiMH/NiCd cells that need forming. Ask why break-in is needed, \
         validate C/10 charge and C/5 discharge, dry-run, confirm, monitor.",
    ),
    (
        "charger_lipo_storage",
        "Refuse multi-cell packs unless the charger supports per-cell/balance handling. Validate \
         cell count and storage voltage, dry-run by default, require explicit confirmation.",
    ),
    (
        "charger_emergency_stop",
        "Call charger.stop_slot or charger.stop_all immediately; do not wait for \
         profile validation.",
    ),
];

/// Prompt descriptors for `prompts/list`.
#[must_use]
pub fn list() -> Vec<Prompt> {
    PROMPTS
        .iter()
        .map(|(name, description)| Prompt::new(*name, Some(*description), None))
        .collect()
}

/// The message list of one prompt; `None` for an unknown name.
#[must_use]
pub fn get(name: &str) -> Option<GetPromptResult> {
    let (_, description) = PROMPTS.iter().find(|(prompt, _)| *prompt == name)?;
    Some(GetPromptResult::new(vec![PromptMessage::new_text(
        Role::User,
        *description,
    )]))
}

#[cfg(test)]
mod tests {
    use super::{PROMPTS, get, list};
    use rmcp::model::{ContentBlock, Role};

    #[test]
    fn list_has_python_names_and_descriptions() {
        let prompts = list();
        assert_eq!(prompts.len(), 5);
        for (index, prompt) in prompts.iter().enumerate() {
            assert_eq!(prompt.name, PROMPTS[index].0);
            assert_eq!(prompt.description.as_deref(), Some(PROMPTS[index].1));
        }
    }

    #[test]
    fn get_returns_single_user_text_message() {
        let result = get("charger_emergency_stop").unwrap_or_default();
        assert_eq!(result.messages.len(), 1);
        let message = &result.messages[0];
        assert!(matches!(message.role, Role::User));
        let ContentBlock::Text(text) = message.content.clone() else {
            panic!("prompt content must be text");
        };
        assert!(text.text.contains("stop_all"));
        assert!(get("missing").is_none());
    }
}
