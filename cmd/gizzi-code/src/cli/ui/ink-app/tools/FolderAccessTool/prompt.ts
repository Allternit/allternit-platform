// Same name, input and description as the runtime builtin `folder_access`
// (contract: handoffs/system-prompts-2026-11-17/PLAN-wave-c.md, Tools table).
// Lane C's runtime/tools/builtins/folder-access.txt was not committed when this
// adapter was written; once it lands, import it here the way CurrentTimeTool
// imports current-time.txt so the two surfaces cannot drift.

export const FOLDER_ACCESS_TOOL_NAME = 'folder_access'

export const DESCRIPTION =
  'Ask the user for access to folders outside the project, or for permission to delete files in them'

export const PROMPT: string = `Ask the user for access to folders outside the project.

Use this before you read, write or run commands in a folder outside the project the user has not already granted. The user is always asked; nothing is granted without their answer.

Actions:
- request: grant access to 1 to 5 folders for this session. After a grant you can read and write inside those folders.
- request_delete: grant permission to delete files inside 1 to 5 already-granted folders. Without it, a delete in a granted outside folder moves the file to <folder>/_to_delete/ instead of removing it, so the user can review it.

Inputs:
- action: "request" or "request_delete"
- paths: 1 to 5 folder paths (absolute, or relative to the working directory; ~ is the home folder). A file path grants the folder that holds it.
- reason: one short sentence the user sees, saying why you need the access.

Returns {granted: [resolved folder paths]}. If the user says no, the tool fails and nothing is granted; do not retry the same request, ask the user what they want instead.

Folders inside the project need no request.`
