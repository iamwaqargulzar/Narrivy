# narrivy-diagnostics — Implementation Notes

Content-safe diagnostics foundation feeding later SEC-005 / TEST-003 evidence.
Scope of this change: only files under `crates/narrivy-diagnostics/`.

## Modules

| Module | Contents |
|---|---|
| `redactor.rs` | `Redactor`, `DEFAULT_DENIED_KEYS`, `DEFAULT_MAX_STRING_LEN` (256), `scrub_kv` |
| `health.rs` | `HealthFact`, `HealthRegistry`, `DEFAULT_HISTORY_CAP_PER_SUBSYSTEM` (64) |
| `latency.rs` | `LatencySample`, `LatencyTracker` (ring 512), `LatencySummary` |
| `support.rs` | `SupportReport` (schema_version = 1), pretty JSON in/out, `content_check`/`ContentCheck` |
| `error.rs` | `DiagnosticsError` (thiserror): `Json`, `InvalidConfig` |

## Redactor

- Deny-list defaults: `transcript, text, audio, samples, clipboard, token,
  secret, password, authorization, cookie, last_delivered, delivered, words,
  spoken, utterance, tokens, context, passphrase, api_key, apikey, access_key,
  private_key`. Configurable via `with_denied_keys` (replaces) and `deny_key`
  (adds).
- The list is at least as strict as the identifier vocabulary
  `scripts/check_log_redaction.py` classifies as content- or
  credential-bearing, so the gate's "value routed through the Redactor"
  exemption cannot be satisfied by a field the Redactor would pass through.
  Deliberately excluded: `environment` (it is `SupportReport`'s own field, and
  env-dump safety is by construction), `selection` (a delivery mode, not
  content; `selected_text` is already covered by `text`), `recognition` (a
  settings name, not content), `credential` (ambiguous with metadata such as
  `credential_type`).
- Key matching is case-insensitive and tokenized: a key matches when its
  lowercased form equals an entry **or** any alphanumeric subtoken equals an
  entry. Subtokens split on non-alphanumeric separators (`_ - . / ` etc.) *and*
  on lower-to-upper case transitions, so `audio_samples`, `auth.token.value`,
  and `LastDelivered` match; `texture` does not falsely match `text`.
- Accepted fail-safe cost of subtoken matching: `has_delivered_text`,
  `spoken_language`, and `context_hint` are denied even when they carry no
  speech, because subtoken matching cannot distinguish them from the
  content-bearing names. Pinned by
  `speech_and_delivery_subtokens_redact_absent_presence_qualifiers`.
- `redact_json(&Value) -> Value`: object keys matching the deny-list have
  their value replaced by `[REDACTED:<len>]`; arrays/objects are recursed;
  string leaves are scrubbed with `scrub_string`.
- String scrubbing: first embedded `key=value` / `key: value` pairs whose key
  matches the deny-list (quoted values with backslash escapes supported,
  bare tokens ended by space/tab/,/;/&/newline) are replaced by
  `[REDACTED:<len>]`; then any remaining string longer than `max_string_len`
  is truncated at a UTF-8 char boundary with `[TRUNCATED:orig_len=N]`
  (N = original code point count). Order matters: kv-scrub before truncate so
  short secrets inside long strings are still redacted, not cut away.
- `<len>` for strings is the code point count; for non-string JSON values it
  is the compact serialized byte length.

## HealthRegistry

- `observe(HealthFact)` keyed by subsystem, latest-wins (`current:
  BTreeMap<String, HealthFact>`); `snapshot()` returns latest fact per
  subsystem sorted by subsystem name. History kept per subsystem as a
  bounded deque (default cap 64 each, oldest dropped);
  `history_snapshot()` returns it grouped per subsystem, oldest first.
- `healthy()`: true iff every current fact is `ok`; empty registry is healthy.

## LatencyTracker

- Bounded ring (default 512) of `(stage, micros)`; eviction drops oldest.
- Percentiles use nearest-rank over an ascending sort copy:
  `rank = ceil(percent * n / 100)` clamped to `[1, n]` — deterministic for a
  given sample set. Empty tracker returns `None` from `p50/p95/percentile`,
  never panics. `LatencySummary::from(&tracker)` gives `{count, p50, p95}`.

## SupportReport

- `schema_version` fixed at 1. Fields: `generated_unix_ms`, `build_id`,
  `health: Vec<HealthFact>` (sorted), `latency: LatencySummary`,
  `environment: BTreeMap<String, String>`.
- Built ONLY from explicit caller inputs. Nothing reads `env::vars()`;
  the report has no constructor that samples the process environment.
- `to_json_pretty()` / `from_json_str()` round-trip; errors are typed via
  `DiagnosticsError`.
- Final content-safety gate: serialize → parse → `Redactor::redact_json`.
  Tested: a mistakenly added `transcript` environment entry and a
  `audio=raw_pcm_bytes` kv pair inside another field both come out redacted,
  as do `last_delivered`, `spoken`, and `asr_words` environment entries
  (`environment` is the only caller-chosen key path into a report, so a caller
  mistake is the only way content reaches one).
- `content_check() -> ContentCheck` is the pre-share verdict a caller can fail
  closed on: `content_free` is true only when the report is byte-identical to
  its own redaction (no denied field name anywhere, and no surviving string
  that embeds a denied `key=value` pair). Truncation is reported separately as
  `overlong_fields` because losing fidelity is not a content leak. A report
  whose health code or environment value embeds speech is reported as
  `health[0].code` / `environment.<key>` paths, and `ContentCheck::summary()`
  renders the verdict as one content-free line.
- `Redactor::scrub_kv` exposes the kv-pair half of `scrub_string` so a caller
  can tell "carries denied content" apart from "longer than the bound".

## Production composition (REL-008)

The crate has two real production consumers; neither is allowed to hold user
content:

- `services/narrivyd/src/daemon/diagnostics.rs` composes `Redactor` for every
  free-form diagnostic message the daemon emits (`content_safe`), and owns the
  `HealthRegistry` + `LatencyTracker` behind `DaemonDiagnostics`. The surface
  is an optional composition owned by the composition root: `None` makes
  `GetStatus` advertise `diagnostics.support-report=unavailable:not_composed`,
  mirroring the retained-History convention.
- `apps/narrivy-cli/src/support.rs` renders the daemon's advertised
  `diagnostics.*` capabilities into a `SupportReport` for
  `narrivy-cli support-report [--output PATH]`, gating on
  `diagnostics.support-report=available`, reducing every capability to
  (subsystem, code, verdict) so no daemon-supplied value is ever copied, and
  refusing to write a report that fails `content_check`.

## Concurrency & lints

- Registry/tracker state sits behind `std::sync::Mutex`; poisoning is
  recovered with `unwrap_or_else(|p| p.into_inner())` (diagnostics must keep
  working after a panic elsewhere).
- No `unsafe` (workspace `unsafe_code = "forbid"`),
  no `unwrap`/`expect`/`panic!` anywhere; all fallible paths return
  `DiagnosticsError` or `Option`.

## Verification

- `cargo test -p narrivy-diagnostics --locked` — 35 passed, 0 failed.
- `cargo test -p narrivy-cli -p narrivyd --locked` — 29 CLI tests (incl. the
  shipped-binary support-report suite) and the full daemon suite pass.
- `cargo clippy -p narrivy-diagnostics -p narrivy-cli -p narrivyd --all-targets
  --locked -- -D warnings` — clean.
- `python3 scripts/check_log_redaction.py` — exit 0, 129 Rust sources scanned.
- `python3 scripts/verify_dependency_boundaries.py` — no longer reports
  `narrivy-diagnostics` as unreachable: the crate now has two production
  reverse dependencies (`narrivyd`, `narrivy-cli`).
