import { mount } from 'svelte';
import App from './App.svelte';
import './app.css';

function isTauriRuntime(): boolean {
  // Works for Tauri v1/v2; avoid breaking normal web dev experience.
  return typeof window !== 'undefined' && (window as any).__TAURI__ != null;
}

function isRemoteDesktopFrameFocused(): boolean {
  if (typeof document === 'undefined') {
    return false;
  }
  const active = document.activeElement;
  return active instanceof HTMLElement && active.classList.contains('remote-desktop-frame');
}

/**
 * True when the key combo should be swallowed so WebView2 / Edge-style browser
 * chrome (reload, devtools, tabs, history, etc.) does not run.
 *
 * Does not block Ctrl+Shift+F: the viewer uses that on the remote viewport to
 * paste clipboard text as keystrokes (see App.svelte). It also does not block
 * Ctrl+Shift+Delete while the remote desktop frame is focused, because the
 * agent/helper interprets that combo as secure-attention (Ctrl+Alt+Del).
 */
function shouldBlockChromiumShortcut(event: KeyboardEvent): boolean {
  const { key, code, ctrlKey, metaKey, shiftKey, altKey } = event;
  const ctrlOrCmd = ctrlKey || metaKey;

  if (ctrlOrCmd && shiftKey && key.toLowerCase() === 'f') {
    return false;
  }

  const u = typeof key === 'string' ? key.toUpperCase() : '';

  // Prefer `code` (physical key); WebView2 can expose F5 reliably here even when `key` differs.
  if (
    code === 'F3' ||
    code === 'F5' ||
    code === 'F6' ||
    code === 'F7' ||
    code === 'F12'
  ) {
    return true;
  }
  if (key === 'F5' || key === 'F12' || key === 'F3' || key === 'F7') {
    return true;
  }
  if (key === 'F6' || (ctrlOrCmd && key === 'F6')) {
    return true;
  }

  if (ctrlOrCmd && u === 'R') {
    return true;
  }
  if (ctrlOrCmd && u === 'U') {
    return true;
  }
  if (ctrlOrCmd && shiftKey && (u === 'I' || u === 'J' || u === 'C')) {
    return true;
  }

  if (ctrlOrCmd && u === 'G') {
    return true;
  }

  if (
    ctrlOrCmd &&
    !shiftKey &&
    (u === 'L' ||
      u === 'D' ||
      u === 'H' ||
      u === 'J' ||
      u === 'T' ||
      u === 'N' ||
      u === 'W' ||
      u === 'K' ||
      u === 'E' ||
      u === 'M')
  ) {
    return true;
  }

  if (ctrlOrCmd && shiftKey && (u === 'N' || u === 'T' || u === 'B' || u === 'O' || u === 'D')) {
    return true;
  }

  if (ctrlOrCmd && key === 'Tab') {
    return true;
  }

  if (ctrlOrCmd && u.length === 1 && u >= '1' && u <= '9') {
    return true;
  }

  if (ctrlOrCmd && (key === 'PageUp' || key === 'PageDown')) {
    return true;
  }

  if (ctrlOrCmd && shiftKey && key === 'Delete') {
    if (isRemoteDesktopFrameFocused()) {
      return false;
    }
    return true;
  }

  if (altKey && !ctrlOrCmd && u === 'D') {
    return true;
  }

  if (altKey && !ctrlOrCmd && (key === 'ArrowLeft' || key === 'ArrowRight')) {
    return true;
  }

  return false;
}

function installContextMenuGuard(): void {
  const blockContextMenu = (event: MouseEvent) => {
    event.preventDefault();
  };

  const opts = { capture: true } as const;
  window.addEventListener('contextmenu', blockContextMenu, opts);
  document.addEventListener('contextmenu', blockContextMenu, opts);
}

function installChromiumShortcutGuard(): void {
  const blockChromiumShortcuts = (event: KeyboardEvent) => {
    if (!shouldBlockChromiumShortcut(event)) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation();
  };

  const keyOpts = { capture: true } as const;
  window.addEventListener('keydown', blockChromiumShortcuts, keyOpts);
  document.addEventListener('keydown', blockChromiumShortcuts, keyOpts);
}

function installDesktopHardeningGuards(): void {
  installContextMenuGuard();
  installChromiumShortcutGuard();
}

if (isTauriRuntime()) {
  installDesktopHardeningGuards();
}

const app = mount(App, {
  target: document.getElementById('app')!,
});

export default app;
