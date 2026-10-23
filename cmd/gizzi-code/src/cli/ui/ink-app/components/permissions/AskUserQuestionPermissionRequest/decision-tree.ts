import type { Question, QuestionOption } from '../../../tools/AskUserQuestionTool/AskUserQuestionTool'

// Decision trees: an option with `then` asks a follow-up question once it is
// picked. The follow-up is shown as an extra question right after its parent,
// and on submit its answer folds back into the parent's: "Branch → Leaf".

export type FollowUpLink = { parent: string; branch: string }

export const PATH_SEPARATOR = ' → '

function branchWithFollowUp(question: Question, answer: string | undefined): QuestionOption | undefined {
  if (!answer || question.multiSelect) return undefined
  const option = question.options.find(opt => opt.label === answer)
  return option?.then ? option : undefined
}

export function hasFollowUp(question: Question, answer: string | undefined): boolean {
  return !!branchWithFollowUp(question, answer)
}

// The questions to show: each root question, then the follow-up of the branch
// the user picked (if it has one). Follow-up texts are made unique so they
// never collide with another question's answer key.
export function expandQuestions(
  questions: Question[],
  answers: Record<string, string>,
): { display: Question[]; links: Record<string, FollowUpLink> } {
  const display: Question[] = []
  const links: Record<string, FollowUpLink> = {}
  const taken = new Set(questions.map(q => q.question))
  for (const question of questions) {
    display.push(question)
    const branch = branchWithFollowUp(question, answers[question.question])
    if (!branch?.then) continue
    let text = branch.then.question
    while (taken.has(text)) text = `${text} (${branch.label})`
    taken.add(text)
    links[text] = { parent: question.question, branch: branch.label }
    display.push({
      question: text,
      header: 'Then',
      multiSelect: false,
      context: `After "${branch.label}"`,
      options: branch.then.options.map(leaf => ({ ...leaf })),
    })
  }
  return { display, links }
}

// Answers keyed by root question only, with follow-up answers folded into a path.
export function mergeAnswers(
  questions: Question[],
  answers: Record<string, string>,
): Record<string, string> {
  const { links } = expandQuestions(questions, answers)
  const merged: Record<string, string> = {}
  for (const question of questions) {
    const answer = answers[question.question]
    if (answer !== undefined) merged[question.question] = answer
  }
  for (const [text, link] of Object.entries(links)) {
    const leaf = answers[text]
    if (leaf) merged[link.parent] = `${link.branch}${PATH_SEPARATOR}${leaf}`
  }
  return merged
}

export function optionLabel(option: Pick<QuestionOption, 'label' | 'recommended'>): string {
  return option.recommended ? `${option.label} · Agent's pick` : option.label
}

export function optionDescription(option: Pick<QuestionOption, 'description' | 'caution' | 'then'>): string {
  const parts = [option.description]
  if (option.caution) parts.push(`⚠ ${option.caution}`)
  if (option.then) parts.push(`then: ${option.then.question}`)
  return parts.filter(Boolean).join(' · ')
}
