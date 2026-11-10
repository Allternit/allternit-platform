import type { ContentBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/messages.js'
import type { Command } from '../commands.js'
import { ALLTERNIT_GATEWAY_BASE } from '@/shared/constants/allternitGateway'

const LOOP_CARD = `
| Field | Answer |
|---|---|
| Outcome | what the user will feel when this works |
| Metric and direction | the one number to move, lower or higher |
| Instrument | the command that prints that number (last number on stdout) |
| Noise floor | spread over unchanged A/A runs |
| Baseline | value now, with n |
| Keep bar | fractional gain required, fixed before iterating |
| Min pairs | paired runs per attempt |
| Budget and stop rule | max iterations; stop after N attempts inside the noise |
| Ledger | every attempt, rejects included |
`

const CARD_ONLY = /^\s*card\s*$/i

const LOOPS_PROMPT = (id: string, args: string) => `
You are in Loop Mode (Loop ID: ${id}). Your instruction is:
"${args}"

First print the Loop Card below, filled in as far as the instruction allows. A blank row means this is not a measured loop yet: say so and run the plain loop. If the Metric and Instrument rows are filled in, tell the user the same fields can be saved on a loop (metric: command, direction, keepBar, noiseRuns, minPairs) so Allternit measures every attempt, keeps or reverts it, and records a ledger row. Read earlier rejected attempts before trying again.
${LOOP_CARD}
Follow this loop execution process:
1. Parse the command/script to run, and the completion or exit condition (if specified).
2. Execute the script or command.
3. Check the exit code or command output:
   - If the condition is met (e.g. exit code 0 or matches expected output), terminate the loop.
   - If the condition is NOT met, wait a moment, adjust the input parameters or codebase if necessary, and re-run.
4. Keep repeating up to a reasonable number of iterations (e.g. max 10 runs) until it succeeds.
5. Report the result of each iteration and the final outcome.

Begin the loop immediately by executing the first run.
`

const loops: Command = {
  type: 'prompt',
  name: 'loops',
  aliases: ['loop'],
  argumentHint: '[interval] <prompt>',
  description: 'Execute a command or action in a loop until a condition is met',
  progressMessage: 'executing loop',
  contentLength: 0,
  source: 'builtin',
  async getPromptForCommand(args): Promise<ContentBlockParam[]> {
    if (CARD_ONLY.test(args)) {
      return [{ type: 'text', text: 'Print this Loop Card as a blank template for the user to fill in, then ask which row to start with:\n' + LOOP_CARD }]
    }
    const id = `loop-${Date.now()}`
    try {
      await fetch(`${ALLTERNIT_GATEWAY_BASE}/api/v1/tasks`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          title: 'Loop: ' + args,
          workspace_id: 'default',
          status: 'in-progress',
          metadata: JSON.stringify({ type: 'loop', id, maxIterations: 10 })
        })
      })
    } catch (e) {
      // Proceed gracefully if API is offline
    }
    return [{ type: 'text', text: LOOPS_PROMPT(id, args) }]
  },
}

export default loops
