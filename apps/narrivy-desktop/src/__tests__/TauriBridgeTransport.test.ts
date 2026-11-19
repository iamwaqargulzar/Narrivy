import { afterEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();
const listen = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: (...args: unknown[]) => listen(...args) }));

describe('Tauri native bridge transport (ADR-0075)', () => {
  afterEach(() => {
    invoke.mockReset();
    listen.mockReset();
    vi.resetModules();
  });

  it('preserves command names and argument names', async () => {
    invoke.mockResolvedValue({ capabilities: ['mic.available'], active_session: null });
    const { bridge } = await import('../lib/bridge');

    await expect(bridge.status()).resolves.toEqual({
      capabilities: ['mic.available'],
      active_session: null,
    });
    await bridge.start('fast-dictation');
    await bridge.installBundledModel();
    await bridge.cancelModelInstall('op-1');
    await bridge.pipelinesImport('/tmp/p.json', true);

    expect(invoke.mock.calls).toEqual([
      ['daemon_status'],
      ['daemon_start', { pipelineId: 'fast-dictation', recognition: null }],
      ['model_install_bundled'],
      ['model_install_cancel', { operationId: 'op-1' }],
      ['pipelines_import', { path: '/tmp/p.json', dryRun: true }],
    ]);
  });

  it('forwards session events emitted by the Rust shell', async () => {
    const unlisten = vi.fn();
    listen.mockResolvedValue(unlisten);
    const { bridge } = await import('../lib/bridge');
    const handler = vi.fn();

    const stop = await bridge.onSessionEvent(handler);
    expect(listen).toHaveBeenCalledWith('session-event', expect.any(Function));
    const deliver = listen.mock.calls[0][1] as (event: { payload: unknown }) => void;
    deliver({ payload: { event_seq: 4, event: { state: 'listening' } } });
    expect(handler).toHaveBeenCalledWith({ event_seq: 4, event: { state: 'listening' } });
    expect(stop).toBe(unlisten);
  });

  it('reports the dormant portal permission as unavailable instead of pretending', async () => {
    const { bridge } = await import('../lib/bridge');
    await expect(bridge.requestPortalKeyboardPermission()).rejects.toMatchObject({
      code: 'unavailable',
    });
    expect(invoke).not.toHaveBeenCalled();
  });
});
