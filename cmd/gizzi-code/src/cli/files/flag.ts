/**
 * The Files drive ships dark behind the per-user flag `files_drive`
 * (PLAN D10). Gizzi has no per-user flag reader yet, so it reads the same
 * switch the runtime uses: ALLTERNIT_FILES_DRIVE=1. Kept in its own tiny
 * module so the TUI command list can check it without loading the client.
 */
export function filesDriveEnabled(env: NodeJS.ProcessEnv = process.env): boolean {
  const v = env.ALLTERNIT_FILES_DRIVE?.trim().toLowerCase()
  return v === "1" || v === "true" || v === "on"
}

export const FILES_DRIVE_OFF = "Files drive is not on for this account yet"
