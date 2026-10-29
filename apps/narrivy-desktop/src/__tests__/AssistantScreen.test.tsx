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

  it("lists the open chat's saved sessions only, not those of other chats", async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    const session = (id: string, title: string, at: number) => ({
      id,
      title,
      started_unix_ms: at,
      duration_ms: 5000,
      state: 'ready',
      progress: 1000,
      model: 'large-v3',
      speakers: 0,
      share: true,
      raw: true,
    });
    // Yesterday's share, made in another chat.
    vi.mocked(bridge.meetings).mockResolvedValue([session('m-yesterday', 'Screen share 2:48 PM', 1000)]);
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    fireEvent.click(await screen.findByRole('switch', { name: 'Screen' }));
    await waitFor(() =>
      expect(bridge.assistantLibrary).toHaveBeenCalledWith('put', 'chat', null, expect.stringContaining('"sessions":["m-screen"]')),
    );
    expect(screen.queryByRole('region', { name: 'Saved sessions' })).toBeNull();
    vi.mocked(bridge.meetings).mockResolvedValue([
      session('m-yesterday', 'Screen share 2:48 PM', 1000),
      session('m-screen', 'Screen share 3:27 PM', 2000),
    ]);
    fireEvent.click(await screen.findByRole('button', { name: 'Stop sharing' }));
    const list = await screen.findByRole('region', { name: 'Saved sessions' });
    await waitFor(() => expect(within(list).getAllByText(/PM$/).map((el) => el.textContent)).toEqual(['Screen share 3:27 PM']));
    vi.mocked(bridge.meetings).mockResolvedValue([]);
  });

  it('shows the model switch once the service answers, after a failed first ask', async () => {
    window.localStorage.clear();
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    // The app opened while the service was restarting: the first ask fails.
    vi.mocked(bridge.languageModels)
      .mockRejectedValueOnce({ code: 'daemon_unavailable', message: 'daemon_unavailable' })
      .mockResolvedValue([
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
          assistant_state: 'unloaded',
        },
      ]);
    render(<App />);
    expect(await screen.findByRole('switch', { name: 'Assistant model' })).toBeInTheDocument();
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

  it('saves a stopped share raw at once, and goes over it only when given to a chat', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    const handlers: ((event: SessionEvent) => void)[] = [];
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      handlers.push(handler);
      return () => {};
    });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    fireEvent.click(await screen.findByRole('switch', { name: 'Screen' }));
    const stop = await screen.findByRole('button', { name: 'Stop sharing' });
    // The service saves it as heard: no transcribing on stop.
    const saved = {
      meeting: {
        id: 'm-screen',
        title: 'Screen share',
        started_unix_ms: Date.now(),
        duration_ms: 5000,
        state: 'ready',
        progress: 1000,
        model: 'large-v3',
        speakers: 0,
        share: true,
        raw: true,
      },
      turns: [{ speaker: 'Person 1', source: 'call', start_ms: 0, end_ms: 1000, text: 'Live words.', language: 'en' }],
      names: {},
      models: [],
      folder: '/m',
      notes: {},
      recording: false,
    };
    vi.mocked(bridge.meeting).mockResolvedValue(saved as never);
    fireEvent.click(stop);
    await waitFor(() => expect(bridge.meetingStop).toHaveBeenCalled());
    act(() => handlers.forEach((h) => h({ event: 'meeting_changed', id: 'm-screen', state: 'ready', progress: 1000 } as SessionEvent)));
    // Ready to give at once, not "transcribing" for minutes.
    await waitFor(() => expect(screen.queryByText('Stopping and saving the session…')).not.toBeInTheDocument());
    const panel = await screen.findByRole('region', { name: 'Saved session' });
    expect(within(panel).getByText(/Saved as heard/)).toBeInTheDocument();
    expect(bridge.meetingProcess).not.toHaveBeenCalled();
    // Given to the chat: it gets the live transcript now, and is gone over.
    fireEvent.click(within(panel).getByRole('button', { name: 'Give it to this chat' }));
    await waitFor(() => expect(bridge.meetingProcess).toHaveBeenCalledWith('m-screen', null, null, null));
    expect(bridge.assistantLibrary).toHaveBeenCalledWith('put', 'chat', null, expect.stringContaining('Live words.'));
    // Gone over: the chat's copy becomes the full transcript.
    vi.mocked(bridge.meeting).mockResolvedValue({
      ...saved,
      meeting: { ...saved.meeting, raw: false },
      turns: [{ ...saved.turns[0], text: 'Full words.' }],
    } as never);
    act(() => handlers.forEach((h) => h({ event: 'meeting_changed', id: 'm-screen', state: 'ready', progress: 1000 } as SessionEvent)));
    await waitFor(() =>
      expect(bridge.assistantLibrary).toHaveBeenCalledWith('put', 'chat', null, expect.stringContaining('Full words.')),
    );
  });

  it('ends the share at once when the last source is switched off from the composer, with no event', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    fireEvent.click(await screen.findByRole('switch', { name: 'Screen' }));
    await screen.findByRole('button', { name: 'Stop sharing' });
    await waitFor(() => expect(bridge.meeting).toHaveBeenCalledWith('m-screen'));
    // The service answers the switch-off with the session stopped and saved;
    // no meeting_changed event reaches the app.
    vi.mocked(bridge.meetingSources).mockImplementationOnce(async (id: string) => ({
      id,
      title: 'Screen',
      started_unix_ms: Date.now(),
      duration_ms: 5000,
      state: 'ready',
      progress: 1000,
      model: 'large-v3',
      speakers: 0,
      screen_on: false,
      sound_on: false,
      mic_on: false,
    }));
    fireEvent.click(screen.getByRole('switch', { name: 'Screen' }));
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Stop sharing' })).not.toBeInTheDocument());
  });

  it('ends the share in the app when the service stops it (every source switched off)', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    const handlers: ((event: SessionEvent) => void)[] = [];
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      handlers.push(handler);
      return () => {};
    });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Assistant' }));
    fireEvent.click(await screen.findByRole('switch', { name: 'Sound' }));
    await screen.findByRole('button', { name: 'Stop sharing' });
    await waitFor(() =>
      expect(bridge.assistantLibrary).toHaveBeenCalledWith('put', 'chat', null, expect.stringContaining('"live":"m-screen"')),
    );
    // Switched off from the composer: the service stops and saves it.
    const stops = vi.mocked(bridge.meetingStop).mock.calls.length;
    act(() => handlers.forEach((h) => h({ event: 'meeting_changed', id: 'm-screen', state: 'ready', progress: 1000 } as SessionEvent)));
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Stop sharing' })).not.toBeInTheDocument());
    const saved = vi.mocked(bridge.assistantLibrary).mock.calls.filter((c) => c[0] === 'put' && c[1] === 'chat');
    await waitFor(() => expect(saved.at(-1)?.[3]).not.toContain('"live"'));
    expect(bridge.meetingStop).toHaveBeenCalledTimes(stops);
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
