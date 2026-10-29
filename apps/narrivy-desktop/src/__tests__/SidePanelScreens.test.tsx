// Models and Meetings keep their sections in a side panel, like Settings.
// Models: speech to text, correction, language models and text to speech.
// Meetings: "New meeting", a search and the list; the recorder or the open
// meeting on the right.
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import App from '../App';
import { bridge, type LanguageModel, type Meeting, type MeetingDetail } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const lm = (id: string, name: string, roles: LanguageModel['roles'], inUse: LanguageModel['roles'] = []): LanguageModel =>
  ({
    id,
    name,
    roles,
    in_use_for: inUse,
    recommended_for: [],
    languages: ['en'],
    publisher: 'Example',
    license: 'Apache-2.0',
    size_bytes: 1_000_000_000,
  }) as LanguageModel;

const meeting = (id: string, title: string): Meeting =>
  ({
    id,
    title,
    started_unix_ms: Date.now(),
    duration_ms: 60_000,
    state: 'ready',
    progress: 1,
    model: 'turbo',
    speakers: 2,
  }) as Meeting;

async function open(nav: string) {
  window.localStorage.setItem('narrivy.onboarded.v1', '1');
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: nav }));
  await screen.findByRole('heading', { level: 1, name: nav });
}

describe('Models side panel', () => {
  it('shows each kind of model in its own section', async () => {
    vi.mocked(bridge.languageModels).mockResolvedValue([
      lm('small', 'Small clean-up model', ['clean_up'], ['clean_up']),
      lm('big', 'Big chat model', ['assistant']),
    ]);
    await open('Models');
    const kinds = screen.getByRole('tablist', { name: 'Kind of model' });
    expect(within(kinds).getAllByRole('tab').map((tab) => tab.textContent)).toEqual([
      'Speech to text',
      'Correction',
      'Language models',
      'Text to speech',
    ]);

    fireEvent.click(within(kinds).getByRole('tab', { name: 'Correction' }));
    expect(await screen.findByRole('heading', { name: 'Clean-up' })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Assistant' })).toBeNull();
    expect(screen.getByTestId('lm-small')).toBeInTheDocument();
    expect(screen.queryByTestId('lm-big')).toBeNull();

    fireEvent.click(within(kinds).getByRole('tab', { name: 'Language models' }));
    expect(await screen.findByRole('heading', { name: 'Assistant' })).toBeInTheDocument();
    expect(screen.getByTestId('lm-big')).toBeInTheDocument();

    fireEvent.click(within(kinds).getByRole('tab', { name: 'Text to speech' }));
    const kokoro = await screen.findByTestId('tts-kokoro');
    expect(within(kokoro).getByText('Ready')).toBeInTheDocument();
    expect(within(kokoro).getByText('Speaks with Heart (US, female)')).toBeInTheDocument();
    expect(within(screen.getByTestId('tts-piper-urdu')).getByText('Speaks with Fasih (Pakistan, male)')).toBeInTheDocument();
  });
});

describe('Meetings side panel', () => {
  it('lists meetings beside the open one, searches them, and starts a new one', async () => {
    vi.mocked(bridge.meetings).mockResolvedValue([meeting('a', 'Weekly sync'), meeting('b', 'Design review')]);
    vi.mocked(bridge.meeting).mockImplementation(async (id: string) => ({
      meeting: meeting(id, id === 'a' ? 'Weekly sync' : 'Design review'),
      turns: [],
      names: {},
      models: [],
    }) as unknown as MeetingDetail);
    await open('Meetings');
    const list = screen.getByRole('navigation', { name: 'Recorded meetings' });
    // The first meeting opens; the recorder waits behind "New meeting".
    expect(await screen.findByRole('heading', { name: 'Weekly sync' })).toBeInTheDocument();
    expect(screen.queryByRole('textbox', { name: 'Meeting name' })).toBeNull();

    fireEvent.change(screen.getByRole('searchbox', { name: 'Search meetings' }), { target: { value: 'design' } });
    expect(within(list).queryByText('Weekly sync')).toBeNull();
    fireEvent.click(within(list).getByText('Design review'));
    expect(await screen.findByRole('heading', { name: 'Design review' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'New meeting' }));
    expect(screen.getByRole('textbox', { name: 'Meeting name' })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Design review' })).toBeNull();
  });

  it('transcribes a recording chosen with the file chooser', async () => {
    vi.mocked(bridge.meetings).mockResolvedValue([]);
    vi.mocked(bridge.pickRecording).mockResolvedValue('/home/you/Videos/standup.mp4');
    vi.mocked(bridge.meetingImport).mockResolvedValue(meeting('c', 'standup'));
    await open('Meetings');
    fireEvent.click(await screen.findByRole('button', { name: 'Transcribe a file…' }));
    await waitFor(() => expect(bridge.meetingImport).toHaveBeenCalledWith('/home/you/Videos/standup.mp4'));
  });
});
