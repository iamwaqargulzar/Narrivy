import { describe, expect, it } from 'vitest';
import { modelInUse, readVoiceModels } from '../lib/voiceModels';

// The owner could not choose the text-to-speech model voice mode uses.
describe('voice models', () => {
  const settings = {
    englishModel: '',
    urduModel: '',
    models: [
      { id: 'voice-kokoro-v1.0-en', name: 'Kokoro', language: 'en', installed: true },
      { id: 'voice-other-en', name: 'Other', language: 'en', installed: false },
      { id: 'voice-piper-ur-fasih-medium', name: 'Fasih', language: 'ur', installed: true },
      { id: 'junk', language: 'xx' },
    ],
  };

  it('reads the packs the service lists and drops malformed ones', () => {
    const choice = readVoiceModels(settings);
    expect(choice.models.map((m) => m.id)).toEqual(['voice-kokoro-v1.0-en', 'voice-other-en', 'voice-piper-ur-fasih-medium']);
    expect(readVoiceModels(null)).toEqual({ englishModel: '', urduModel: '', models: [] });
  });

  it('shows which pack is read: the chosen one when installed, else the first installed', () => {
    const auto = readVoiceModels(settings);
    expect(modelInUse(auto, 'en')?.id).toBe('voice-kokoro-v1.0-en');
    expect(modelInUse({ ...auto, englishModel: 'voice-other-en' }, 'en')?.id).toBe('voice-kokoro-v1.0-en');
    expect(modelInUse(auto, 'ur')?.id).toBe('voice-piper-ur-fasih-medium');
    expect(modelInUse({ ...auto, urduModel: 'off' }, 'ur')).toBeNull();
  });
});

describe('voice model picker', () => {
  it('offers the installed packs, marks the others, and sends the choice', async () => {
    const { render, screen, fireEvent } = await import('@testing-library/react');
    const { createElement } = await import('react');
    const { VoiceModelPicker } = await import('../app/components/assistant/VoiceModelPicker');
    const changes: unknown[] = [];
    const choice = readVoiceModels({
      englishModel: '',
      urduModel: '',
      models: [
        { id: 'voice-kokoro-v1.0-en', name: 'Kokoro', language: 'en', installed: true },
        { id: 'voice-other-en', name: 'Other', language: 'en', installed: false },
        { id: 'voice-piper-ur-fasih-medium', name: 'Fasih', language: 'ur', installed: true },
      ],
    });
    render(createElement(VoiceModelPicker, { choice, onChange: (c: unknown) => changes.push(c) }));
    const english = screen.getByRole('combobox', { name: 'English model' });
    expect(english).toHaveDisplayValue('Automatic (Kokoro)');
    expect(screen.getByRole('option', { name: 'Other — not installed' })).toBeDisabled();
    fireEvent.change(screen.getByRole('combobox', { name: 'Urdu model' }), { target: { value: 'off' } });
    expect(changes).toEqual([{ urduModel: 'off' }]);
  });
});
