/** Tool-part → chat SSE frame mapping shared by the agent-chat bridge. */

/**
 * SSE frames for a session tool part — the same wire the web client already
 * parses (Anthropic-style content_block_start tool_use, then tool_result or
 * tool_error). Applies to every provider: SDK-executed tools and CLI-observed
 * tools both land as `tool` parts on the bus. `sent` de-duplicates repeated
 * part updates so each call yields exactly one start and one end.
 */
export function toolFramesForPart(
  part: any,
  messageId: string,
  sent: Map<string, "start" | "end">,
): Array<Record<string, unknown>> {
  const callID = typeof part?.callID === "string" ? part.callID : ""
  if (!callID || sent.get(callID) === "end") return []
  const status = part?.state?.status
  const toolName = typeof part?.tool === "string" ? part.tool : "tool"
  const frames: Array<Record<string, unknown>> = []
  const settled = status === "completed" || status === "error"
  if (!sent.has(callID) && (settled || status === "pending" || status === "running")) {
    frames.push({
      type: "content_block_start",
      messageId,
      content_block: { type: "tool_use", id: callID, name: toolName, input: part?.state?.input ?? {} },
    })
    sent.set(callID, "start")
  }
  if (settled) {
    frames.push(
      status === "completed"
        ? { type: "tool_result", messageId, toolCallId: callID, toolName, result: part?.state?.output ?? "" }
        : { type: "tool_error", messageId, toolCallId: callID, toolName, error: String(part?.state?.error ?? "Tool execution failed") },
    )
    sent.set(callID, "end")
  }
  return frames
}

/**
 * Run usage for the finish frame from gizzi's assistant message info.
 * Input/output whenever present; cached/reasoning tokens and cost only when
 * the provider reported them, so clients can tell "not reported" from zero.
 */
export function usageFromMessageInfo(info: any): Record<string, number> | undefined {
  const tokens = info?.tokens
  if (typeof tokens?.input !== "number" && typeof tokens?.output !== "number") return undefined
  const usage: Record<string, number> = {
    inputTokens: typeof tokens.input === "number" ? tokens.input : 0,
    outputTokens: typeof tokens.output === "number" ? tokens.output : 0,
  }
  const positive = (n: unknown) => (typeof n === "number" && n > 0 ? n : undefined)
  const cacheRead = positive(tokens?.cache?.read)
  const cacheWrite = positive(tokens?.cache?.write)
  const reasoning = positive(tokens?.reasoning)
  const cost = positive(info?.cost)
  if (cacheRead) usage.cacheReadTokens = cacheRead
  if (cacheWrite) usage.cacheWriteTokens = cacheWrite
  if (reasoning) usage.reasoningTokens = reasoning
  if (cost) usage.cost = cost
  return usage
}

/**
 * Artifact kind the chat renders for a generated file's media type.
 * Mirrors `artifact_kind_for_mime` in allternit-api's agent-chat bridge.
 */
export function artifactKindForMime(mime: string): string {
  const m = mime.toLowerCase()
  if (m.startsWith("image/")) return "image"
  if (m.startsWith("audio/")) return "audio"
  if (m.startsWith("video/")) return "video"
  if (m === "text/html") return "html"
  if (m.includes("presentationml") || m.includes("powerpoint")) return "slides"
  if (m.includes("spreadsheetml") || m.includes("ms-excel") || m === "text/csv") return "sheet"
  return "document"
}

/**
 * The `artifact` frame for a generated file part on the assistant message
 * (a model-made image, deck or document). Keyed by the part id so a repeated
 * part update replaces the card instead of adding a second one.
 */
export function fileFrameForPart(part: any, messageId: string): Record<string, unknown> | undefined {
  if (part?.type !== "file" || typeof part?.id !== "string" || typeof part?.url !== "string" || !part.url) return undefined
  const mime = typeof part.mime === "string" && part.mime ? part.mime : "application/octet-stream"
  const filename = typeof part.filename === "string" && part.filename ? part.filename : undefined
  const sourceTitle =
    typeof part.source?.text?.value === "string" && part.source.text.value.trim() ? part.source.text.value.trim() : undefined
  const title = sourceTitle ?? filename
  return {
    type: "artifact",
    messageId,
    artifactId: part.id,
    kind: artifactKindForMime(mime),
    ...(title ? { title } : {}),
    url: part.url,
    mimeType: mime,
    ...(filename ? { filename } : {}),
    ...(typeof part.source?.uri === "string" ? { sourceUri: part.source.uri } : {}),
  }
}

