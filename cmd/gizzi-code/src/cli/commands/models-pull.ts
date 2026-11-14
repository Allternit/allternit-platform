import { UI } from "@/cli/ui"
import { downloadRepo, listModels } from "@/runtime/services/localEngine"
import { describeVerification, pullExitCode, type ModelVerification } from "@/runtime/services/modelVerification"

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

export function printVerification(v: ModelVerification | undefined | null) {
  const line = describeVerification(v)
  const style = line.level === "ok" ? UI.Style.TEXT_SUCCESS_BOLD : UI.Style.TEXT_WARNING_BOLD
  UI.println(style + line.text + UI.Style.TEXT_NORMAL)
}

/** gizzi models pull <repo>: queue the download, stream progress, print the verification line. */
export async function pullModel(repo: string, quant: string | undefined, allowUnverified: boolean): Promise<number> {
  const queued = await downloadRepo(repo, quant, { allowUnverified })
  let verification = queued.verification
  let last = ""
  for (;;) {
    const { models } = await listModels()
    const m = models.find((x) => x.id === queued.id) ?? models.find((x) => x.name === repo)
    if (!m) break
    const pct = m.total_bytes > 0 ? Math.floor((m.downloaded_bytes / m.total_bytes) * 100) : 0
    const msg = `${m.status} ${pct}% (${(m.downloaded_bytes / 1e6).toFixed(0)} / ${(m.total_bytes / 1e6).toFixed(0)} MB)`
    if (msg !== last) {
      process.stderr.write(`\r${msg}   `)
      last = msg
    }
    if (m.verification) verification = m.verification
    if (m.status === "ready") break
    if (m.status === "failed") {
      process.stderr.write("\n")
      UI.error(`Download failed: ${repo}`)
      return 1
    }
    await sleep(1000)
  }
  if (last) process.stderr.write("\n")
  printVerification(verification)
  return pullExitCode(verification, allowUnverified)
}

const SOURCE_BADGE: Record<string, string> = {
  subprocess: UI.Style.TEXT_INFO_BOLD + "[cli]" + UI.Style.TEXT_NORMAL,
  local:      UI.Style.TEXT_WARNING_BOLD + "[local]" + UI.Style.TEXT_NORMAL,
  platform:   UI.Style.TEXT_SUCCESS_BOLD + "[platform]" + UI.Style.TEXT_NORMAL,
  plugin:     UI.Style.TEXT_SUCCESS_BOLD + "[plugin]" + UI.Style.TEXT_NORMAL,
}

