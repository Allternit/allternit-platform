import React from 'react'
import { MessageResponse } from '../../components/MessageResponse.js'
import { Text } from '../../ink.js'
import { cronToHuman } from '../../utils/cron.js'
import type { CreateOutput } from './CronCreateTool.js'
import type { DeleteOutput } from './CronDeleteTool.js'
import type { ListOutput } from './CronListTool.js'

export function renderCreateToolUseMessage(input: {
  cron?: string
  prompt?: string
}): React.ReactNode {
  return input.cron ? cronToHuman(input.cron) : ''
}

export function renderCreateResultMessage(output: CreateOutput): React.ReactNode {
  return (
    <MessageResponse>
      <Text>
        Scheduled <Text bold>{output.id}</Text> · {output.humanSchedule}
        {output.recurring ? '' : ' · once'}
        <Text dimColor>{output.durable ? ' · durable' : ' · this session'}</Text>
      </Text>
    </MessageResponse>
  )
}

export function renderDeleteToolUseMessage(input: { id?: string }): React.ReactNode {
  return input.id ?? ''
}

export function renderDeleteResultMessage(output: DeleteOutput): React.ReactNode {
  return (
    <MessageResponse>
      <Text>Cancelled {output.id}</Text>
    </MessageResponse>
  )
}

export function renderListToolUseMessage(): React.ReactNode {
  return ''
}

export function renderListResultMessage(output: ListOutput): React.ReactNode {
  if (output.jobs.length === 0) {
    return (
      <MessageResponse>
        <Text dimColor>No scheduled jobs</Text>
      </MessageResponse>
    )
  }
  return (
    <MessageResponse>
      <Text>
        {output.jobs.length} scheduled {output.jobs.length === 1 ? 'job' : 'jobs'}
        <Text dimColor>
          {' · '}
          {output.jobs.map(j => `${j.id} ${j.humanSchedule}`).join(', ')}
        </Text>
      </Text>
    </MessageResponse>
  )
}
