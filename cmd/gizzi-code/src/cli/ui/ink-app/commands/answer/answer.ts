import { Question } from '@/runtime/integrations/question'
import type { LocalCommandCall } from '../../types/command.js'
import { ANSWER_USAGE, formatAsk, parseAnswers } from '../../tools/AskUserLaterTool/answer.js'
import { ownedSessionIDs, withRuntime } from '../../tools/AskUserLaterTool/session.js'

/**
 * /answer: the terminal's way to answer ask_user_later asks. With no id it
 * lists the asks waiting in this session; `/answer <id> <choice or text>`
 * answers through Question.reply, and the answer reaches the agent as the next
 * turn (hooks/useAskUserLaterAnswers.ts). Only this session's asks are
 * answered here: another session's ask is answered from that session or the app.
 */
export const call: LocalCommandCall = async args => {
  const owned = [...ownedSessionIDs()]
  const pending = await withRuntime(async () =>
    (await Promise.all(owned.map(id => Question.listPendingAsync(id)))).flat(),
  )
  const input = args.trim()
  if (!input) {
    if (!pending.length) return { type: 'text', value: 'No questions are waiting for your answer in this session.' }
    const n = pending.length
    return {
      type: 'text',
      value: `${n} question${n === 1 ? '' : 's'} waiting for your answer:\n\n${pending.map(formatAsk).join('\n\n')}\n\n${ANSWER_USAGE}`,
    }
  }
  const match = /^(\S+)\s*([\s\S]*)$/.exec(input)!
  const id = match[1]!
  const rest = match[2]!.trim()
  const request = pending.find(r => r.id === id)
  if (!request) {
    const ids = pending.map(r => r.id).join(', ')
    return {
      type: 'text',
      value: `No question ${id} is waiting in this session (it may be answered, expired, or from another session).${ids ? ` Waiting here: ${ids}.` : ''}`,
    }
  }
  if (!rest) return { type: 'text', value: `${formatAsk(request)}\n\n${ANSWER_USAGE}` }
  const parsed = parseAnswers(request.questions, rest)
  if ('error' in parsed) return { type: 'text', value: `${parsed.error}\n\n${formatAsk(request)}\n\n${ANSWER_USAGE}` }
  await withRuntime(() => Question.reply({ requestID: request.id, answers: parsed.answers }))
  const summary = request.questions.map((q, i) => Question.formatAnswer(q, parsed.answers[i])).join('; ')
  return { type: 'text', value: `Answered ${id}: ${summary}. The agent gets it as the next turn.` }
}
