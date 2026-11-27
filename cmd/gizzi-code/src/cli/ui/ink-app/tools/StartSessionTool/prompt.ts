export const START_SESSION_TOOL_NAME = 'start_session'

export const DESCRIPTION = 'Start a coding session on this computer or another one of the user\'s computers'

export const PROMPT = `Start a new coding session in a folder, on this computer or another computer the user owns, and (optionally) give it a first task.

- device: a computer's name or id from \`gizzi start --list\`; leave it out for this computer.
- folder: an absolute folder path on that computer.
- runtime: the coding tool to run (default "allternit", Gizzi; also "claude", "codex" and the others that computer has).
- mode: plan, default, auto or bypass. It can never be higher than this session's own mode.
- prompt: the first task for the new session.

The user is always asked first and sees the computer, folder, tool and mode. Sessions started from sessions can only go three levels deep. The result names the new session; tell the user where it is and do not wait for it to finish.`
