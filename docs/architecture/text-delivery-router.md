# Text Delivery Router

Text delivery is a transaction-oriented, target-aware subsystem. Linux implementation details are qualified in `docs/platforms/linux-text-delivery-matrix.md`.

## Inputs
- captured target identity/context allowed by policy;
- runtime platform/backend capabilities and health;
- operation (`insert`, later `replace-selection`);
- Unicode/multiline/content-shape requirements;
- target application compatibility observations;
- current permission state;
- user/profile preference that does not violate safety/privacy;
- recent bounded backend failures.

## Transaction invariants

1. assign delivery transaction tied to the dictation session;
2. ensure final text is recoverable before risky delivery attempt where required;
3. choose highest-ranked **qualified compatible** backend;
4. attempt with deadline/cancellation;
5. verify commit where the backend can provide meaningful verification;
6. on failure, record content-free reason and try only the bounded approved fallback chain;
7. final safe fallback is Inbox;
8. mark committed once so a late/stale retry cannot duplicate insertion.

## Linux helpers

Input-method/native, automation protocol, virtual-keyboard/helper utility and clipboard-paste mechanisms are candidates, not assumed support. Any external utility is spawned with explicit argv (never shell-concatenated text), capability/version detected, permission-scoped and failure-tested.

The router learns health/compatibility metadata, never the user's transcript content, for backend ranking.

## Current installed composition

The strict installed lane `linux-mic-routed-inbox` composes the real
`RoutedThenInboxDelivery` in `narrivyd` with the daemon-owned portal/EIS
permission handle (`PortalPermissionSubmitter`), the trusted `wtype` helper and
the clipboard rungs declared unavailable
(`DeliveryIntent.backend = routed-then-inbox`). A rung whose helper accepted
the whole text ends the ladder as `SubmittedUnverified` with no Inbox copy
(F-M4); every other non-verified outcome (clipboard only, a failure after
dispatch, nothing qualified) ends in exactly one durable Inbox recovery write
under the same transaction.

The delivery target is bound once at session admission, before any
activation/focus change, and consumed immutably by the adapter — it is never
queried at delivery time. The installed Start path cannot yet supply a
trustworthy pre-activation binding, so it passes the explicit typed-unavailable
`pre_activation_target_binding_absent`: every target-requiring rung (portal/EIS,
clipboard paste) is skipped and the exact corrected text goes durably to the
Inbox. The legacy PATH-resolved `xprop` target probe and the
`wl-copy`/`wl-paste`/`xclip`/paste-chord helpers are not part of the production
design; the clipboard rungs are reported unavailable until a separate trusted
implementation is accepted. The explicit Inbox-only lane (`linux-mic-inbox`) and
the single-wtype lane (`linux-mic-wtype-inbox`) remain available with unchanged
meaning, and the packaged default stays on the accepted Inbox lane. No rung can
verify insertion in this increment, so `DeliveryOutcome::Delivered` is not
claimed by any real backend and native target-application qualification is still
open (ADR-0072).
