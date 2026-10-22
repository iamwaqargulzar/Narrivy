//! Daemon submodules: capture, policy, portal-session ownership, session.

#![forbid(unsafe_code)]
#![allow(unused_imports)]

pub mod assistant;
pub mod assistant_builtin;
pub mod assistant_library;
pub mod assistant_tools;
pub mod browser;
pub mod capture;
pub mod compute;
pub mod diagnostics;
pub mod download;
pub mod echo;
pub mod hold;
pub(crate) mod ipc_policy;
pub mod language_models;
pub mod live_preview;
pub mod meetings;
pub mod model_selection;
pub mod models;
pub mod openvino;
pub mod policy;
pub mod portal;
pub(crate) mod provisioning;
pub mod refine;
pub mod retained_history;
pub mod session;
pub mod shortcuts;
pub mod shutdown;
pub mod sounds;
pub mod store;
pub mod text_options;
pub mod vocabulary;
pub mod voice;
pub mod voice_mode;
pub mod writing;

pub use self::compute::{
    COMPUTE_POLICY_ENV, ComputePlan, DEVICE_RESOLVED_CAPABILITY, DEVICE_UNAVAILABLE_CAPABILITY,
    QUALIFICATIONS_FILE, RUNTIME_PACK_CAPABILITY_PREFIX, build_plan, build_plan_for, parse_policy,
    policy_from_env, report_of,
};
pub use self::diagnostics::{
    DaemonDiagnostics, SessionOutcome, append_diagnostics_capabilities, content_safe,
    diagnostic_redactor,
};
pub use self::portal::{PORTAL_EVENT_POLL_INTERVAL, spawn_portal_event_pump};
pub use self::shutdown::{DAEMON_SHUTDOWN_TOTAL, ShutdownBudget, join_thread_bounded};

// Re-exports for thin binary wiring and backward-compatible `daemon_bin::` include path.
// Use relative `self::` paths so both the real binary (`crate::daemon`) and the
// integration-test `daemon_bin::daemon` (via `include!("../src/main.rs")`) resolve.
pub use self::capture::{AudioSourceKind, SAMPLE_RATE_HZ, parse_wav_mono, sine_fixture_samples};
pub use self::policy::{
    ActivationFacts, AsrEngineKind, CLEAN_PIPELINE_ID, CorrectionStrategy, DEFAULT_PIPELINE_ENV,
    DEFAULT_PIPELINE_ID, DeliveryBackendKind, EngineConfig, LINUX_MIC_INBOX_PIPELINE_ID,
    MOCK_TRANSCRIPT, RAW_PIPELINE_ID, builtin_pipeline_defs, collapse_only, default_pipeline_id,
    default_pipeline_id_from, open_journal, open_startup_journal, pipelines_dir, probe_pw_record,
    probe_theme_store_writability, run_capability_check, run_capability_check_with,
    seed_builtin_pipelines,
};
pub use self::retained_history::{
    HISTORY_LIST_LIMIT, HISTORY_NOT_COMPOSED_DETAIL, HISTORY_RETENTION_ENV,
    HISTORY_UNAVAILABLE_DETAIL, MAX_HISTORY_ENTRIES, RecordOutcome, RetainedHistory, entry_detail,
    listing_row, open_retained_history, retained_history_db_path, retained_state_wire,
    retention_from_env,
};
#[cfg(feature = "asr-transcribe-resident")]
pub use self::session::warm_default_pipeline;
pub use self::session::{
    DaemonEvent, DaemonState, DeliveryReport, DeliverySink, EXIT_ALREADY_RUNNING, EXIT_FAILURE,
    PORTAL_PERMISSION_REQUEST_TIMEOUT, PORTAL_REVOKE_TIMEOUT, PUMP_WAKE_FALLBACK, SessionRequest,
    SessionSlot, build_handler, build_handler_with_portal, exit_code, spawn_event_pump, state_wire,
    stop_event_pump,
};

// Keep the `crate::daemon::session::SessionRequest` form available for any
// absolute-path users while still supporting the relative integration-test path.
// These additional aliases are conditional on not breaking the `self::` tests.
#[cfg(not(test))]
pub use crate::daemon::capture::AudioSourceKind as AudioSourceKindAbsolute;
#[cfg(not(test))]
pub use crate::daemon::policy::EngineConfig as EngineConfigAbsolute;
#[cfg(not(test))]
pub use crate::daemon::session::SessionRequest as SessionRequestAbsolute;
