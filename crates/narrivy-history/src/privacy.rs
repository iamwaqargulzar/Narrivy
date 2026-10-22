//! Privacy core — re-exports PrivacyZone and is_allowed helper.
//! Mission spec: PrivacyZone {id, app_ids: Vec<String>, deny: bool} persisted; is_allowed(app_id) checked BEFORE history insert.

use crate::privacy_zones::{PrivacyZone, ZoneMatcher};

pub use crate::privacy_zones::{
    PrivacyZone as PrivacyZoneExport, PrivacyZoneAction, ZoneMatcher as PrivacyZoneMatcher,
};

/// Check if app_id is allowed given slice of zones. Deny overrides allow.
/// This is the mission-required is_allowed(app_id) helper.
pub fn is_allowed(app_id: &str, zones: &[PrivacyZone]) -> bool {
    ZoneMatcher::new(zones.to_vec()).is_allowed(app_id)
}

/// Convenience wrapper: is_allowed with single app_id and matcher.
pub fn is_allowed_matcher(app_id: &str, matcher: &ZoneMatcher) -> bool {
    matcher.is_allowed(app_id)
}

// Re-export PrivacyZone for direct use
pub type PrivacyZoneType = PrivacyZone;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::privacy_zones::{PrivacyZone, PrivacyZoneAction};

    #[test]
    fn privacy_is_allowed_true_when_no_zones() {
        let zones: Vec<PrivacyZone> = vec![];
        assert!(is_allowed("any.app", &zones));
    }

    #[test]
    fn privacy_is_allowed_false_when_deny_matches() {
        let zone = PrivacyZone {
            id: "z".to_string(),
            pattern: "".to_string(),
            app_ids: vec!["app_x".to_string()],
            deny: true,
            actions: vec![PrivacyZoneAction::DenyHistory],
        };
        assert!(!is_allowed("app_x", &[zone]));
    }

    #[test]
    fn privacy_is_allowed_deny_overrides() {
        let allow = PrivacyZone {
            id: "a".to_string(),
            pattern: "".to_string(),
            app_ids: vec!["app_x".to_string()],
            deny: false,
            actions: vec![],
        };
        let deny = PrivacyZone {
            id: "d".to_string(),
            pattern: "".to_string(),
            app_ids: vec!["app_x".to_string()],
            deny: true,
            actions: vec![],
        };
        assert!(!is_allowed("app_x", &[allow, deny]));
    }
}
