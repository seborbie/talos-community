import { browser } from '$app/environment';
import { writable } from 'svelte/store';

const STORAGE_KEY = 'aero-cursor-effects';

export const cursorEffectsEnabled = writable(true);

export function initCursorEffects() {
  if (!browser) return;
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved !== null) {
    // Load the user preference; runtime environment checks still gate rendering.
    cursorEffectsEnabled.set(saved !== 'off');
  } else {
    // No saved preference — default to off when the OS requests reduced motion.
    const prefersReduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    cursorEffectsEnabled.set(!prefersReduced);
  }
}

export function toggleCursorEffects() {
  cursorEffectsEnabled.update((current) => {
    const next = !current;
    if (browser) {
      localStorage.setItem(STORAGE_KEY, next ? 'on' : 'off');
    }
    return next;
  });
}
