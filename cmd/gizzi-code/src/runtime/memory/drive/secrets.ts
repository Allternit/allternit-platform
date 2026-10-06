/**
 * Secret gate for Memory Drive candidates.
 *
 * Two layers, both run over every file of a candidate tree before gizzi
 * commits it:
 *  1. gizzi's existing gitleaks-derived scanner (teamMemorySync/secretScanner)
 *  2. the server's push-gate rules (memory_kernel_service::mentions_secret +
 *     memory_drive::scan_secrets), ported so a commit the server would reject
 *     never lands locally.
 * Only a boolean comes out: matched text is never logged, returned or shown.
 */
import { scanForSecrets } from "@/cli/ui/ink-app/services/teamMemorySync/secretScanner"

const MARKERS = [
  "password",
  "passcode",
  "passwd",
  "api key",
  "api_key",
  "apikey",
  "secret key",
  "access token",
  "private key",
  "seed phrase",
  "recovery phrase",
  "ssn",
  "social security",
  "card number",
  "cvv",
  "pin is",
  "pin code",
]

const TOKEN_PATTERN =
  /(sk-[a-z0-9_-]{8,}|gh[pousr]_[a-z0-9_]{8,}|github_pat_[a-z0-9_]+|glpat-[a-z0-9_-]+|xox[baprs]-[a-z0-9-]+|AKIA[A-Z0-9]{16}|ASIA[A-Z0-9]{16}|allternit_git_[a-z0-9]+|eyJ[a-z0-9_-]+\.[a-z0-9_-]+\.[a-z0-9_-]+|-----BEGIN [A-Z ]*PRIVATE KEY-----|(?:token|secret|password)\s*[:=]\s*[^\s]+)/i

/** Port of memory_kernel_service::mentions_secret. */
function mentionsSecret(text: string): boolean {
  const lower = text.toLowerCase()
  if (MARKERS.some((m) => lower.includes(m))) return true
  return lower.split(/\s+/).some((raw) => {
    const w = raw.replace(/^[^a-z0-9_-]+|[^a-z0-9_-]+$/g, "")
    if (["sk-", "ghp_", "gho_", "xoxb-", "xoxp-", "akia"].some((p) => w.startsWith(p))) return true
    return w.length >= 13 && w.length <= 19 && /^[0-9]+$/.test(w)
  })
}

/** True when `content` looks like it holds a credential or other secret. */
export function scanDriveSecrets(content: string): boolean {
  return mentionsSecret(content) || TOKEN_PATTERN.test(content) || scanForSecrets(content).length > 0
}
