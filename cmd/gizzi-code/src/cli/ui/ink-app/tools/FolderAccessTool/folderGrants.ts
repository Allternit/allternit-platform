/**
 * Session state for folder_access request_delete grants. Kept free of tool and
 * permission imports so the Ink permission path can read it without an import
 * cycle (permissions.ts → this file, never → FolderAccessTool.ts).
 */
import { resolve } from 'path'
import { expandPath } from '../../utils/path.js'
import { pathInWorkingPath } from '../../utils/permissions/pathInWorkingPath.js'

/**
 * Folders granted with folder_access `request`, this process (session). Only
 * these get the _to_delete move: folders added with /add-dir or settings keep
 * their old behavior.
 */
export const accessGrantedFolders = new Set<string>()

/** Folders the user approved `request_delete` for, this process (session). */
export const deleteGrantedFolders = new Set<string>()

/** Whether a real delete (not the _to_delete/ move) is granted for this path. */
export function isDeleteGranted(target: string): boolean {
  const abs = resolve(expandPath(target))
  for (const folder of deleteGrantedFolders) {
    if (pathInWorkingPath(abs, folder)) return true
  }
  return false
}
