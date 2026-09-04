// @ts-nocheck
import { describe, expect, test } from 'bun:test';
import { renderMarkdown } from './render';

describe('renderMarkdown secure note links', () => {
  test('linkifies relative secure note paths with full visible labels', () => {
    const originalWindow = globalThis.window;
    (globalThis as any).window = {
      location: { origin: 'https://dev.telos.cloud' }
    };
    try {
      const html = renderMarkdown('Secure note: /SN/5h8i5g5j');
      expect(html).toContain('href="/SN/5h8i5g5j"');
      expect(html).toContain('https://dev.telos.cloud/SN/5h8i5g5j');
    } finally {
      (globalThis as any).window = originalWindow;
    }
  });

  test('linkifies backticked secure note paths instead of rendering them as code', () => {
    const originalWindow = globalThis.window;
    (globalThis as any).window = {
      location: { origin: 'https://dev.telos.cloud' }
    };
    try {
      const html = renderMarkdown('Secure note: `/SN/r95wtgzz`.');
      expect(html).toContain('href="/SN/r95wtgzz"');
      expect(html).toContain('https://dev.telos.cloud/SN/r95wtgzz');
      expect(html).not.toContain('<code>/SN/r95wtgzz</code>');
    } finally {
      (globalThis as any).window = originalWindow;
    }
  });

  test('linkifies bold secure note paths with full visible labels', () => {
    const originalWindow = globalThis.window;
    (globalThis as any).window = {
      location: { origin: 'https://dev.telos.cloud' }
    };
    try {
      const html = renderMarkdown('Secure note: **/SN/jayaktq0**');
      expect(html).toContain('<strong><a href="/SN/jayaktq0"');
      expect(html).toContain('https://dev.telos.cloud/SN/jayaktq0');
      expect(html).not.toContain('<strong>/SN/jayaktq0</strong>');
    } finally {
      (globalThis as any).window = originalWindow;
    }
  });

  test('linkifies full URLs without rewriting markdown anchors', () => {
    const html = renderMarkdown('Open https://dev.telos.cloud/SN/5h8i5g5j or [dashboard](https://dev.telos.cloud/dashboard).');
    expect(html).toContain('href="https://dev.telos.cloud/SN/5h8i5g5j"');
    expect(html).toContain('>https://dev.telos.cloud/SN/5h8i5g5j</a>');
    expect(html).toContain('href="https://dev.telos.cloud/dashboard"');
    expect(html).toContain('>dashboard</a>');
  });
});
