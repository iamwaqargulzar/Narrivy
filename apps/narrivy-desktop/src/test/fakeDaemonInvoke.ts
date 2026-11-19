// A fake of the Tauri `invoke` transport that behaves like the daemon for the
// few commands a test drives, so the test goes through the real TauriBridge
// (command names, argument names, response shapes) instead of a mocked bridge
// that returns whatever the test wants (F-M16).
//
// Responses use the shapes the Rust shell serialises (`src-tauri/src/bridge.rs`
// and `narrivy-ipc` DTOs). Commands the fake does not model reject with
// `unavailable`, as the real bridge does when the daemon lacks a feature, so a
// test can never pass by reaching an unmodelled command that "succeeds".

type Handler = (args: Record<string, unknown> | undefined) => unknown;

export interface ProvisionDto {
  operation_id: string;
  state: string;
  publication: string;
  cleanup: string;
  failure?: string;
  revision: number;
}

/** How the daemon's import of the packaged model ends (the daemon decides,
 * from the source file's owner, mode and digest; see `model_import.rs`). */
export type BundledOutcome = 'ready' | 'source_rejected';

export interface FakeDaemon {
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
  /** Every command the UI sent, in order, with its arguments. */
  calls: Array<[string, Record<string, unknown> | undefined]>;
  /** Override or add one command. */
  on: (cmd: string, handler: Handler) => void;
}

const LIST_COMMANDS = [
  'daemon_list_pipelines',
  'daemon_history',
  'daemon_models',
  'daemon_store_list',
  'daemon_language_models',
  'daemon_meetings',
];

export function createFakeDaemon(opts: { bundled?: BundledOutcome } = {}): FakeDaemon {
  const bundled = opts.bundled ?? 'ready';
  let installedModel = 'awaiting_model';
  let provision: ProvisionDto | null = null;
  // Each status read while the import runs moves it one step, as the
  // daemon's worker does: importing -> activating -> ready.
  const advance = () => {
    if (!provision) return;
    if (provision.state === 'importing') {
      provision = { ...provision, state: 'activating', revision: provision.revision + 1 };
      installedModel = 'provisioning';
    } else if (provision.state === 'activating') {
      provision = {
        ...provision,
        state: 'ready',
        publication: 'verified_installed',
        cleanup: 'confirmed',
        revision: provision.revision + 1,
      };
      installedModel = 'ready';
    }
  };

  const handlers = new Map<string, Handler>();
  handlers.set('daemon_status', () => {
    advance();
    return {
      capabilities: [],
      active_session: null,
      installed_model: installedModel,
      ...(provision ? { model_provisioning: { ...provision } } : {}),
    };
  });
  handlers.set('model_install_bundled', () => {
    if (provision && ['importing', 'activating'].includes(provision.state)) {
      return Promise.reject({ code: 'busy', message: 'a model install is already running' });
    }
    const revision = (provision?.revision ?? 0) + 1;
    provision =
      bundled === 'source_rejected'
        ? {
            operation_id: 'op-bundled',
            state: 'failed',
            publication: 'not_published',
            cleanup: 'not_required',
            failure: 'source_rejected',
            revision,
          }
        : {
            operation_id: 'op-bundled',
            state: 'importing',
            publication: 'not_published',
            cleanup: 'not_required',
            revision,
          };
    installedModel = provision.state === 'failed' ? 'awaiting_model' : 'provisioning';
    return { ...provision };
  });
  for (const cmd of LIST_COMMANDS) handlers.set(cmd, () => []);

  const calls: FakeDaemon['calls'] = [];
  return {
    calls,
    on: (cmd, handler) => handlers.set(cmd, handler),
    invoke: async (cmd, args) => {
      calls.push([cmd, args]);
      const handler = handlers.get(cmd);
      if (!handler) {
        throw { code: 'unavailable', message: `${cmd} is not modelled by the fake daemon` };
      }
      return handler(args);
    },
  };
}
