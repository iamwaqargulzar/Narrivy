// Builds a full mock of src/lib/bridge for vi.mock factories.
// Deliberately does NOT import the real bridge module.
import { vi } from "vitest";
import type { SessionEventHandler } from "../lib/bridge";

export const DEFAULT_CAPABILITIES = [
  "mic.available",
  "asr.transcribe-cpp",
  "mock.asr",
  "delivery.clipboard",
  "history.optional=disabled:not_composed",
];

export function createMockBridgeModule() {
  // What the running mock meeting records, switched by meetingSources.
  const liveSources = { screen: true, sound: true, microphone: false };
  return {
    SESSION_EVENT: "session-event",
    // Mirrors the real normalization so components get typed errors in tests.
    toBridgeError: (raw: unknown) => {
      if (typeof raw === "string") {
        return raw === "daemon_unavailable"
          ? { code: "daemon_unavailable", message: raw }
          : { code: "unknown", message: raw };
      }
      if (raw && typeof raw === "object") {
        const o = raw as Record<string, unknown>;
        return {
          code: typeof o.code === "string" ? o.code : "unknown",
          message: typeof o.message === "string" ? o.message : String(raw),
        };
      }
      return { code: "unknown", message: String(raw) };
    },
    FakeBridge: class FakeBridgeMock {},
    bridge: {
      status: vi.fn(async () => ({
        capabilities: [...DEFAULT_CAPABILITIES],
        active_session: { state: "idle", pipeline_id: null },
        daemon_build: "narrivyd 0.3.0-dev",
        installed_model: "ready",
        portal_keyboard: {
          eligibility: "ready_to_request",
          lifecycle: "idle",
          can_request: true,
          permission_active: false,
          transport_connected: false,
          sender_available: false,
          delivery_qualified: false,
          cleanup: "not_required",
        },
      })),
      start: vi.fn(
        async (_pipelineId?: string | null, _recognition?: unknown) => {},
      ),
      stop: vi.fn(async () => {}),
      cancel: vi.fn(async () => {}),
      listPipelines: vi.fn(async () => [
        {
          id: "mock-dictation",
          name: "Test lane",
          version: 1,
          enabled: true,
        },
      ]),
      history: vi.fn(async () => []),
      historyEntry: vi.fn(async (txid: string) => ({
        txid,
        text: "full text",
      })),
      deleteHistoryEntry: vi.fn(async (_txid: string) => undefined),
      models: vi.fn(async () => []),
      selectModel: vi.fn(async (_id: string) => {}),
      downloadModel: vi.fn(async (_id: string) => {}),
      cancelModelDownload: vi.fn(async (_id: string) => {}),
      storeItems: vi.fn(async () => []),
      removeStoreItem: vi.fn(async (_id: string) => {}),
      assistantSend: vi.fn(async (_turnId: string, _messages: unknown[], _options?: unknown) => {}),
      assistantCancel: vi.fn(async (_turnId: string) => {}),
      assistantUnload: vi.fn(async () => {}),
      pickFolder: vi.fn(async () => '/home/you/Projects/site'),
      assistantLibrary: vi.fn(async (op: string, kind: string): Promise<string> =>
        kind === "voice"
          ? JSON.stringify({ speedPercent: 100, pushToTalk: false, interrupt: "stop", sounds: true, voice: 3, running: false, available: true })
          : op === "list"
            ? "[]"
            : "{}",
      ),
      voiceMode: vi.fn(async () => {}),
      assistantToolDecision: vi.fn(async () => {}),
      audioOptions: vi.fn(async () => ({
        echo_cancel: true,
        echo_cancel_active: true,
        noise_suppression: "standard",
      })),
      setOnlyMyVoice: vi.fn(async (on: boolean) => ({
        echo_cancel: true,
        echo_cancel_active: true,
        only_my_voice: on,
      })),
      trainVoice: vi.fn(async () => ({
        echo_cancel: true,
        echo_cancel_active: true,
        voice_training: "recording",
      })),
      forgetVoice: vi.fn(async () => ({
        echo_cancel: true,
        echo_cancel_active: true,
        voice_trained: false,
      })),
      setNoiseSuppression: vi.fn(async (level: string) => ({
        echo_cancel: true,
        echo_cancel_active: true,
        noise_suppression: level,
      })),
      vocabulary: vi.fn(async () => ({ words: [], replacements: [] })),
      setVocabulary: vi.fn(
        async (v: {
          words: string[];
          replacements: Array<{ from: string; to: string }>;
        }) => v,
      ),
      autostart: vi.fn(async () => ({
        enabled: false,
        hidden: false,
        service_installed: true,
      })),
      setAutostart: vi.fn(async (enabled: boolean, hidden: boolean) => ({
        enabled,
        hidden,
        service_installed: true,
      })),
      setMicrophone: vi.fn(async (name: string) => ({
        echo_cancel: true,
        echo_cancel_active: true,
        microphone: name,
        microphones: [],
      })),
      holdKey: vi.fn(async () => ({ key: null, state: "off", keys: [] })),
      setHoldKey: vi.fn(async (key: number | null) => ({
        key,
        state: key === null ? "off" : "reading",
        keys: [],
      })),
      setSounds: vi.fn(async (on: boolean) => ({
        echo_cancel: true,
        echo_cancel_active: true,
        sounds: on,
      })),
      setHistoryRetention: vi.fn(async (_retention: string) => {}),
      clearHistory: vi.fn(async () => {}),
      textOptions: vi.fn(async () => ({
        numbers_as_digits: true,
        delivery_method: "paste",
      })),
      setNumbersAsDigits: vi.fn(async (on: boolean) => ({
        numbers_as_digits: on,
      })),
      setPipelineVoice: vi.fn(async () => ({
        echo_cancel: true,
        echo_cancel_active: true,
        pipelines: {},
      })),
      onFileDrop: vi.fn(async () => () => {}),
      meetingImport: vi.fn(async (path: string) => ({
        id: "m1",
        title: path,
        started_unix_ms: 0,
        duration_ms: 0,
        state: "processing",
        progress: 0,
        stage: "import",
        model: "",
        speakers: 0,
      })),
      setDeliveryMethod: vi.fn(async (method: string) => ({
        numbers_as_digits: true,
        delivery_method: method,
      })),
      meetingOptions: vi.fn(async () => ({ auto_notes: false })),
      setAutoNotes: vi.fn(async (on: boolean) => ({ auto_notes: on })),
      setProcessOnStop: vi.fn(async (on: boolean) => ({ auto_notes: false, process_on_stop: on })),
      setMeetingDevices: vi.fn(async () => ({ auto_notes: false })),
      setMeetingScreen: vi.fn(async (on: boolean) => ({ auto_notes: false, screen: on, screen_available: true })),
      meetingSlide: vi.fn(async () => ""),
      meetingControl: vi.fn(async () => undefined),
      meetingStop: vi.fn(async () => undefined),
      meetings: vi.fn(async () => []),
      meeting: vi.fn(async (id: string) => ({
        meeting: {
          id,
          title: 'Screen',
          started_unix_ms: Date.now(),
          duration_ms: 0,
          state: 'recording',
          progress: 0,
          model: 'large-v3',
          speakers: 0,
          screen_on: liveSources.screen,
          sound_on: liveSources.sound,
          mic_on: liveSources.microphone,
        },
        turns: [],
        names: {},
        models: [],
      })),
      meetingSources: vi.fn(async (id: string, change: { screen?: boolean; sound?: boolean; microphone?: boolean }) => {
        Object.assign(liveSources, change);
        return {
        id,
        title: 'Screen',
        started_unix_ms: Date.now(),
        duration_ms: 0,
        state: 'recording',
        progress: 0,
        model: 'large-v3',
        speakers: 0,
        screen_on: liveSources.screen,
        sound_on: liveSources.sound,
        mic_on: liveSources.microphone,
      };
      }),
      screenStart: vi.fn(async () => ({
        id: 'm-screen',
        title: 'Screen',
        started_unix_ms: Date.now(),
        duration_ms: 0,
        state: 'recording',
        progress: 0,
        model: 'large-v3',
        speakers: 0,
        watch: true,
      })),
      setCleanupIdle: vi.fn(async (minutes: number) => ({
        numbers_as_digits: true,
        cleanup_idle_minutes: minutes,
      })),
      setVoiceStrictness: vi.fn(async (level: string) => ({
        echo_cancel: true,
        echo_cancel_active: true,
        voice_strictness: level,
      })),
      setSpokenBreaks: vi.fn(async (on: boolean) => ({
        numbers_as_digits: true,
        spoken_breaks: on,
      })),
      setEchoCancel: vi.fn(async (on: boolean) => ({
        echo_cancel: on,
        echo_cancel_active: on,
      })),
      languageModels: vi.fn(async () => []),
      selectLanguageModel: vi.fn(
        async (_role: string, _id: string | null) => {},
      ),
      setPipelineEngine: vi.fn(
        async (
          _pipelineId: string,
          _modelId: string | null,
          _compute: string,
        ) => {},
      ),
      setOverlayAppearance: vi.fn(async () => {}),
      shortcuts: vi.fn(async () => ({
        support: "managed",
        manual_command: "/usr/bin/narrivy-cli toggle",
        bindings: [],
      })),
      setShortcut: vi.fn(async () => ({ outcome: "created" })),
      removeShortcut: vi.fn(async () => undefined),
      recoveryEntry: vi.fn(async (txid: string) => ({
        txid,
        text: "full text",
      })),
      capabilityCheck: vi.fn(async (id: string) => ({
        id,
        pass: true,
        detail: "ok",
      })),
      pipelinesExport: vi.fn(async (path: string) => ({ count: 2, path })),
      pipelinesImport: vi.fn(async (_path: string, dryRun: boolean) => ({
        dry_run: dryRun,
        imported: 0,
        skipped_duplicates: 0,
        required_dependencies: ["narrivy:test-module@1"],
      })),
      requestPortalKeyboardPermission: vi.fn(async () => ({
        outcome: "permission_active",
      })),
      cancelPortalKeyboardPermission: vi.fn(async () => {}),
      installBundledModel: vi.fn(async () => ({
        operation_id: "fake-model-operation",
        state: "ready",
        publication: "verified_installed",
        cleanup: "confirmed",
        revision: 1,
      })),
      cancelModelInstall: vi.fn(async (operationId: string) => ({
        operation_id: operationId,
        state: "cancel_requested",
        publication: "not_published",
        cleanup: "not_required",
        revision: 2,
      })),
      onSessionEvent: vi.fn(async (_handler: SessionEventHandler) => () => {}),
    },
  };
}
