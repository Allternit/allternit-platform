import type { Command } from '../../commands'
import { consoleEnabled } from '@/cli/work/console'

/** Work views from Desktop, in the terminal: decisions, PR checks, projects, routines. */
export const decisions = {
  type: 'local-jsx',
  name: 'decisions',
  description: 'Everything agents are waiting on, across all sessions and bots: answer questions, allow or deny (1-9 answer · y/a/n)',
  load: () => import('./decisions.js'),
} satisfies Command

export const pr = {
  type: 'local-jsx',
  name: 'pr',
  description: "This branch's pull request and its live checks (enter opens a check, o opens the PR)",
  load: () => import('./pr.js'),
} satisfies Command

export const projects = {
  type: 'local-jsx',
  name: 'projects',
  description: 'Your projects with their status (on track, at risk, blocked, needs decision) and next action',
  load: () => import('./projects.js'),
} satisfies Command

export const routines = {
  type: 'local-jsx',
  name: 'routines',
  aliases: ['routine'],
  description: 'Your routines with next and last runs (p pauses or resumes)',
  load: () => import('./routines.js'),
} satisfies Command

export const computers = {
  type: 'local-jsx',
  name: 'computers',
  description: 'Your computers grouped, with status and meters; open one for its snapshot, services and ports (read-only)',
  isEnabled: () => consoleEnabled(),
  load: () => import('./computers.js'),
} satisfies Command

/** The Sleep tab of the work views. Reached from the work views, not listed as its own slash command (D11). */
export const sleep = {
  type: 'local-jsx',
  name: 'sleep',
  description: 'Allternit Sleep: last and next run, last report (n run now · p pause/resume)',
  isHidden: true,
  load: () => import('./sleep.js'),
} satisfies Command
