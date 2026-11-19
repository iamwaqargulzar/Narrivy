import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { bridge } from '../lib/bridge';
import { DocumentCard, documentOf } from '../app/components/assistant/DocumentCard';
import { ChangesBar } from '../app/components/assistant/ChangesBar';
import { permissionOf } from '../state/AssistantContext';
import { AllowedCommands } from '../app/screens/Assistant';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const detail = JSON.stringify({
  kind: 'document',
  id: 'doc-1',
  chat: 'chat-1',
  title: 'Quarterly review',
  format: 'docx',
  name: 'Quarterly review.docx',
  size: 6200,
});

// D55: documents are made in the chat, like web chat apps, by Narrivy's own
// writers; the card previews, downloads and opens them.
describe('document card', () => {
  it('reads only document details', () => {
    expect(documentOf(detail)?.name).toBe('Quarterly review.docx');
    expect(documentOf(JSON.stringify({ kind: 'write', path: '/x' }))).toBeNull();
    expect(documentOf('not json')).toBeNull();
  });

  it('previews, downloads, shows and opens the file', async () => {
    const doc = documentOf(detail);
    if (!doc) throw new Error('no document');
    vi.mocked(bridge.assistantLibrary).mockImplementation(async (op) => {
      if (op === 'get') return JSON.stringify({ markdown: '# Quarterly review\n\nSales grew.' });
      if (op === 'download') return JSON.stringify({ path: '/home/u/Downloads/Quarterly review.docx' });
      return '{}';
    });
    render(<DocumentCard doc={doc} renderPreview={(md) => <pre>{md}</pre>} />);
    expect(screen.getByText('Word document · 6 KB')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
    expect(await screen.findByText(/Sales grew/)).toBeInTheDocument();
    expect(bridge.assistantLibrary).toHaveBeenCalledWith('get', 'document', 'chat-1/doc-1');
    fireEvent.click(screen.getByRole('button', { name: 'Download' }));
    expect(await screen.findByText('Saved to /home/u/Downloads/Quarterly review.docx.')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Show in folder' }));
    await waitFor(() =>
      expect(bridge.assistantLibrary).toHaveBeenCalledWith('show', 'document', 'chat-1/doc-1', '/home/u/Downloads/Quarterly review.docx'),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Open' }));
    await waitFor(() => expect(bridge.assistantLibrary).toHaveBeenCalledWith('open', 'document', 'chat-1/doc-1'));
  });

  it('says how many slides a deck has', () => {
    const deck = documentOf(JSON.stringify({ ...JSON.parse(detail), format: 'pptx', name: 'Deck.pptx', units: 6 }));
    if (!deck) throw new Error('no document');
    render(<DocumentCard doc={deck} renderPreview={(md) => <pre>{md}</pre>} />);
    expect(screen.getByText('Presentation · 6 slides · 6 KB')).toBeInTheDocument();
  });

  it('says plainly when the file is gone', async () => {
    const doc = documentOf(detail);
    if (!doc) throw new Error('no document');
    vi.mocked(bridge.assistantLibrary).mockRejectedValue({ code: 'bad_request', message: 'document_not_found' });
    render(<DocumentCard doc={doc} renderPreview={(md) => <pre>{md}</pre>} />);
    fireEvent.click(screen.getByRole('button', { name: 'Open' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('no longer kept');
  });
});

describe('changes bar', () => {
  it('lists the changed files and undoes them after asking', async () => {
    let undone = false;
    vi.mocked(bridge.assistantLibrary).mockImplementation(async (op) => {
      if (op === 'list')
        return JSON.stringify({
          files: undone
            ? []
            : [
                { path: '/p/src/main.rs', new: false, undoable: true },
                { path: '/p/src/new.rs', new: true, undoable: true },
              ],
        });
      if (op === 'undo') {
        undone = true;
        return JSON.stringify({ restored: 1, removed: 1, failed: 0 });
      }
      return '{}';
    });
    render(<ChangesBar chatId="chat-1" busy={false} />);
    fireEvent.click(await screen.findByRole('button', { name: '2 files changed in this chat' }));
    expect(screen.getByText('/p/src/new.rs')).toBeInTheDocument();
    expect(screen.getByText(/not undone/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Undo all' }));
    expect(screen.getByText(/Put 2 files back/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Undo all' }));
    expect(await screen.findByText('Undone: 2 files are back as they were.')).toBeInTheDocument();
    expect(bridge.assistantLibrary).toHaveBeenCalledWith('undo', 'changes', 'chat-1');
  });

  it('shows nothing when no file was changed', async () => {
    vi.mocked(bridge.assistantLibrary).mockResolvedValue(JSON.stringify({ files: [] }));
    const { container } = render(<ChangesBar chatId="chat-1" busy={false} />);
    await waitFor(() => expect(bridge.assistantLibrary).toHaveBeenCalledWith('list', 'changes', 'chat-1'));
    expect(container).toBeEmptyDOMElement();
  });
});

describe('permission modes', () => {
  it('defaults to Auto and reads older chats', () => {
    expect(permissionOf(null)).toBe('auto');
    expect(permissionOf({})).toBe('auto');
    expect(permissionOf({ fullAccess: true })).toBe('bypass');
    expect(permissionOf({ permission: 'ask' })).toBe('manual');
    expect(permissionOf({ permission: 'plan', fullAccess: true })).toBe('plan');
  });
});

describe('allowed commands', () => {
  it('lists what runs without asking and takes one back', async () => {
    let kept = ['cargo test', 'ls'];
    vi.mocked(bridge.assistantLibrary).mockImplementation(async (op, _kind, _id, value) => {
      if (op === 'get') return JSON.stringify({ id: 'chat-1', allowedCommands: kept });
      if (op === 'forget') kept = kept.filter((c) => c !== value);
      return '{}';
    });
    render(<AllowedCommands chatId="chat-1" disabled={false} />);
    expect(await screen.findByText('cargo test')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Ask again before “cargo test”' }));
    await waitFor(() => expect(screen.queryByText('cargo test')).not.toBeInTheDocument());
    expect(bridge.assistantLibrary).toHaveBeenCalledWith('forget', 'command', 'chat-1', 'cargo test');
    expect(screen.getByText('ls')).toBeInTheDocument();
  });
});
