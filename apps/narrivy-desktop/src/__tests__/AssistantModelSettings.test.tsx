// Settings → Extensions → AI assistant: the model it answers with, with
// Load and Unload in view (the Store only lists models).
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { AssistantModelSettings } from '../app/screens/LanguageModels';
import { bridge } from '../lib/bridge';

vi.mock('../ui/primitives/Toast', () => ({ useErrorHandler: () => () => undefined }));

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

describe('assistant model settings', () => {
  it('shows the chosen model and loads or unloads it on request', async () => {
    let state = 'loaded:gpu';
    vi.mocked(bridge.languageModels).mockImplementation(async () => [
      {
        id: 'qwen',
        name: 'Qwen3.5 4B',
        publisher: 'Qwen',
        size_bytes: 1,
        license: 'Apache-2.0',
        roles: ['clean_up', 'assistant'],
        languages: ['en'],
        recommended_for: ['assistant'],
        in_use_for: ['assistant'],
        assistant_state: state,
        installed: true,
      },
    ]);
    vi.mocked(bridge.assistantUnload).mockImplementation(async () => {
      state = 'unloaded';
    });
    render(<AssistantModelSettings onGetModel={() => undefined} />);
    expect(await screen.findByRole('combobox', { name: 'Model' })).toBeInTheDocument();

    fireEvent.click(await screen.findByRole('button', { name: 'Unload model' }));
    expect(bridge.assistantUnload).toHaveBeenCalledTimes(1);

    const load = await screen.findByRole('button', { name: 'Load model' }, { timeout: 2000 });
    fireEvent.click(load);
    expect(bridge.assistantLibrary).toHaveBeenCalledWith('run', 'model');
  });
});
