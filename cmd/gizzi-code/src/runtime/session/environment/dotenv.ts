/**
 * Parse a code environment's raw `.env` text into variables.
 *
 * Supports what people paste from real .env files:
 *   KEY=value                plain, trailing ` # comment` stripped
 *   export KEY=value         `export ` prefix ignored
 *   KEY="a\nb"               double quotes: \n \r \t \" \\ escapes, may span lines
 *   KEY='literal'            single quotes: literal, may span lines
 *   KEY=`literal`            backticks: literal, may span lines
 *   # comment / blank lines  skipped
 *
 * No `${VAR}` expansion: values are used as written. Lines that are not
 * `NAME=value` with a valid shell name are reported, not guessed at.
 */
export namespace DotEnv {
  export type Result = { vars: Record<string, string>; invalid: { line: number; text: string }[] }

  const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/

  function unescapeDouble(raw: string): string {
    return raw.replace(/\\([nrt"\\$])/g, (_, c: string) => {
      if (c === "n") return "\n"
      if (c === "r") return "\r"
      if (c === "t") return "\t"
      return c
    })
  }

  export function parse(text: string): Result {
    const vars: Record<string, string> = {}
    const invalid: Result["invalid"] = []
    const src = (text ?? "").replace(/\r\n?/g, "\n")
    const lines = src.split("\n")
    let i = 0
    while (i < lines.length) {
      const lineNo = i + 1
      const raw = lines[i]!
      i++
      const trimmed = raw.trim()
      if (!trimmed || trimmed.startsWith("#")) continue
      const body = trimmed.startsWith("export ") ? trimmed.slice(7).trimStart() : trimmed
      const eq = body.indexOf("=")
      if (eq <= 0) {
        invalid.push({ line: lineNo, text: raw })
        continue
      }
      const name = body.slice(0, eq).trim()
      if (!NAME.test(name)) {
        invalid.push({ line: lineNo, text: raw })
        continue
      }
      let rest = body.slice(eq + 1).trimStart()
      const quote = rest[0]
      if (quote === '"' || quote === "'" || quote === "`") {
        // Gather lines until the closing quote (not escaped, for double quotes).
        let value = rest.slice(1)
        const closes = (s: string): number => {
          for (let k = 0; k < s.length; k++) {
            if (quote === '"' && s[k] === "\\") {
              k++
              continue
            }
            if (s[k] === quote) return k
          }
          return -1
        }
        let end = closes(value)
        while (end === -1 && i < lines.length) {
          value += "\n" + lines[i]!
          i++
          end = closes(value)
        }
        if (end === -1) {
          invalid.push({ line: lineNo, text: raw })
          continue
        }
        const inner = value.slice(0, end)
        vars[name] = quote === '"' ? unescapeDouble(inner) : inner
        continue
      }
      // Unquoted: strip an inline comment that follows whitespace.
      const hash = rest.search(/\s#/)
      if (hash !== -1) rest = rest.slice(0, hash)
      vars[name] = rest.trim()
    }
    return { vars, invalid }
  }
}
