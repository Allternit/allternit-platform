import * as React from 'react'
import { open } from 'fs/promises'
import { join } from 'path'
import { getCwdState } from '../../bootstrap/state.js'
import { Box, Text } from '../../ink.js'
import type { LocalJSXCommandCall } from '../../types/command.js'
import { codexEngine } from '../../../../../runtime/tools/builtins/codex-image.js'
import {
  MAX_ENGINE_PROMPT,
  resolveImageJob,
} from '../../../../../runtime/tools/builtins/image-engine.js'

const NO_ENGINE =
  'No picture engine is set up on this computer. Install Codex and sign in with ChatGPT, or ask in chat for a drawing in code.'

function slugOf(brief: string): string {
  const s = brief
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 40)
    .replace(/-+$/g, '')
  return s || 'picture'
}

// Writes to <slug>.png, else <slug>-2.png, -3 ... Never overwrites ('wx' fails if the file exists).
async function saveNew(dir: string, slug: string, bytes: Buffer): Promise<string> {
  for (let n = 1; n < 1000; n++) {
    const path = join(dir, n === 1 ? `${slug}.png` : `${slug}-${n}.png`)
    let handle
    try {
      handle = await open(path, 'wx')
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code === 'EEXIST') continue
      throw e
    }
    try {
      await handle.writeFile(bytes)
    } finally {
      await handle.close()
    }
    return path
  }
  throw new Error('too many pictures with this name')
}

async function run(brief: string, signal: AbortSignal): Promise<string> {
  const outcome = await resolveImageJob({
    engines: [codexEngine],
    prompt: brief,
    signal,
    progress: () => {},
    now: () => Date.now(),
  })
  if (signal.aborted) return 'Stopped. No picture was saved.'
  if (outcome.kind === 'no_engine') return NO_ENGINE
  if (outcome.kind !== 'image') return outcome.message
  const base64 = outcome.image.replace(/^data:image\/png;base64,/, '')
  try {
    const path = await saveNew(getCwdState(), slugOf(brief), Buffer.from(base64, 'base64'))
    return `Picture saved (${outcome.width}x${outcome.height}): ${path}`
  } catch (e) {
    return `The picture was made but could not be saved: ${e instanceof Error ? e.message : String(e)}`
  }
}

function ImageProgress({
  brief,
  onDone,
}: {
  brief: string
  onDone: (result?: string, options?: { display?: 'system' }) => void
}) {
  const controller = React.useMemo(() => new AbortController(), [])
  React.useEffect(() => {
    let finished = false
    run(brief, controller.signal)
      .catch(e => (e instanceof Error ? e.message : String(e)))
      .then(message => {
        finished = true
        onDone(message, { display: 'system' })
      })
    return () => {
      if (!finished) controller.abort()
    }
  }, [brief, controller, onDone])
  return (
    <Box
      flexDirection="column"
      paddingLeft={2}
      tabIndex={0}
      autoFocus={true}
      onKeyDown={(e: { key: string; ctrl?: boolean; preventDefault: () => void }) => {
        if (e.key === 'escape' || (e.ctrl && e.key === 'c')) {
          e.preventDefault()
          controller.abort()
        }
      }}
    >
      <Text color="warning">Making the picture…</Text>
      <Text dimColor>Esc to stop</Text>
    </Box>
  )
}

// The person typing the command is the confirmation: no model, no extra prompt.
export const call: LocalJSXCommandCall = async (onDone, _context, args) => {
  const brief = String(args ?? '').trim()
  if (!brief) {
    onDone('Usage: /image <what to draw>', { display: 'system' })
    return null
  }
  if (brief.length > MAX_ENGINE_PROMPT) {
    onDone(`That brief is too long (max ${MAX_ENGINE_PROMPT} characters).`, { display: 'system' })
    return null
  }
  return <ImageProgress brief={brief} onDone={onDone} />
}
