/**
 * Credential and git-control paths: the patterns the default `read` and `edit`
 * rules ask about (runtime/loop/agent.ts), kept in one place so other tools
 * that hand files on (send_user_message) refuse the same files.
 *
 * Order matters: the last matching pattern wins (PermissionNext's findLast),
 * so the template and public-key variants listed after the deny patterns stay
 * ordinary files. Free of runtime state, so the Ink adapters can use it too.
 */
import { Wildcard } from "@/shared/util/wildcard"

export const CREDENTIAL_PATH_RULES = {
  "*.env": "ask",
  "*.env.*": "ask",
  "*id_rsa*": "ask",
  "*id_ed25519*": "ask",
  "*id_ecdsa*": "ask",
  "*credentials*": "ask",
  ".git/*": "ask",
  "*/.git/*": "ask",
  "*.env.example": "allow",
  "*.env.sample": "allow",
  "*.env.template": "allow",
  "*id_rsa.pub": "allow",
  "*id_ed25519.pub": "allow",
  "*id_ecdsa.pub": "allow",
} as const satisfies Record<string, "ask" | "allow">

/** True when `filepath` matches a credential pattern (last match wins). */
export function isCredentialPath(filepath: string): boolean {
  const normalized = filepath.replaceAll("\\", "/")
  let action: "ask" | "allow" | undefined
  for (const [pattern, value] of Object.entries(CREDENTIAL_PATH_RULES)) {
    if (Wildcard.match(normalized, pattern)) action = value
  }
  return action === "ask"
}
