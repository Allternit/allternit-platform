/**
 * Prompt-gap primitives (G3 contract): the shapes every later lane builds on.
 *
 * One place for the parameters and result metadata of the nine new tools
 * (worktree_enter, worktree_exit, second_opinion, pr_comments, status_update,
 * feedback_note, user_location, voice_control, end_conversation), the
 * `resource` extension (search, `file:` refs, a continue cursor, citations)
 * and the optional `label` on long-running tool calls.
 *
 * Each tool has a gated stub in tools/builtins/<id-with-dashes>.ts that
 * exports `available` (false until its lane lands). The registry, the CLI
 * bridge and the tool families list a tool only while its module says
 * `available = true`, so the contract never shows a tool that does nothing.
 *
 *   worktree_enter / worktree_exit   G5 (PR1)
 *   second_opinion / pr_comments     G5 (PR6)
 *   status_update / feedback_note    G4 (PR8b, PR11c)
 *   label                            G4 (PR8a)
 *   resource search / file: / cite   G6 (PR3)
 *   user_location                    G7 (PR4)
 *   voice_control                    G7 (PR9)
 *   end_conversation                 G7 (PR11g)
 *
 * Only zod, no runtime imports, so the Ink adapters, the CLI bridge and the
 * app parsers can share it.
 */
import z from "zod/v4"

// ---------------------------------------------------------------------------
// Tool ids
// ---------------------------------------------------------------------------

export const WORKTREE_ENTER_TOOL_ID = "worktree_enter"
export const WORKTREE_EXIT_TOOL_ID = "worktree_exit"
export const SECOND_OPINION_TOOL_ID = "second_opinion"
export const PR_COMMENTS_TOOL_ID = "pr_comments"
export const STATUS_UPDATE_TOOL_ID = "status_update"
export const FEEDBACK_NOTE_TOOL_ID = "feedback_note"
export const USER_LOCATION_TOOL_ID = "user_location"
export const VOICE_CONTROL_TOOL_ID = "voice_control"
export const END_CONVERSATION_TOOL_ID = "end_conversation"

export const GAP_TOOL_IDS = [
  WORKTREE_ENTER_TOOL_ID,
  WORKTREE_EXIT_TOOL_ID,
  SECOND_OPINION_TOOL_ID,
  PR_COMMENTS_TOOL_ID,
  STATUS_UPDATE_TOOL_ID,
  FEEDBACK_NOTE_TOOL_ID,
  USER_LOCATION_TOOL_ID,
  VOICE_CONTROL_TOOL_ID,
  END_CONVERSATION_TOOL_ID,
] as const
export type GapToolId = (typeof GAP_TOOL_IDS)[number]

/**
 * Deferred families for the new tools (primitives/tool-families.ts).
 * status_update is always loaded: long work needs it without a search.
 */
export const GAP_TOOL_FAMILIES = ["code-review", "location", "voice", "session"] as const
export type GapToolFamily = (typeof GAP_TOOL_FAMILIES)[number]

/**
 * Tools whose result metadata draws a card in the apps: all nine. Must match
 * the gap ids in the API's METADATA_CARD_TOOLS (v1_routes.rs) and the
 * CANONICAL list in integrations/cli-bridge/results.ts (literals there).
 */
export const GAP_CARD_TOOL_IDS = [
  SECOND_OPINION_TOOL_ID,
  PR_COMMENTS_TOOL_ID,
  STATUS_UPDATE_TOOL_ID,
  FEEDBACK_NOTE_TOOL_ID,
  USER_LOCATION_TOOL_ID,
  VOICE_CONTROL_TOOL_ID,
  END_CONVERSATION_TOOL_ID,
  WORKTREE_ENTER_TOOL_ID,
  WORKTREE_EXIT_TOOL_ID,
] as const

/** Every result carries this: false when the tool refused or failed (the text says why). */
const ok = z.boolean()

// ---------------------------------------------------------------------------
// label (PR8a): optional on long-running tool calls only (decision 14)
// ---------------------------------------------------------------------------

export const LABEL_MAX_CHARS = 60
export const LABEL_WORDS = { min: 3, max: 8 } as const

/** A few words for the activity row ("Running the API tests"). Stripped before execute. */
export const ToolCallLabel = z
  .string()
  .trim()
  .min(1)
  .max(LABEL_MAX_CHARS)
  .describe("3 to 8 words for the activity row, such as \"Running the API tests\"")
export type ToolCallLabel = z.infer<typeof ToolCallLabel>

/** Spread into a long-running tool's parameters: `z.object({ ...LabelField, ... })`. */
export const LabelField = { label: ToolCallLabel.optional() }

// ---------------------------------------------------------------------------
// worktree_enter / worktree_exit (PR1)
// ---------------------------------------------------------------------------

export const WorktreeEnterInput = z.object({
  name: z.string().min(1).max(64).optional().describe("Short name for the worktree and its branch; generated when left out"),
  base: z.string().min(1).max(200).optional().describe("Branch or commit to start from; the current HEAD when left out"),
})
export type WorktreeEnterInput = z.infer<typeof WorktreeEnterInput>

export const WorktreeEnterMetadata = z.object({
  ok,
  path: z.string().optional(),
  branch: z.string().optional(),
  base: z.string().optional(),
  /** The session that created it; only that session may remove it. */
  created_by: z.string().optional(),
})
export type WorktreeEnterMetadata = z.infer<typeof WorktreeEnterMetadata>

export const WorktreeExitInput = z.object({
  action: z.enum(["keep", "remove"]).describe("keep the worktree on disk, or remove it (only one this session created, with no unpushed work)"),
})
export type WorktreeExitInput = z.infer<typeof WorktreeExitInput>

export const WorktreeExitMetadata = z.object({
  ok,
  action: z.enum(["keep", "remove"]),
  /** What actually happened: a remove of a tree with unpushed work is kept instead. */
  result: z.enum(["kept", "removed", "refused"]).optional(),
  path: z.string().optional(),
  branch: z.string().optional(),
  /** Why a remove was refused or turned into keep. */
  reason: z.enum(["unpushed_work", "uncommitted_changes", "not_created_here", "not_in_worktree"]).optional(),
})
export type WorktreeExitMetadata = z.infer<typeof WorktreeExitMetadata>

// ---------------------------------------------------------------------------
// second_opinion (PR6): advisory review on a deeper tier (decision 13)
// ---------------------------------------------------------------------------

export const SecondOpinionInput = z.object({
  kind: z.enum(["plan", "diff"]).describe("Review a plan before building, or a diff before claiming done"),
  subject: z.string().min(1).max(40_000).describe("The plan text, or a summary of the change"),
  evidence: z.string().max(40_000).optional().describe("Test output, commands run, or the diff itself"),
  question: z.string().max(2_000).optional().describe("What you most want checked"),
})
export type SecondOpinionInput = z.infer<typeof SecondOpinionInput>

export const SecondOpinionVerdict = z.enum(["agree", "concerns", "disagree"])
export type SecondOpinionVerdict = z.infer<typeof SecondOpinionVerdict>

export const SecondOpinionMetadata = z.object({
  ok,
  kind: z.enum(["plan", "diff"]),
  verdict: SecondOpinionVerdict.optional(),
  /** Short reasons, most important first. */
  reasons: z.array(z.string()).optional(),
  /** The tier the review ran on. */
  tier: z.string().optional(),
})
export type SecondOpinionMetadata = z.infer<typeof SecondOpinionMetadata>

// ---------------------------------------------------------------------------
// pr_comments (PR6): list and reply
// ---------------------------------------------------------------------------

export const PrCommentsInput = z.object({
  action: z.enum(["list", "reply"]).describe("list the review comments on a pull request, or reply to one"),
  pr: z.union([z.number().int().positive(), z.string().min(1)]).optional().describe("PR number or URL; the current branch's PR when left out"),
  comment_id: z.string().min(1).optional().describe("The comment to reply to (reply)"),
  body: z.string().min(1).max(10_000).optional().describe("The reply text (reply)"),
  unresolved_only: z.boolean().optional().describe("Only threads not yet resolved (list)"),
})
export type PrCommentsInput = z.infer<typeof PrCommentsInput>

export const PrComment = z.object({
  id: z.string(),
  author: z.string(),
  body: z.string(),
  path: z.string().optional(),
  line: z.number().int().optional(),
  resolved: z.boolean().optional(),
  url: z.string().optional(),
})
export type PrComment = z.infer<typeof PrComment>

export const PrCommentsMetadata = z.object({
  ok,
  action: z.enum(["list", "reply"]),
  pr: z.number().int().optional(),
  url: z.string().optional(),
  comments: z.array(PrComment).optional(),
  /** The id of the reply that was posted. */
  reply_id: z.string().optional(),
})
export type PrCommentsMetadata = z.infer<typeof PrCommentsMetadata>

// ---------------------------------------------------------------------------
// status_update (PR8b): short progress for long work; publishes session.status
// ---------------------------------------------------------------------------

export const STATUS_NOTE_MAX_CHARS = 200

export const StatusUpdateInput = z.object({
  phase: z.string().min(1).max(40).describe("The current phase in a few words, such as \"Running tests\""),
  note: z.string().max(STATUS_NOTE_MAX_CHARS).optional().describe("One short sentence of progress"),
})
export type StatusUpdateInput = z.infer<typeof StatusUpdateInput>

export const StatusUpdateMetadata = z.object({
  ok,
  phase: z.string(),
  note: z.string().optional(),
  /** Epoch ms the status was published. */
  at: z.number().optional(),
})
export type StatusUpdateMetadata = z.infer<typeof StatusUpdateMetadata>

// ---------------------------------------------------------------------------
// feedback_note (PR11c): a diagnostic note kept on the device
// ---------------------------------------------------------------------------

export const FeedbackNoteInput = z.object({
  kind: z.enum(["bug", "confusion", "missing", "other"]).describe("What sort of problem this is"),
  summary: z.string().min(1).max(300).describe("One line: what went wrong"),
  details: z.string().max(4_000).optional().describe("Steps, errors and context, with no secrets"),
})
export type FeedbackNoteInput = z.infer<typeof FeedbackNoteInput>

export const FeedbackNoteMetadata = z.object({
  ok,
  /** Local note id; nothing leaves the device until the person sends it. */
  note_id: z.string().optional(),
  kind: z.enum(["bug", "confusion", "missing", "other"]).optional(),
  /** One-line summary for the card and the Diagnostics list. */
  summary: z.string().optional(),
  /** Epoch milliseconds. */
  created_at: z.number().optional(),
})
export type FeedbackNoteMetadata = z.infer<typeof FeedbackNoteMetadata>

// ---------------------------------------------------------------------------
// user_location (PR4): a client round trip, approximate by default
// ---------------------------------------------------------------------------

export const LocationPrecision = z.enum(["approximate", "precise"])
export type LocationPrecision = z.infer<typeof LocationPrecision>

export const UserLocationInput = z.object({
  precision: LocationPrecision.optional().describe("approximate (default, city level) or precise (asks the person every time)"),
  reason: z.string().min(1).max(200).describe("Why you need it, shown to the person"),
})
export type UserLocationInput = z.infer<typeof UserLocationInput>

export const UserLocationMetadata = z.object({
  ok,
  precision: LocationPrecision.optional(),
  /** What happened on the client. */
  status: z.enum(["granted", "denied", "unavailable", "timeout", "requested"]).optional(),
  city: z.string().optional(),
  region: z.string().optional(),
  country: z.string().optional(),
  time_zone: z.string().optional(),
  /** Precise only. */
  latitude: z.number().optional(),
  longitude: z.number().optional(),
  /** Meters. */
  accuracy: z.number().optional(),
})
export type UserLocationMetadata = z.infer<typeof UserLocationMetadata>

// ---------------------------------------------------------------------------
// voice_control (PR9): language, speed, reply mode, end call
// ---------------------------------------------------------------------------

export const VOICE_SPEED = { min: 0.5, max: 2, default: 1 } as const

export const VoiceControlInput = z.object({
  action: z.enum(["set_language", "set_speed", "set_reply_mode", "end_call"]).describe("What to change in this voice session"),
  language: z.string().min(2).max(35).optional().describe("BCP 47 tag such as \"es\" or \"pt-BR\" (set_language)"),
  speed: z.number().min(VOICE_SPEED.min).max(VOICE_SPEED.max).optional().describe("Speaking rate, 0.5 to 2 (set_speed)"),
  reply_mode: z.enum(["voice", "text", "both"]).optional().describe("How replies come back (set_reply_mode)"),
})
export type VoiceControlInput = z.infer<typeof VoiceControlInput>

export const VoiceControlMetadata = z.object({
  ok,
  action: z.enum(["set_language", "set_speed", "set_reply_mode", "end_call"]),
  language: z.string().optional(),
  speed: z.number().optional(),
  reply_mode: z.enum(["voice", "text", "both"]).optional(),
})
export type VoiceControlMetadata = z.infer<typeof VoiceControlMetadata>

// ---------------------------------------------------------------------------
// end_conversation (PR11g): Gizzi's own chats, after a warning
// ---------------------------------------------------------------------------

export const EndConversationInput = z.object({
  reason: z.enum(["sustained_abuse", "user_requested"]).describe("Why the chat ends"),
  warned: z.boolean().describe("True when you warned the person in an earlier reply"),
  summary: z.string().max(500).optional().describe("One neutral line for the record"),
})
export type EndConversationInput = z.infer<typeof EndConversationInput>

export const EndConversationMetadata = z.object({
  ok,
  reason: z.enum(["sustained_abuse", "user_requested"]).optional(),
  /** The session is now in the ended state; the composer offers a new chat. */
  ended: z.boolean().optional(),
})
export type EndConversationMetadata = z.infer<typeof EndConversationMetadata>

// ---------------------------------------------------------------------------
// resource extension (PR3): search, file: refs, continue cursor, citations
// ---------------------------------------------------------------------------

/** Ref prefixes. `file:` names an attachment or a Files-drive file the session can read. */
export const RESOURCE_REF_PREFIXES = ["out:", "mcp:", "file:"] as const
export const FILE_REF_PREFIX = "file:"

/** The ref grammar for files: `file:<attachment or drive id>`. */
export const FileRef = z
  .string()
  .regex(/^file:(msg|upload|drive):[A-Za-z0-9._~\-/:]+$/)
  .describe("A file ref: \"file:msg:<messageID>:<partIndex>\", \"file:upload:<id>\" or \"file:drive:<id>\"")
export type FileRef = z.infer<typeof FileRef>

export const RESOURCE_SEARCH_MAX_RESULTS = 20

/** Fields `resource` gains (merged into Resource.Input by G6). */
export const ResourceSearchFields = {
  query: z.string().min(1).max(500).optional().describe("Keywords to rank files and outputs by (search)"),
  cursor: z.string().max(500).optional().describe("The `next_cursor` from the last result, to continue (read, search)"),
  max_results: z.number().int().min(1).max(RESOURCE_SEARCH_MAX_RESULTS).optional().describe("Results per page (search)"),
}

export const ResourceAction = z.enum(["list", "read", "find", "search"])
export type ResourceAction = z.infer<typeof ResourceAction>

/** A line range the answer can cite; rendered as a chip that opens the file at those lines. */
export const ResourceCitation = z.object({
  ref: z.string(),
  start_line: z.number().int().min(1),
  end_line: z.number().int().min(1),
})
export type ResourceCitation = z.infer<typeof ResourceCitation>

export const ResourceSearchHit = z.object({
  ref: z.string(),
  title: z.string(),
  /** Why it matched, in a few words. */
  why: z.string().optional(),
  score: z.number().optional(),
  citation: ResourceCitation.optional(),
})
export type ResourceSearchHit = z.infer<typeof ResourceSearchHit>

/** Result metadata fields `resource` gains. */
export const ResourceExtensionMetadata = z.object({
  hits: z.array(ResourceSearchHit).optional(),
  citations: z.array(ResourceCitation).optional(),
  /** Pass back as `cursor` to continue; absent at the end. */
  next_cursor: z.string().optional(),
})
export type ResourceExtensionMetadata = z.infer<typeof ResourceExtensionMetadata>
