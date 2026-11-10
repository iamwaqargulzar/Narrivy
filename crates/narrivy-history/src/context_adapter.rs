//! Context privacy adapter — P8 FUN-064 / PRIV-007.
//!
//! `ContextProvider` is isolated helper used ONLY when history is On and
//! PrivacyZone allows. When history Off, context returns None (no leakage).
//! Simple `NoopContext` for P8 satisfies vacuously when broad providers deferred.
//!
//! This module does NOT log context content; diagnostics redaction is via
//! `narrivy-diagnostics::Redactor` (already proven).

use crate::context::{ContextProvider, ContextSnapshot, NoopContextProvider};
use crate::history::RetentionPolicy;
use crate::privacy_zones::{PrivacyZoneAction, ZoneMatcher};

/// Result of a policy-gated context collection attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextGateDecision {
    /// Allowed — snapshot may be collected.
    Allowed,
    /// Denied because history retention is Off.
    DeniedHistoryOff,
    /// Denied by a PrivacyZone (DenyContext or DisableNarrivy).
    DeniedByZone { zone_id: String },
}

/// Central gate: checks history retention and PrivacyZone before collecting context.
/// Pure function, no I/O, no logging of content.
pub fn gate_context(
    retention: RetentionPolicy,
    matcher: &ZoneMatcher,
    app_id: &str,
) -> ContextGateDecision {
    if retention == RetentionPolicy::Off {
        return ContextGateDecision::DeniedHistoryOff;
    }
    for zone in matcher.zones() {
        if zone_matches(zone, app_id)
            && (zone.actions.contains(&PrivacyZoneAction::DenyContext)
                || zone.actions.contains(&PrivacyZoneAction::DisableNarrivy))
        {
            return ContextGateDecision::DeniedByZone {
                zone_id: zone.id.clone(),
            };
        }
    }
    ContextGateDecision::Allowed
}

/// Collect context only if gate allows. Returns None when denied (no leakage).
/// Isolated helper — caller must ensure history Off never reaches provider.
pub fn maybe_collect_context<P: ContextProvider>(
    provider: &P,
    retention: RetentionPolicy,
    matcher: &ZoneMatcher,
    app_id: &str,
) -> Option<ContextSnapshot> {
    match gate_context(retention, matcher, app_id) {
        ContextGateDecision::Allowed => Some(provider.snapshot()),
        _ => None,
    }
}

/// Simple pattern matcher for PrivacyZone pattern (substring contains, case-insensitive).
/// Skeleton: real matcher will be richer; this satisfies P8 isolation test.
fn zone_matches(zone: &crate::privacy_zones::PrivacyZone, app_id: &str) -> bool {
    let pat = zone.pattern.trim().to_ascii_lowercase();
    if pat.is_empty() {
        return false;
    }
    app_id.to_ascii_lowercase().contains(&pat)
}

/// `NoopContext` alias for P8 — vacuously satisfies minimal set.
pub type NoopContext = NoopContextProvider;

/// Helper that demonstrates history-Off isolation: returns None without touching provider.
/// Test-only tracking provider wrapper used in unit tests.
#[cfg(test)]
pub mod test_helpers {
    use crate::context::{ContextField, ContextProvider, ContextSnapshot};
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Provider that records whether snapshot() was called.
    pub struct TrackingProvider {
        pub called: AtomicBool,
        pub snapshot_data: ContextSnapshot,
    }

    impl TrackingProvider {
        pub fn new(snapshot_data: ContextSnapshot) -> Self {
            Self {
                called: AtomicBool::new(false),
                snapshot_data,
            }
        }

        pub fn was_called(&self) -> bool {
            self.called.load(Ordering::SeqCst)
        }
    }

    impl ContextProvider for TrackingProvider {
        fn requested_fields(&self) -> Vec<ContextField> {
            vec![ContextField::AppName]
        }

        fn sensitivity(&self) -> &'static str {
            "low"
        }

        fn snapshot(&self) -> ContextSnapshot {
            self.called.store(true, Ordering::SeqCst);
            self.snapshot_data.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::TrackingProvider;
    use super::{ContextGateDecision, gate_context, maybe_collect_context};
    use crate::context::NoopContextProvider;
    use crate::context::{ContextField, ContextSnapshot};
    use crate::history::RetentionPolicy;
    use crate::privacy_zones::{PrivacyZone, PrivacyZoneAction, ZoneMatcher};

    fn empty_matcher() -> ZoneMatcher {
        ZoneMatcher::new(vec![])
    }

    #[test]
    fn history_off_returns_none_no_leakage() {
        let provider = TrackingProvider::new(ContextSnapshot {
            fields: vec![(ContextField::AppName, "secret-app".to_owned())],
        });
        let result =
            maybe_collect_context(&provider, RetentionPolicy::Off, &empty_matcher(), "any.app");
        assert!(result.is_none(), "when history Off, context must be None");
        assert!(
            !provider.was_called(),
            "provider snapshot() must NOT be called when history Off (no leakage)"
        );
    }

    #[test]
    fn history_on_allows_collection() {
        let provider = TrackingProvider::new(ContextSnapshot {
            fields: vec![(ContextField::AppName, "ok".to_owned())],
        });
        let result = maybe_collect_context(
            &provider,
            RetentionPolicy::Days30,
            &empty_matcher(),
            "any.app",
        );
        assert!(result.is_some());
        assert!(provider.was_called());
        assert_eq!(
            result
                .as_ref()
                .map(|snapshot| snapshot.fields[0].1.as_str()),
            Some("ok")
        );
    }

    #[test]
    fn privacy_zone_deny_context_blocks_even_when_history_on() {
        let provider = TrackingProvider::new(ContextSnapshot {
            fields: vec![(ContextField::AppName, "leak".to_owned())],
        });
        let zone = PrivacyZone {
            id: "zone1".to_owned(),
            pattern: "bank".to_owned(),
            app_ids: vec![],
            deny: true,
            actions: vec![PrivacyZoneAction::DenyContext],
        };
        let matcher = ZoneMatcher::new(vec![zone]);
        // matching app should be blocked
        let result =
            maybe_collect_context(&provider, RetentionPolicy::Days30, &matcher, "com.bank.app");
        assert!(result.is_none());
        assert!(!provider.was_called());
        // non-matching app allowed
        let provider2 = TrackingProvider::new(ContextSnapshot {
            fields: vec![(ContextField::AppName, "ok2".to_owned())],
        });
        let result2 = maybe_collect_context(
            &provider2,
            RetentionPolicy::Days30,
            &matcher,
            "com.other.app",
        );
        assert!(result2.is_some());
        assert!(provider2.was_called());
    }

    #[test]
    fn privacy_zone_disable_narrivy_also_blocks_context() {
        let zone = PrivacyZone {
            id: "zone2".to_owned(),
            pattern: "secret".to_owned(),
            app_ids: vec![],
            deny: true,
            actions: vec![PrivacyZoneAction::DisableNarrivy],
        };
        let matcher = ZoneMatcher::new(vec![zone]);
        let provider = TrackingProvider::new(ContextSnapshot { fields: vec![] });
        let result =
            maybe_collect_context(&provider, RetentionPolicy::Forever, &matcher, "secret-app");
        assert!(result.is_none());
        assert!(!provider.was_called());
    }

    #[test]
    fn noop_context_returns_empty_snapshot_when_allowed() {
        let matcher = empty_matcher();
        let provider = NoopContextProvider;
        let snap = maybe_collect_context(&provider, RetentionPolicy::Days7, &matcher, "any");
        assert!(snap.is_some());
        assert_eq!(snap.as_ref().map(|snapshot| snapshot.fields.len()), Some(0));
    }

    #[test]
    fn gate_decision_variants() {
        assert_eq!(
            gate_context(RetentionPolicy::Off, &empty_matcher(), "x"),
            ContextGateDecision::DeniedHistoryOff
        );
        assert_eq!(
            gate_context(RetentionPolicy::Days30, &empty_matcher(), "x"),
            ContextGateDecision::Allowed
        );
        let zone = PrivacyZone {
            id: "z".to_owned(),
            pattern: "test".to_owned(),
            app_ids: vec![],
            deny: true,
            actions: vec![PrivacyZoneAction::DenyContext],
        };
        let m = ZoneMatcher::new(vec![zone]);
        assert!(matches!(
            gate_context(RetentionPolicy::Days30, &m, "mytestapp"),
            ContextGateDecision::DeniedByZone { .. }
        ));
    }

    #[test]
    fn history_off_overrides_even_matching_zone() {
        // history Off takes precedence, still None and not called
        let zone = PrivacyZone {
            id: "z".to_owned(),
            pattern: "app".to_owned(),
            app_ids: vec![],
            deny: true,
            actions: vec![PrivacyZoneAction::DenyContext],
        };
        let matcher = ZoneMatcher::new(vec![zone]);
        let provider = TrackingProvider::new(ContextSnapshot { fields: vec![] });
        let result = maybe_collect_context(&provider, RetentionPolicy::Off, &matcher, "app");
        assert!(result.is_none());
        assert!(!provider.was_called());
    }
}
