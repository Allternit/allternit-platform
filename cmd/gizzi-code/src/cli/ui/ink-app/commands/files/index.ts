import type { Command } from '../../commands'
import { filesDriveEnabledSync } from '@/cli/files/flag'

/**
 * `/files`: the Files drive (contracts/files-drive, SPEC section 18). Browse a
 * computer's folders, versions, trash and share links without leaving the
 * session. It only reads, and browsing never wakes a computer; changes and
 * downloads go through `gizzi files …`, which asks before waking one.
 * The files-in-context list that used to live here is `/context files`.
 * Hidden until the account's files_drive flag is on (cached /me/flags answer,
 * refreshed at TUI start; ALLTERNIT_FILES_DRIVE=1 forces it).
 */
const files = {
  type: 'local',
  name: 'files',
  description: "Browse your computers' files (the Files drive). Files in context: /context files",
  argumentHint: '[ls] [computer:]Folder/sub | versions <path> | trash [folder] | search <text> | shares',
  isEnabled: () => filesDriveEnabledSync(),
  supportsNonInteractive: true,
  load: () => import('./files.js'),
} satisfies Command
export default files
