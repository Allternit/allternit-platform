/**
 * Reading `/answer <id> …` and listing pending asks as text, for the /answer
 * command (commands/answer). Answers are option labels, the shape every other
 * surface sends to Question.reply.
 *
 * Grammar: one answer per question, separated by ";" when the ask has more
 * than one. An answer is an option number or label; a multi-select question
 * takes several separated by ","; a decision-tree option with a follow-up
 * takes "<choice> > <follow-up choice>". Anything else is a typed answer,
 * unless the question turned typed answers off.
 */
import { Question } from '@/runtime/integrations/question'

type Leaf = { label: string; description?: string; recommended?: boolean }

function pick(token: string, options: readonly Leaf[]): string | undefined {
  const t = token.trim()
  if (/^\d+$/.test(t)) {
    const n = Number(t)
    return n >= 1 && n <= options.length ? options[n - 1]!.label : undefined
  }
  const lower = t.toLowerCase()
  return options.find(o => o.label.toLowerCase() === lower)?.label
}

function one(q: Question.Info, token: string): string[] | string {
  const typed = q.custom !== false
  const t = token.trim()
  if (!t) return `No answer for "${q.question}".`
  if (q.multiple) {
    const out: string[] = []
    for (const part of t.split(',').map(s => s.trim()).filter(Boolean)) {
      const label = pick(part, q.options)
      if (label) out.push(label)
      else if (typed) out.push(part)
      else return `"${part}" is not one of the choices for "${q.question}".`
    }
    return out
  }
  if (Question.isTree(q)) {
    const [first, ...rest] = t.split(/\s*(?:>|→)\s*/)
    const label = pick(first!, q.options)
    if (!label) return typed ? [t] : `"${first}" is not one of the choices for "${q.question}".`
    const then = q.options.find(o => o.label === label)?.then
    const next = rest.join(' > ').trim()
    if (!then || !next) return [label]
    const follow = pick(next, then.options)
    if (follow) return [label, follow]
    return typed ? [label, next] : `"${next}" is not one of the choices for "${then.question}".`
  }
  const label = pick(t, q.options)
  if (label) return [label]
  if (typed) return [t]
  return `"${t}" is not one of the choices for "${q.question}".`
}

/** The answers for Question.reply, or an error to show. */
export function parseAnswers(questions: readonly Question.Info[], raw: string): { answers: string[][] } | { error: string } {
  const parts = questions.length === 1 ? [raw] : raw.split(';')
  if (parts.length !== questions.length) {
    return { error: `This ask has ${questions.length} questions: give one answer for each, separated by ";".` }
  }
  const answers: string[][] = []
  for (let i = 0; i < questions.length; i++) {
    const result = one(questions[i]!, parts[i]!)
    if (typeof result === 'string') return { error: result }
    answers.push(result)
  }
  return { answers }
}

function ago(created: number | undefined): string {
  if (!created) return ''
  const minutes = Math.max(0, Math.round((Date.now() - created) / 60_000))
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.round(minutes / 60)
  if (hours < 48) return `${hours} h ago`
  return `${Math.round(hours / 24)} days ago`
}

/** One pending ask as text: its questions, numbered choices and the agent's assumption. */
export function formatAsk(request: Question.Request): string {
  const lines = [`${request.id}${request.time ? ` · asked ${ago(request.time.created)}` : ''}`]
  for (const q of request.questions) {
    lines.push(`  ${q.question}${q.multiple ? ' (pick any)' : ''}`)
    q.options.forEach((o, i) => {
      lines.push(`    ${i + 1}. ${o.label}${o.recommended ? ' (recommended)' : ''}${o.description ? ` — ${o.description}` : ''}`)
      if (o.then) lines.push(`       then: ${o.then.question} (${o.then.options.map(f => f.label).join(' / ')})`)
    })
  }
  if (request.assumption) lines.push(`  Assuming until you answer: ${request.assumption}`)
  return lines.join('\n')
}

export const ANSWER_USAGE =
  'Answer with /answer <id> <choice>: a number, a label or your own words. ' +
  'Several questions: separate answers with ";". Several choices: separate with ",". A follow-up: "2 > 1".'
