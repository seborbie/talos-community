import { writable } from 'svelte/store';
import { browser } from '$app/environment';

export const isLightMode = writable(false);

export function toggleTheme() {
  isLightMode.update((current) => {
    const next = !current;
    if (browser) {
      if (next) {
        document.documentElement.classList.add('light');
        localStorage.setItem('aero-theme', 'light');
      } else {
        document.documentElement.classList.remove('light');
        localStorage.setItem('aero-theme', 'dark');
      }
    }
    return next;
  });
}

export function initTheme() {
  if (!browser) return;
  const saved = localStorage.getItem('aero-theme');
  if (saved === 'light') {
    document.documentElement.classList.add('light');
    isLightMode.set(true);
  } else {
    document.documentElement.classList.remove('light');
    isLightMode.set(false);
  }
}
