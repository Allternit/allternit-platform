/**
 * Classify allternit-api custom-protocol URLs so Electron injects the
 * desktop device token only onto the local operator API — never onto
 * api.allternit.com (host `cloud`), which expects a Clerk JWT.
 */

export function isCloudControlPlaneUrl(url: string, cloudApiOrigin: string): boolean {
  try {
    const target = new URL(url);
    if (target.protocol === 'allternit-api:') {
      return target.hostname === 'cloud' || target.host === 'cloud';
    }
    return target.origin === cloudApiOrigin;
  } catch {
    return false;
  }
}

/** True when Electron main should attach the paired desktop device identity. */
export function shouldInjectDesktopIdentity(
  url: string,
  localApiOrigin: string,
  cloudApiOrigin: string,
): boolean {
  if (isCloudControlPlaneUrl(url, cloudApiOrigin)) return false;
  try {
    const target = new URL(url);
    return (
      target.origin === localApiOrigin
      || (target.protocol === 'allternit-api:' && target.hostname !== 'cloud')
    );
  } catch {
    return false;
  }
}

/** Rewrite https://api.allternit.com/... to the privileged custom protocol. */
export function rewriteCloudApiToProtocol(url: string, cloudApiOrigin: string): string | null {
  if (!url.startsWith(`${cloudApiOrigin}/`)) return null;
  return url.replace(cloudApiOrigin, 'allternit-api://cloud');
}

/**
 * Cloud-api `GET /v1/models` is a public catalog. Pairing device tokens and
 * Clerk JWTs must not be attached — they turn a simple GET into a CORS
 * preflight / 401.
 */
export function isPublicCloudCatalogPath(urlOrPath: string): boolean {
  let path = urlOrPath;
  try {
    if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(urlOrPath)) {
      path = new URL(urlOrPath).pathname;
    }
  } catch {
    path = urlOrPath;
  }
  const normalized = path.split('?')[0];
  return normalized === '/v1/models' || normalized.startsWith('/v1/models/');
}

/**
 * Cloud routes that accept this desktop's paired device token as well as a
 * Clerk JWT (allternit-cloud-api: `mesh.rs` enroll, `paired_computers.rs`).
 * When the app has no Clerk session (signed in by device pairing only), main
 * sends the device token for these instead of nothing.
 */
const DEVICE_TOKEN_CLOUD_PATHS = ['/api/v1/mesh/enroll', '/api/v1/computers/pairing-codes', '/api/v1/computers/paired'];

export function acceptsDeviceToken(urlOrPath: string): boolean {
  let path = urlOrPath;
  try {
    if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(urlOrPath)) path = new URL(urlOrPath).pathname;
  } catch {
    path = urlOrPath;
  }
  const normalized = path.split('?')[0];
  return DEVICE_TOKEN_CLOUD_PATHS.some((prefix) => normalized === prefix || normalized.startsWith(`${prefix}/`));
}
