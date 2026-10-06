import { mkdir, readFile, writeFile } from 'fs/promises'
import { dirname } from 'path'
import type { LocalCommandCall } from '../../types/command.js'
import { getMemoryPath } from '../../utils/config.js'
import { getErrnoCode } from '../../utils/errors.js'
import { saveToMemoryDrive } from '../../utils/memory/quickAdd.js'
import {
  buildRememberedContent,
  parseRememberArgs,
} from './rememberTarget.js'

export const call: LocalCommandCall = async args => {
  const parsed = parseRememberArgs(String(args ?? ''))
  if ('error' in parsed) {
    return {
      type: 'text',
      value: parsed.error,
    }
  }

  const memoryPath = getMemoryPath(parsed.target)
  if (await saveToMemoryDrive(memoryPath, parsed.note)) {
    return { type: 'text', value: `Saved to your Memory Drive (notes.md)\n- ${parsed.note}` }
  }
  await mkdir(dirname(memoryPath), { recursive: true })

  let existing = ''
  try {
    existing = await readFile(memoryPath, 'utf8')
  } catch (error) {
    if (getErrnoCode(error) !== 'ENOENT') throw error
  }

  const stamp = new Date().toISOString().slice(0, 10)
  const { next, line } = buildRememberedContent(existing, parsed.note, stamp)

  await writeFile(memoryPath, next, 'utf8')
  return {
    type: 'text',
    value: `Saved to ${memoryPath}\n${line}`,
  }
}
