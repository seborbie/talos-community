import { describe, expect, test } from 'bun:test';
import { resolveOperatorContent, safeOperatorUrl } from './operatorContent';

describe('operator-facing Community content', () => {
  test('accepts only absolute HTTP(S) URLs without embedded credentials', () => {
    expect(safeOperatorUrl('https://support.example.test/help')).toBe(
      'https://support.example.test/help',
    );
    expect(safeOperatorUrl('javascript:alert(1)')).toBeNull();
    expect(safeOperatorUrl('/internal/support')).toBeNull();
    expect(safeOperatorUrl('https://user:secret@example.test')).toBeNull();
  });

  test('defaults to an unconfigured operator instead of Talos-hosted claims', () => {
    expect(resolveOperatorContent({})).toEqual({
      name: null,
      supportUrl: null,
      termsUrl: null,
      privacyUrl: null,
      sourceUrl: 'https://github.com/seborbie/talos-community',
    });
  });

  test('normalizes bounded operator configuration', () => {
    expect(
      resolveOperatorContent({
        PUBLIC_OPERATOR_NAME: '  Example MSP  ',
        PUBLIC_SUPPORT_URL: 'https://support.example.test',
        PUBLIC_TERMS_URL: 'https://example.test/terms',
        PUBLIC_PRIVACY_URL: 'https://example.test/privacy',
        PUBLIC_SOURCE_URL: 'https://github.com/example/talos-community',
      }),
    ).toEqual({
      name: 'Example MSP',
      supportUrl: 'https://support.example.test/',
      termsUrl: 'https://example.test/terms',
      privacyUrl: 'https://example.test/privacy',
      sourceUrl: 'https://github.com/example/talos-community',
    });
  });
});
