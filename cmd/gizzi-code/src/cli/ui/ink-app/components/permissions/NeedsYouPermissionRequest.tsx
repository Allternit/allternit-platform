import * as React from 'react'
import { useState } from 'react'
import { Box, Text } from '../../ink'
import { useTerminalSize } from '../../hooks/useTerminalSize'
import { needsYouView, type NeedsYouReply } from '../../utils/needsYou'
import TextInput from '../TextInput'
import { Select } from '../CustomSelect/select'
import { PermissionDialog } from './PermissionDialog'
import type { PermissionRequestProps } from './PermissionRequest'

/**
 * Agent-browser needs-you prompt (computer.credential / human / block):
 * the summary line, one selectable button per `metadata.options` entry, and,
 * when `metadata.secret` is set, a masked field. The typed value only lives in
 * this component's state until it is sent as the reply's `answer`; it is never
 * rendered (bullets only), put in the transcript, history or any log.
 */
export function NeedsYouPermissionRequest({ toolUseConfirm, onDone, workerBadge }: PermissionRequestProps) {
  const view = needsYouView(toolUseConfirm.tool.name, toolUseConfirm.input)
  const [stage, setStage] = useState<'pick' | 'secret'>('pick')
  const [pending, setPending] = useState<NeedsYouReply>('once')
  const [secret, setSecret] = useState('')
  const [cursor, setCursor] = useState(0)
  const { columns } = useTerminalSize()

  const send = (reply: NeedsYouReply, answer?: string) => {
    onDone()
    if (reply === 'reject') return toolUseConfirm.onReject()
    toolUseConfirm.onNeedsYou?.(reply, answer)
  }

  const pick = (value: string) => {
    const index = Number(value)
    const option = view.options[index]
    if (!option) return
    if (option.reply === 'reject' || !view.secret) return send(option.reply)
    setPending(option.reply)
    setStage('secret')
  }

  return (
    <PermissionDialog title="Needs you" subtitle={undefined} workerBadge={workerBadge}>
      <Box flexDirection="column" paddingX={1}>
        <Text bold>{view.summary}</Text>
        {stage === 'pick' ? (
          <Box marginTop={1}>
            <Select
              options={view.options.map((o, i) => ({ label: o.label, value: String(i) }))}
              onChange={pick}
              onCancel={() => send('reject')}
            />
          </Box>
        ) : (
          <Box marginTop={1} flexDirection="column">
            <Text dimColor>{view.secretLabel} (hidden) · Enter send · Esc stop</Text>
            <Box>
              <Text>{'> '}</Text>
              <TextInput
                value={secret}
                onChange={setSecret}
                onSubmit={value => value && send(pending, value)}
                onExit={() => send('reject')}
                focus={true}
                showCursor={true}
                mask="•"
                columns={Math.max(10, columns - 6)}
                cursorOffset={cursor}
                onChangeCursorOffset={setCursor}
              />
            </Box>
          </Box>
        )}
      </Box>
    </PermissionDialog>
  )
}
