import { error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';

const shortCodePattern = /^[a-z0-9]{8}$/;

function trimTrailingSlash(value: string): string {
  return value.replace(/\/+$/, '');
}

function resolveApiBaseUrl(): string {
  const value = process.env.INTERNAL_API_URL || process.env.PUBLIC_API_URL;
  if (!value?.trim()) {
    throw error(500, 'API URL is not configured');
  }
  return trimTrailingSlash(value.trim());
}

function resolvePublicApiForwardingTarget(): URL {
  const value = process.env.PUBLIC_API_URL || process.env.INTERNAL_API_URL;
  if (!value?.trim()) {
    throw error(500, 'API URL is not configured');
  }
  return new URL(trimTrailingSlash(value.trim()));
}

export const GET: RequestHandler = async ({ fetch, params, request }) => {
  const shortCode = params.shortCode.trim().toLowerCase();
  if (!shortCodePattern.test(shortCode)) {
    throw error(404, 'Not found');
  }

  const apiUrl = `${resolveApiBaseUrl()}/rmm/installers/linux/short/${encodeURIComponent(shortCode)}/install.sh`;
  const publicApiUrl = resolvePublicApiForwardingTarget();
  const response = await fetch(apiUrl, {
    headers: {
      'user-agent': request.headers.get('user-agent') ?? '',
      'x-forwarded-for': request.headers.get('x-forwarded-for') ?? '',
      'x-forwarded-proto': publicApiUrl.protocol.replace(/:$/, ''),
      'x-forwarded-host': publicApiUrl.host
    }
  });

  const body = await response.text();
  const contentType = response.headers.get('content-type') ?? 'text/x-shellscript; charset=utf-8';

  return new Response(body, {
    status: response.status,
    headers: {
      'content-type': contentType,
      'cache-control': 'no-store'
    }
  });
};
