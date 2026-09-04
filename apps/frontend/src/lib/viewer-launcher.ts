import { browser } from '$app/environment';
import { env } from '$env/dynamic/public';
import { resolveRuntimePublicServiceUrls } from '$lib/runtimePublicConfig';

export type ViewerLaunchStatus = 'launched' | 'unsupported_platform';

export type ViewerLaunchResult = {
  status: ViewerLaunchStatus;
};

export type ViewerInstallerPlatform = 'windows' | 'macos';

const SUPPORTED_PLATFORM_PATTERNS = ['Win32', 'Win64', 'Windows', 'MacIntel', 'MacPPC', 'Mac68K'];

export function isDesktopViewerLaunchSupported(): boolean {
  if (!browser) {
    return false;
  }

  const navigatorPlatform = window.navigator.platform || '';
  const navigatorUserAgent = window.navigator.userAgent || '';
  return (
    SUPPORTED_PLATFORM_PATTERNS.some((pattern) => navigatorPlatform.includes(pattern)) ||
    navigatorUserAgent.includes('Windows') ||
    navigatorUserAgent.includes('Macintosh') ||
    navigatorUserAgent.includes('Mac OS X')
  );
}

export const isWindowsViewerInstallSupported = isDesktopViewerLaunchSupported;

export function detectViewerInstallerPlatform(): ViewerInstallerPlatform {
  if (!browser) {
    return 'windows';
  }

  const userAgentData = (window.navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData;
  const navigatorPlatform = userAgentData?.platform || window.navigator.platform || '';
  const navigatorUserAgent = window.navigator.userAgent || '';
  if (
    navigatorPlatform.includes('Mac') ||
    navigatorUserAgent.includes('Macintosh') ||
    navigatorUserAgent.includes('Mac OS X')
  ) {
    return 'macos';
  }
  return 'windows';
}

function injectBackendApi(url: string): string {
  const backendApi = resolveRuntimePublicServiceUrls(env).apiUrl;
  if (!backendApi) {
    return url;
  }
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== 'rmm:' || parsed.searchParams.has('backendApi')) {
      return url;
    }
    parsed.searchParams.set('backendApi', backendApi);
    return parsed.toString();
  } catch {
    return url;
  }
}

export async function launchViewerDeepLink(url: string): Promise<ViewerLaunchResult> {
  if (!browser || !isDesktopViewerLaunchSupported()) {
    return { status: 'unsupported_platform' };
  }

  window.location.assign(injectBackendApi(url));
  return { status: 'launched' };
}
