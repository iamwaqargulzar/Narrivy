import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import App from '../App';
import { bridge, type SessionEvent } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

describe('AssistantScreen', () => {
  it('sends the conversation and streams the reply in, with a stop button meanwhile', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    const handlers: ((event: SessionEvent) => void)[] = [];
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      handlers.push(handler);
      return () => {};
    });
    const emit = (event: SessionEvent) => handlers.forEach((handler) => handler(event));

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    const input = await screen.findByRole('textbox', { name: 'Message to the assistant' });
    fireEvent.change(input, { target: { value: 'what is the capital of france' } });
    fireEvent.keyDown(input, { key: 'Enter' });

    await waitFor(() => expect(bridge.assistantSend).toHaveBeenCalledTimes(1));
    const [turnId, messages, options] = vi.mocked(bridge.assistantSend).mock.calls[0];
    expect(messages).toEqual([{ role: 'user', content: 'what is the capital of france' }]);
    // The service keeps the chat: a new one is saved before its first question.
    const chatId = (options as { chatId: string }).chatId;
    expect(chatId).toMatch(/^chat-/);
    expect(bridge.assistantLibrary).toHaveBeenCalledWith('put', 'chat', null, expect.stringContaining(chatId));
    expect(screen.getByRole('button', { name: 'Stop' })).toBeInTheDocument();

    act(() => emit({ event: 'assistant_delta', event_seq: 1, turn_id: turnId, text: 'The capital is ' }));
    act(() => emit({ event: 'assistant_delta', event_seq: 2, turn_id: turnId, text: '**Paris**.' }));
    expect(await screen.findByText('Paris')).toBeInTheDocument();
    act(() =>
      emit({ event: 'assistant_done', event_seq: 3, turn_id: turnId, outcome: 'ok', text: 'The capital is **Paris**.' }),
    );
    await waitFor(() => expect(screen.getByRole('button', { name: 'Send' })).toBeInTheDocument());

    // The next question goes into the same chat; the service adds the history.
    fireEvent.change(input, { target: { value: 'and of italy' } });
    fireEvent.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() => expect(bridge.assistantSend).toHaveBeenCalledTimes(2));
    const [, nextMessages, nextOptions] = vi.mocked(bridge.assistantSend).mock.calls[1];
    expect(nextMessages).toEqual([{ role: 'user', content: 'and of italy' }]);
    expect(nextOptions).toEqual({ chatId, speak: false, pictures: [] });
  });

  it('explains when no assistant model is set up', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.assistantSend).mockRejectedValueOnce({ code: 'assistant_not_configured' });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    const input = await screen.findByRole('textbox', { name: 'Message to the assistant' });
    fireEvent.change(input, { target: { value: 'hello' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(await screen.findByText(/No assistant model is set up yet/)).toBeInTheDocument();
  });

  it('keeps the conversation when switching tabs, and can unload the model', async () => {
    window.localStorage.clear();
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.languageModels).mockResolvedValue([
      {
        id: 'assistant-4b',
        name: 'Assistant model',
        publisher: '',
        size_bytes: 1,
        license: 'Apache-2.0',
        roles: ['assistant'],
        languages: ['en'],
        recommended_for: [],
        in_use_for: ['assistant'],
        assistant_state: 'loaded:gpu',
      },
    ]);
    const handlers: ((event: SessionEvent) => void)[] = [];
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      handlers.push(handler);
      return () => {};
    });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    const input = await screen.findByRole('textbox', { name: 'Message to the assistant' });
    fireEvent.change(input, { target: { value: 'remember this question' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(bridge.assistantSend).toHaveBeenCalled());

    fireEvent.click(screen.getByRole('button', { name: 'History' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    expect((await screen.findAllByText('remember this question')).length).toBeGreaterThan(0);

    // Unloading waits for the reply to end.
    const turnId = vi.mocked(bridge.assistantSend).mock.calls.at(-1)?.[0] ?? '';
    act(() =>
      handlers.forEach((handler) =>
        handler({ event: 'assistant_done', event_seq: 9, turn_id: turnId, outcome: 'ok', text: 'Noted.' }),
      ),
    );
    expect(await screen.findByText('Model loaded on GPU')).toBeInTheDocument();
    // Load, Unload and "Unload when idle" sit behind the model's chip.
    fireEvent.click(screen.getByText('Model loaded on GPU'));
    expect(screen.getByRole('switch', { name: 'Unload when idle' })).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('button', { name: 'Unload model' })).not.toBeDisabled());
    fireEvent.click(screen.getByRole('button', { name: 'Unload model' }));
    await waitFor(() => expect(bridge.assistantUnload).toHaveBeenCalledTimes(1));
  });

  it('moves chats kept by an older app into the service once', async () => {
    window.localStorage.clear();
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    const old = [{ id: 'chat-old', title: 'Old', updatedAt: 1, turns: [] }];
    window.localStorage.setItem('narrivy.assistant.chats.v1', JSON.stringify(old));
    render(<App />);
    await waitFor(() =>
      expect(bridge.assistantLibrary).toHaveBeenCalledWith('import', 'chat', null, JSON.stringify(old)),
    );
    await waitFor(() => expect(window.localStorage.getItem('narrivy.assistant.chats.v1')).toBeNull());
  });

  it('asks what voice mode hears, reads it aloud, and stops the reply when talked over', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    const handlers: ((event: SessionEvent) => void)[] = [];
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      handlers.push(handler);
      return () => {};
    });
    const emit = (event: SessionEvent) => act(() => handlers.forEach((handler) => handler(event)));
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    const talk = await screen.findByRole('button', { name: 'Talk with the assistant' });
    await waitFor(() => expect(talk).not.toBeDisabled());
    fireEvent.click(talk);
    await waitFor(() => expect(bridge.voiceMode).toHaveBeenCalledWith('start'));

    emit({ event: 'voice', kind: 'state', state: 'listening' } as SessionEvent);
    expect(await screen.findByText('Listening')).toBeInTheDocument();
    emit({ event: 'voice', kind: 'partial', text: 'what time' } as SessionEvent);
    expect(await screen.findByText('what time')).toBeInTheDocument();
    emit({ event: 'voice', kind: 'heard', text: 'what time is it' } as SessionEvent);
    await waitFor(() => expect(bridge.assistantSend).toHaveBeenCalled());
    const [turnId, messages, options] = vi.mocked(bridge.assistantSend).mock.calls.at(-1) ?? [];
    expect(messages).toEqual([{ role: 'user', content: 'what time is it' }]);
    expect(options).toMatchObject({ speak: true });

    emit({ event: 'voice', kind: 'interrupt' } as SessionEvent);
    await waitFor(() => expect(bridge.assistantCancel).toHaveBeenCalledWith(turnId));
    emit({ event: 'voice', kind: 'timing', timing: { asr_ms: 300, reply_ms: 700, voice_ms: 400, total_ms: 1500 } } as SessionEvent);
    expect(await screen.findByText(/last answer in 1.5 s/)).toBeInTheDocument();
    // The settings show both voices, and where the time went.
    fireEvent.click(screen.getByRole('button', { name: 'Voice settings' }));
    expect(screen.getByText('Fasih (Pakistan, male)')).toBeInTheDocument();
    expect(screen.getByText(/first sound after 1.5 s/)).toBeInTheDocument();

    // Turning it off keeps the voice models loaded; unloading is separate.
    fireEvent.click(screen.getByRole('button', { name: 'Turn off' }));
    await waitFor(() => expect(bridge.voiceMode).toHaveBeenCalledWith('settings', { muted: true }));
    expect(bridge.voiceMode).not.toHaveBeenCalledWith('stop');
    expect(screen.getByRole('switch', { name: 'Voice models' })).toHaveAttribute('aria-checked', 'true');
    fireEvent.click(screen.getByRole('switch', { name: 'Voice models' }));
    await waitFor(() => expect(bridge.voiceMode).toHaveBeenCalledWith('stop'));
  });

  it('steers a reply: typing while it is written stops it and asks the instruction next', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    const handlers: ((event: SessionEvent) => void)[] = [];
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      handlers.push(handler);
      return () => {};
    });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    const input = await screen.findByRole('textbox', { name: 'Message to the assistant' });
    fireEvent.change(input, { target: { value: 'write a long poem' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(bridge.assistantSend).toHaveBeenCalled());
    const calls = vi.mocked(bridge.assistantSend).mock.calls.length;
    const turnId = vi.mocked(bridge.assistantSend).mock.calls.at(-1)?.[0] ?? '';

    fireEvent.change(input, { target: { value: 'make it short' } });
    fireEvent.click(screen.getByRole('button', { name: 'Steer' }));
    await waitFor(() => expect(bridge.assistantCancel).toHaveBeenCalledWith(turnId));
    act(() =>
      handlers.forEach((handler) =>
        handler({ event: 'assistant_done', event_seq: 4, turn_id: turnId, outcome: 'cancelled', text: 'Roses' }),
      ),
    );
    await waitFor(() => expect(vi.mocked(bridge.assistantSend).mock.calls.length).toBe(calls + 1));
    expect(vi.mocked(bridge.assistantSend).mock.calls.at(-1)?.[1]).toEqual([{ role: 'user', content: 'make it short' }]);
  });

  it('adds a persona in the library', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Library' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Personas' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Add a persona' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Name' }), { target: { value: 'Urdu tutor' } });
    fireEvent.change(screen.getByRole('textbox', { name: 'Instructions' }), { target: { value: 'Teach me Urdu.' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() =>
      expect(bridge.assistantLibrary).toHaveBeenCalledWith('put', 'persona', null, expect.stringContaining('Urdu tutor')),
    );
  });

  it('shows a tool call as a card, asks before it runs, and hides the raw call', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    const handlers: ((event: SessionEvent) => void)[] = [];
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      handlers.push(handler);
      return () => {};
    });
    const emit = (event: SessionEvent) => act(() => handlers.forEach((handler) => handler(event)));
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    const input = await screen.findByRole('textbox', { name: 'Message to the assistant' });
    fireEvent.change(input, { target: { value: 'how many chairs are in stock' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(bridge.assistantSend).toHaveBeenCalled());
    const turnId = vi.mocked(bridge.assistantSend).mock.calls.at(-1)?.[0] ?? '';

    emit({ event: 'assistant_delta', turn_id: turnId, text: 'Checking.\n<tool_call>\n<function=stock>' } as SessionEvent);
    // In the reply while it streams, and in Activity as what it thought.
    expect((await screen.findAllByText('Checking.')).length).toBe(2);
    expect(await screen.findByTestId('activity-thinking')).toHaveTextContent('Checking.');
    expect(screen.queryByText(/function=stock/)).not.toBeInTheDocument();
    // The call as it is written, before it runs.
    emit({ event: 'assistant_delta', turn_id: turnId, text: '>\n<parameter=item>\nchai' } as SessionEvent);
    await waitFor(() => expect(screen.getByTestId('activity-calling')).toHaveTextContent('"item": "chai"'));
    emit({
      event: 'assistant_tool',
      turn_id: turnId,
      call_id: 'call-1',
      server: 'Warehouse',
      tool: 'stock',
      arguments: '{"item":"chairs"}',
      state: 'ask',
    } as SessionEvent);
    fireEvent.click(await screen.findByRole('button', { name: 'Always in this chat' }));
    await waitFor(() => expect(bridge.assistantToolDecision).toHaveBeenCalledWith('call-1', true, true));
    emit({
      event: 'assistant_tool',
      turn_id: turnId,
      call_id: 'call-1',
      server: 'Warehouse',
      tool: 'stock',
      arguments: '{"item":"chairs"}',
      state: 'done',
      summary: '42 units',
    } as SessionEvent);
    // On the card, and in the Activity panel beside the chat.
    expect((await screen.findAllByText('42 units')).length).toBeGreaterThan(0);
    expect(screen.getAllByText('Done').length).toBeGreaterThan(0);
    expect(screen.getByRole('complementary', { name: 'Activity' })).toBeInTheDocument();
    // The call itself, as the tool receives it.
    expect(screen.getAllByText(/"tool": "Warehouse\/stock"/).length).toBeGreaterThan(0);
  });

  it('turns a closing list of links into numbered sources, cited in the text', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    const handlers: ((event: SessionEvent) => void)[] = [];
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      handlers.push(handler);
      return () => {};
    });
    const emit = (event: SessionEvent) => act(() => handlers.forEach((handler) => handler(event)));
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    const input = await screen.findByRole('textbox', { name: 'Message to the assistant' });
    fireEvent.change(input, { target: { value: 'how much sram has the esp32-s3' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(bridge.assistantSend).toHaveBeenCalled());
    const turnId = vi.mocked(bridge.assistantSend).mock.calls.at(-1)?.[0] ?? '';
    const text =
      'It has 512 KB of SRAM, says [the datasheet](https://www.espressif.com/s3.pdf).\n\n' +
      'Sources:\n- [Datasheet](https://www.espressif.com/s3.pdf)\n- [Docs](https://docs.espressif.com/s3)';
    emit({ event: 'assistant_done', turn_id: turnId, outcome: 'ok', text } as SessionEvent);
    const sources = await screen.findByRole('list', { name: 'Sources' });
    expect(sources).toHaveTextContent('1espressif.com');
    expect(sources).toHaveTextContent('2docs.espressif.com');
    expect(screen.queryByText('Sources:')).not.toBeInTheDocument();
    expect(screen.getByText('the datasheet')).toHaveTextContent('the datasheet1');
  });

  it('switches web research on for a chat from the composer', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    const web = await screen.findByRole('switch', { name: 'Web' });
    expect(web).toHaveAttribute('aria-checked', 'false');
    fireEvent.click(web);
    expect(await screen.findByRole('switch', { name: 'Web' })).toHaveAttribute('aria-checked', 'true');
    // How much the computer may do is next to its button.
    fireEvent.click(screen.getByRole('button', { name: 'Can use' }));
    expect(screen.getByRole('radio', { name: /Ask first/ })).toHaveAttribute('aria-checked', 'true');
  });

  it('shows in Activity what a reply is doing, without thinking or tools', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    const handlers: ((event: SessionEvent) => void)[] = [];
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      handlers.push(handler);
      return () => {};
    });
    const emit = (event: SessionEvent) => act(() => handlers.forEach((handler) => handler(event)));
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    const input = await screen.findByRole('textbox', { name: 'Message to the assistant' });
    fireEvent.change(input, { target: { value: 'say hello' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(bridge.assistantSend).toHaveBeenCalled());
    const turnId = vi.mocked(bridge.assistantSend).mock.calls.at(-1)?.[0] ?? '';
    const progress = await screen.findByTestId('activity-progress');
    expect(progress).toHaveTextContent('Reading the question');
    emit({ event: 'assistant_delta', turn_id: turnId, text: 'Hello there, friend' } as SessionEvent);
    await waitFor(() => expect(screen.getByTestId('activity-progress')).toHaveTextContent('Writing the answer · 3 words'));
    expect(screen.getByTestId('activity-progress')).toHaveTextContent('Read the question');
    emit({ event: 'assistant_done', turn_id: turnId, outcome: 'ok', text: 'Hello there, friend' } as SessionEvent);
    await waitFor(() => expect(screen.getByTestId('activity-progress')).toHaveTextContent('Answered · 3 words'));
  });

  it('sets how much the chat thinks and shows the thinking in Activity', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    const handlers: ((event: SessionEvent) => void)[] = [];
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      handlers.push(handler);
      return () => {};
    });
    const emit = (event: SessionEvent) => act(() => handlers.forEach((handler) => handler(event)));
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Thinking' }));
    fireEvent.click(await screen.findByRole('option', { name: /Think deeply/ }));
    expect(screen.getByRole('button', { name: 'Thinking' })).toHaveTextContent('Think deeply');
    const input = await screen.findByRole('textbox', { name: 'Message to the assistant' });
    fireEvent.change(input, { target: { value: 'why is the sky blue' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() =>
      expect(bridge.assistantLibrary).toHaveBeenCalledWith('put', 'chat', null, expect.stringContaining('"thinking":"deep"')),
    );
    await waitFor(() => expect(bridge.assistantSend).toHaveBeenCalled());
    const turnId = vi.mocked(bridge.assistantSend).mock.calls.at(-1)?.[0] ?? '';
    emit({ event: 'assistant_delta', turn_id: turnId, text: '<think>\nLight scatters by wavelength' } as SessionEvent);
    expect(await screen.findByTestId('activity-thinking')).toHaveTextContent('Light scatters by wavelength');
    emit({ event: 'assistant_delta', turn_id: turnId, text: '</think>\n\nBecause of Rayleigh scattering.' } as SessionEvent);
    expect(await screen.findByText('Because of Rayleigh scattering.')).toBeInTheDocument();
    expect(screen.queryAllByText(/Light scatters/)).toHaveLength(1);
  });

  it('starts a live transcript from the composer and shows it beside the chat', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    fireEvent.click(await screen.findByRole('switch', { name: 'Sound' }));
    await waitFor(() =>
      expect(bridge.screenStart).toHaveBeenCalledWith(expect.stringMatching(/^Live transcript/), {
        screen: false,
        share: true,
        microphone: false,
        computerSound: true,
      }),
    );
    expect(await screen.findByTestId('live-transcript')).toBeInTheDocument();
  });

  it('stops a share left running without its chat from the live panel', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    // Recording since before the app opened, its chat gone.
    vi.mocked(bridge.meetings).mockResolvedValue([
      {
        id: 'm-old',
        title: 'Screen share',
        started_unix_ms: Date.now() - 600_000,
        duration_ms: 0,
        state: 'recording',
        progress: 0,
        model: 'large-v3',
        speakers: 0,
        share: true,
        sound_on: true,
      },
    ]);
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    const panel = await screen.findByRole('region', { name: 'Live' });
    fireEvent.click(within(panel).getByRole('button', { name: 'Stop' }));
    await waitFor(() => expect(bridge.meetingStop).toHaveBeenCalled());
    vi.mocked(bridge.meetings).mockResolvedValue([]);
  });

  it('shares the screen live with the open chat, and stops seeing it when sharing stops', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    fireEvent.click(await screen.findByRole('switch', { name: 'Screen' }));
    await waitFor(() =>
      expect(bridge.screenStart).toHaveBeenCalledWith(expect.stringMatching(/^Screen share/), {
        screen: true,
        share: true,
        microphone: false,
        computerSound: false,
      }),
    );
    // The chat is linked to the share, so its questions carry the screen.
    await waitFor(() =>
      expect(bridge.assistantLibrary).toHaveBeenCalledWith('put', 'chat', null, expect.stringContaining('"live":"m-screen"')),
    );
    // Talking first, sharing the screen later: each can be switched alone.
    fireEvent.click(await screen.findByRole('switch', { name: 'Sound' }));
    await waitFor(() => expect(bridge.meetingSources).toHaveBeenCalledWith('m-screen', { sound: false }));
    expect(await screen.findByRole('switch', { name: 'Sound' })).toHaveAttribute('aria-checked', 'false');
    expect(screen.getByRole('switch', { name: 'Screen' })).toHaveAttribute('aria-checked', 'true');
    // The microphone is switched apart from the computer's sound.
    fireEvent.click(screen.getByRole('switch', { name: 'Mic' }));
    await waitFor(() => expect(bridge.meetingSources).toHaveBeenCalledWith('m-screen', { microphone: true }));
    expect(await screen.findByRole('switch', { name: 'Mic' })).toHaveAttribute('aria-checked', 'true');
    fireEvent.click(await screen.findByRole('button', { name: 'Stop sharing' }));
    await waitFor(() => expect(bridge.meetingStop).toHaveBeenCalled());
    const saved = vi.mocked(bridge.assistantLibrary).mock.calls.filter((c) => c[0] === 'put' && c[1] === 'chat');
    expect(saved.at(-1)?.[3]).not.toContain('"live"');
  });

  it('records a session with only the sound from the top bar', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Record a session' }));
    fireEvent.click(screen.getByRole('radio', { name: 'Sound only (transcript)' }));
    fireEvent.click(screen.getByRole('switch', { name: 'My microphone' }));
    fireEvent.click(screen.getByRole('button', { name: 'Start recording the sound' }));
    await waitFor(() =>
      expect(bridge.screenStart).toHaveBeenCalledWith(expect.stringMatching(/^Sound recording/), {
        screen: false,
        share: false,
        microphone: true,
        computerSound: true,
      }),
    );
  });
});
