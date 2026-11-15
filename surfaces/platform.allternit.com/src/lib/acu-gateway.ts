// ACU (Allternit Computer Use) gateway base URL resolver.
//
// The browser can't reach a local Tailscale runtime directly, so the default
// falls back to a configurable gateway. In the desktop/ai.allternit.com shell a
// local engine or localStorage override is used; in the web console the same
// overrides apply, with an env-var fallback for hosted deployments.

const BASE_URL_STORAGE_KEY = 'allternit.platform.computerUse.baseUrl';
const BASE_URL_SOURCE_STORAGE_KEY = 'allternit.platform.computerUse.baseUrlSource';

function normalizeBaseUrl(value?: string | null): string | null {
  const trimmed = String(value ?? '').trim();
  if (!trimmed) {
    const env = import.meta.env.VITE_ACU_GATEWAY_URL;
    if (env) return normalizeBaseUrl(env);
    return null;
  }

  if (/^https?:\/\//i.test(trimmed)) {
    return trimmed.replace(/\/+$/, '');
  }

  if (/^\d{2,5}$/.test(trimmed)) {
    return `http://127.0.0.1:${trimmed}`;
  }

  if (/^[^/:?#]+:\d{2,5}$/i.test(trimmed)) {
    return `http://${trimmed}`;
  }

  return trimmed.replace(/\/+$/, '');
}

/** Returns null when no gateway is configured (no localhost fallback). */
export function getAcuGatewayBaseUrl(): string | null {
  if (typeof window !== 'undefined') {
    try {
      const stored = window.localStorage.getItem(BASE_URL_STORAGE_KEY);
      const source = window.localStorage.getItem(BASE_URL_SOURCE_STORAGE_KEY);
      if (stored && source === 'manual') {
        return normalizeBaseUrl(stored);
      }
      if (stored) {
        return normalizeBaseUrl(stored);
      }
    } catch {
      // Ignore localStorage access failures.
    }
  }
  return normalizeBaseUrl(import.meta.env.VITE_ACU_GATEWAY_URL);
}
