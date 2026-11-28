'use strict';

// Release builds download sidecar inputs (bundled Python, Cua Driver, Lima,
// mesh-node) from GitHub. One dropped connection used to fail the whole
// release job (release-desktop run 46067811157, Linux x64: "connect ETIMEDOUT"
// fetching python-build-standalone). Retry network errors and 5xx/429 with
// backoff; a 404 or a checksum mismatch still fails at once.

const SOCKET_TIMEOUT_MS = 60_000;

function retryable(err) {
  const status = /HTTP (\d{3})/.exec(String(err && err.message))?.[1];
  if (status) return status === '429' || status.startsWith('5');
  return true; // ETIMEDOUT, ECONNRESET, EAI_AGAIN, socket timeout, ...
}

async function withRetry(label, fn, { attempts = 4, baseDelayMs = 5_000 } = {}) {
  for (let attempt = 1; ; attempt += 1) {
    try {
      return await fn();
    } catch (err) {
      if (attempt >= attempts || !retryable(err)) throw err;
      const delay = baseDelayMs * 2 ** (attempt - 1);
      console.warn(`[retry-download] ${label}: ${err.message} (attempt ${attempt}/${attempts}); retrying in ${delay / 1000}s`);
      await new Promise((resolve) => setTimeout(resolve, delay));
    }
  }
}

// Abort a request whose socket goes quiet, so a stalled connection is retried
// instead of hanging until the OS gives up minutes later.
function guardSocket(req, url) {
  req.setTimeout(SOCKET_TIMEOUT_MS, () => req.destroy(new Error(`socket timeout after ${SOCKET_TIMEOUT_MS / 1000}s: ${url}`)));
  return req;
}

module.exports = { withRetry, guardSocket };
