import type { PartialCompactDirection } from '../../types/message.js'

// Aggressive no-tools preamble. The cache-sharing fork path inherits the
// parent's full tool set (required for cache-key match), and on Sonnet 4.6+
// adaptive-thinking models the model sometimes attempts a tool call despite
// the weaker trailer instruction. With maxTurns: 1, a denied tool call means
// no text output → falls through to the streaming fallback (2.79% on 4.6 vs
// 0.01% on 4.5). Putting this FIRST and making it explicit about rejection
// consequences prevents the wasted turn.
const NO_TOOLS_PREAMBLE = `CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.

- Do NOT use Read, Bash, Grep, Glob, Edit, Write, or ANY other tool.
- You already have all the context you need in the conversation above.
- Tool calls will be REJECTED and will waste your only turn — you will fail the task.
- Your entire response must be plain text: an <analysis> block followed by a <summary> block.

`

// Two variants: BASE scopes to "the conversation", PARTIAL scopes to "the
// recent messages". The <analysis> block is a drafting scratchpad that
// formatCompactSummary() strips before the summary reaches context.
const DETAILED_ANALYSIS_INSTRUCTION_BASE = `Before providing your final summary, wrap your analysis in <analysis> tags to organize your thoughts and ensure you've covered all necessary points. In your analysis process:

1. Chronologically analyze each message and section of the conversation. For each section thoroughly identify:
   - The user's explicit requests and intents
   - Your approach to addressing the user's requests
   - Key decisions, technical concepts and code patterns
   - Specific details like:
     - file names
     - full code snippets
     - function signatures
     - file edits
   - Errors that you ran into and how you fixed them
   - Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
2. Double-check for technical accuracy and completeness, addressing each required element thoroughly.`

const DETAILED_ANALYSIS_INSTRUCTION_PARTIAL = `Before providing your final summary, wrap your analysis in <analysis> tags to organize your thoughts and ensure you've covered all necessary points. In your analysis process:

1. Analyze the recent messages chronologically. For each section thoroughly identify:
   - The user's explicit requests and intents
   - Your approach to addressing the user's requests
   - Key decisions, technical concepts and code patterns
   - Specific details like:
     - file names
     - full code snippets
     - function signatures
     - file edits
   - Errors that you ran into and how you fixed them
   - Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
2. Double-check for technical accuracy and completeness, addressing each required element thoroughly.`

const BASE_COMPACT_PROMPT = `Your task is to create a detailed summary of the conversation so far, paying close attention to the user's explicit requests and your previous actions.
This summary should be thorough in capturing technical details, code patterns, and architectural decisions that would be essential for continuing development work without losing context.

${DETAILED_ANALYSIS_INSTRUCTION_BASE}

Inside the <summary> block, write plain text under exactly these five headings, in this order:

## Goal and user requests
What the user is trying to do, and each request they made, in their own words where it matters. Include every correction the user gave and every preference or constraint that must keep applying.

## Work done and current state
What has been done and where things stand now. List each file read, created or changed with its full path and what changed in it. Keep names, numbers, commands and IDs exactly as they appear.

## Errors and failed approaches
Errors met, how each was fixed, and approaches that were tried and did not work, so they are not tried again.

## Promises to the user and open items
Anything you said you would do and have not done yet, questions waiting on the user, and work that is unfinished or blocked.

## Next step
The one next action, matching the user's latest request, with a direct quote of that request. If the last request is finished and nothing is open, say so.

Here is how your output is structured:

<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
## Goal and user requests
[...]

## Work done and current state
[...]

## Errors and failed approaches
[...]

## Promises to the user and open items
[...]

## Next step
[...]
</summary>
</example>

Please provide your summary based on the conversation so far, following this structure and ensuring precision and thoroughness in your response. 

There may be additional summarization instructions provided in the included context. If so, remember to follow these instructions when creating the above summary. Examples of instructions include:
<example>
## Compact Instructions
When summarizing the conversation focus on typescript code changes and also remember the mistakes you made and how you fixed them.
</example>

<example>
# Summary instructions
When you are using compact - please focus on test output and code changes. Include file reads verbatim.
</example>
`

const PARTIAL_COMPACT_PROMPT = `Your task is to create a detailed summary of the RECENT portion of the conversation — the messages that follow earlier retained context. The earlier messages are being kept intact and do NOT need to be summarized. Focus your summary on what was discussed, learned, and accomplished in the recent messages only.

${DETAILED_ANALYSIS_INSTRUCTION_PARTIAL}

Inside the <summary> block, write plain text under exactly these five headings, in this order:

## Goal and user requests
What the user is trying to do, and each request they made, in their own words where it matters. Include every correction the user gave and every preference or constraint that must keep applying.

## Work done and current state
What has been done and where things stand now. List each file read, created or changed with its full path and what changed in it. Keep names, numbers, commands and IDs exactly as they appear.

## Errors and failed approaches
Errors met, how each was fixed, and approaches that were tried and did not work, so they are not tried again.

## Promises to the user and open items
Anything you said you would do and have not done yet, questions waiting on the user, and work that is unfinished or blocked.

## Next step
The one next action, matching the user's latest request, with a direct quote of that request. If the last request is finished and nothing is open, say so.

Here is how your output is structured:

<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
## Goal and user requests
[...]

## Work done and current state
[...]

## Errors and failed approaches
[...]

## Promises to the user and open items
[...]

## Next step
[...]
</summary>
</example>

Please provide your summary based on the RECENT messages only (after the retained earlier context), following this structure and ensuring precision and thoroughness in your response.
`

// 'up_to': model sees only the summarized prefix (cache hit). Summary will
// precede kept recent messages, hence "Context for Continuing Work" section.
const PARTIAL_COMPACT_UP_TO_PROMPT = `Your task is to create a detailed summary of this conversation. This summary will be placed at the start of a continuing session; newer messages that build on this context will follow after your summary (you do not see them here). Summarize thoroughly so that someone reading only your summary and then the newer messages can fully understand what happened and continue the work.

${DETAILED_ANALYSIS_INSTRUCTION_BASE}

Inside the <summary> block, write plain text under exactly these five headings, in this order:

## Goal and user requests
What the user is trying to do, and each request they made, in their own words where it matters. Include every correction the user gave and every preference or constraint that must keep applying.

## Work done and current state
What has been done and where things stand now. List each file read, created or changed with its full path and what changed in it. Keep names, numbers, commands and IDs exactly as they appear.

## Errors and failed approaches
Errors met, how each was fixed, and approaches that were tried and did not work, so they are not tried again.

## Promises to the user and open items
Anything you said you would do and have not done yet, questions waiting on the user, and work that is unfinished or blocked.

## Next step
The one next action, matching the user's latest request, with a direct quote of that request. If the last request is finished and nothing is open, say so.

Here is how your output is structured:

<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
## Goal and user requests
[...]

## Work done and current state
[...]

## Errors and failed approaches
[...]

## Promises to the user and open items
[...]

## Next step
[...]
</summary>
</example>

Please provide your summary following this structure, ensuring precision and thoroughness in your response.
`

const NO_TOOLS_TRAILER =
  '\n\nREMINDER: Do NOT call any tools. Respond with plain text only — ' +
  'an <analysis> block followed by a <summary> block. ' +
  'Tool calls will be rejected and you will fail the task.'

export function getPartialCompactPrompt(
  customInstructions?: string,
  direction: PartialCompactDirection = 'from',
): string {
  const template =
    direction === 'up_to'
      ? PARTIAL_COMPACT_UP_TO_PROMPT
      : PARTIAL_COMPACT_PROMPT
  let prompt = NO_TOOLS_PREAMBLE + template

  if (customInstructions && customInstructions.trim() !== '') {
    prompt += `\n\nAdditional Instructions:\n${customInstructions}`
  }

  prompt += NO_TOOLS_TRAILER

  return prompt
}

export function getCompactPrompt(customInstructions?: string): string {
  let prompt = NO_TOOLS_PREAMBLE + BASE_COMPACT_PROMPT

  if (customInstructions && customInstructions.trim() !== '') {
    prompt += `\n\nAdditional Instructions:\n${customInstructions}`
  }

  prompt += NO_TOOLS_TRAILER

  return prompt
}

/**
 * Formats the compact summary by stripping the <analysis> drafting scratchpad
 * and replacing <summary> XML tags with readable section headers.
 * @param summary The raw summary string potentially containing <analysis> and <summary> XML tags
 * @returns The formatted summary with analysis stripped and summary tags replaced by headers
 */
export function formatCompactSummary(summary: string): string {
  let formattedSummary = summary

  // Strip analysis section — it's a drafting scratchpad that improves summary
  // quality but has no informational value once the summary is written.
  formattedSummary = formattedSummary.replace(
    /<analysis>[\s\S]*?<\/analysis>/,
    '',
  )

  // Extract and format summary section
  const summaryMatch = formattedSummary.match(/<summary>([\s\S]*?)<\/summary>/)
  if (summaryMatch) {
    const content = summaryMatch[1] || ''
    formattedSummary = formattedSummary.replace(
      /<summary>[\s\S]*?<\/summary>/,
      `Summary:\n${content.trim()}`,
    )
  }

  // Clean up extra whitespace between sections
  formattedSummary = formattedSummary.replace(/\n\n+/g, '\n\n')

  return formattedSummary.trim()
}

export function getCompactUserSummaryMessage(
  summary: string,
  suppressFollowUpQuestions?: boolean,
  transcriptPath?: string,
  recentMessagesPreserved?: boolean,
): string {
  const formattedSummary = formatCompactSummary(summary)

  let baseSummary = `This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.

${formattedSummary}`

  if (transcriptPath) {
    baseSummary += `\n\nIf you need specific details from before compaction (like exact code snippets, error messages, or content you generated), read the full transcript at: ${transcriptPath}`
  }

  if (recentMessagesPreserved) {
    baseSummary += `\n\nRecent messages are preserved verbatim.`
  }

  if (suppressFollowUpQuestions) {
    const continuation = `${baseSummary}
Continue the conversation from where it left off without asking the user any further questions. Resume directly — do not acknowledge the summary, do not recap what was happening, do not preface with "I'll continue" or similar. Pick up the last task as if the break never happened.`

    return continuation
  }

  return baseSummary
}
