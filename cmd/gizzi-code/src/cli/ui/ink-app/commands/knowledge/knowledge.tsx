import * as React from 'react'
import { useEffect, useState } from 'react'
import type { CommandResultDisplay } from '../../commands'
import { Select } from '../../components/CustomSelect/select'
import { Dialog } from '../../components/design-system/Dialog'
import { Box, Text } from '../../ink'
import type { LocalJSXCommandCall } from '../../types/command'
import { logError } from '../../utils/log.js'
import { runKnowledge } from '../../../../../runtime/knowledge/client.js'

type Question = { text: string; answer: (yes: boolean) => void }

/**
 * /knowledge — list, show, approve, reject, dismiss, activate. Turning on an
 * approved skill (approve on a skill proposal, or activate) is the
 * skill_manage ask: a yes/no dialog here, unless the permission rule says
 * allow or deny. Everything else prints its text and closes.
 */
function KnowledgeCommand({
  argv,
  onDone,
}: {
  argv: string[]
  onDone: (result?: string, options?: { display?: CommandResultDisplay }) => void
}): React.ReactNode {
  const [question, setQuestion] = useState<Question | null>(null)

  useEffect(() => {
    let finished = false
    const confirm = (text: string) =>
      new Promise<boolean>((resolve) => {
        setQuestion({
          text,
          answer: (yes) => {
            setQuestion(null)
            resolve(yes)
          },
        })
      })
    runKnowledge(argv, { confirm })
      .then((text) => {
        finished = true
        onDone(text, { display: 'system' })
      })
      .catch((err) => {
        logError(err)
        if (!finished) onDone(`Knowledge failed: ${err instanceof Error ? err.message : String(err)}`, { display: 'system' })
      })
  }, [])

  if (!question) return null
  return (
    <Dialog title="Turn on skill" onCancel={() => question.answer(false)} color="remember">
      <Box flexDirection="column" gap={1}>
        <Text>{question.text}</Text>
        <Select
          options={[
            { label: 'Turn it on', value: 'yes' },
            { label: 'Not now', value: 'no' },
          ]}
          onChange={(value: string) => question.answer(value === 'yes')}
          onCancel={() => question.answer(false)}
        />
      </Box>
    </Dialog>
  )
}

export const call: LocalJSXCommandCall = async (onDone, _context, args) => {
  return <KnowledgeCommand argv={String(args ?? '').trim().split(/\s+/).filter(Boolean)} onDone={onDone} />
}
