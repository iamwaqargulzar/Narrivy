// Focus handling shared by dialogs and popovers (WCAG 2.1.2, 2.4.3, 2.4.7).
// A modal dialog takes focus when it opens, keeps Tab inside itself and gives
// focus back to where it was when it closes. A popover closes on Escape or a
// click outside and, when focus was inside it, returns focus to its trigger.
import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent, type RefObject } from 'react';

const FOCUSABLE = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

export function focusableIn(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((el) => !el.closest('[inert]'));
}

/** Keeps Tab and Shift+Tab inside `root`. Call it from the dialog's keydown handler. */
export function keepTabInside(event: KeyboardEvent | ReactKeyboardEvent, root: HTMLElement | null): void {
  if (event.key !== 'Tab' || !root || event.defaultPrevented) return;
  const items = focusableIn(root);
  const active = document.activeElement as HTMLElement | null;
  if (items.length === 0) {
    event.preventDefault();
    root.focus();
    return;
  }
  const first = items[0];
  const last = items[items.length - 1];
  const outside = !active || !root.contains(active) || active === root;
  if (event.shiftKey && (outside || active === first)) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && (outside || active === last)) {
    event.preventDefault();
    first.focus();
  }
}

/**
 * Moves focus into a modal dialog while it is open and back to the element
 * that had it before, once it closes. The dialog element itself takes focus
 * (give it tabIndex={-1}) so its name is read first.
 */
export function useDialogFocus(ref: RefObject<HTMLElement | null>, open = true): void {
  useEffect(() => {
    if (!open) return undefined;
    const previous = document.activeElement as HTMLElement | null;
    const root = ref.current;
    if (root && !root.contains(document.activeElement)) root.focus();
    return () => {
      if (previous && previous.isConnected && typeof previous.focus === 'function') previous.focus();
    };
  }, [ref, open]);
}

/**
 * Closes a popover on a click outside it or on Escape. Escape gives focus
 * back to the trigger (the element in the popover root carrying
 * aria-expanded) when focus was inside. Arrow keys move between the items of
 * a role="menu" inside it.
 */
export function usePopoverDismiss<T extends HTMLElement = HTMLDivElement>(open: boolean, close: () => void) {
  const ref = useRef<T | null>(null);
  useEffect(() => {
    if (!open) return undefined;
    const onDown = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) close();
    };
    const onKey = (event: KeyboardEvent) => {
      const root = ref.current;
      if (event.key === 'Escape') {
        const inside = Boolean(root && root.contains(document.activeElement));
        close();
        if (inside) root?.querySelector<HTMLElement>('[aria-expanded]')?.focus();
        return;
      }
      if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
      const menu = (document.activeElement as HTMLElement | null)?.closest<HTMLElement>('[role="menu"]');
      if (!root || !menu || !root.contains(menu)) return;
      const items = Array.from(menu.querySelectorAll<HTMLElement>('[role^="menuitem"]:not([disabled])'));
      if (items.length === 0) return;
      event.preventDefault();
      const index = items.indexOf(document.activeElement as HTMLElement);
      const step = event.key === 'ArrowDown' ? 1 : -1;
      items[(index + step + items.length) % items.length].focus();
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open, close]);
  return ref;
}
