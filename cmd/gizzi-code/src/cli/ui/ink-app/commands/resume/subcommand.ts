/**
 * /resume owns resume-now and keep-going (overhaul plan D4): `/resume now` and
 * `/resume keep-going` run them; the old names stay as hidden aliases.
 */
export type ResumeSubcommand = 'now' | 'keep-going'

export function resumeSubcommand(args: string | undefined): ResumeSubcommand | undefined {
  const word = (args ?? '').trim().toLowerCase()
  return word === 'now' || word === 'keep-going' ? word : undefined
}
