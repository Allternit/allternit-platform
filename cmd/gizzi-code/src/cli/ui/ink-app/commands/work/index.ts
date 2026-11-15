import type { Command } from '../../commands'

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
