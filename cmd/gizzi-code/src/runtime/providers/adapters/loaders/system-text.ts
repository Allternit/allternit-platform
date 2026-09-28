/** The system messages of the turn, as one text. */
export function extractSystemText(prompt: any[]): string {
  return prompt
    .filter((msg) => msg?.role === "system")
    .map((msg) =>
      typeof msg.content === "string"
        ? msg.content
        : Array.isArray(msg.content)
          ? msg.content.map((p: any) => (p?.type === "text" ? String(p.text ?? "") : "")).join("")
          : "",
    )
    .filter(Boolean)
    .join("\n\n")
}
