import { UI } from "@/cli/ui"
import { downloadRepo, listModels, verifyRepo, type VerifyFile, type VerifyResult } from "@/runtime/services/localEngine"
import { describeVerification, pullExitCode, type ModelVerification } from "@/runtime/services/modelVerification"

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

export function printVerification(v: ModelVerification | undefined | null) {
  const line = describeVerification(v)
  const style = line.level === "ok" ? UI.Style.TEXT_SUCCESS_BOLD : UI.Style.TEXT_WARNING_BOLD
  UI.println(style + line.text + UI.Style.TEXT_NORMAL)
}

/** gizzi models pull <repo>: queue the download, stream progress, print the verification line. */
export async function pullModel(repo: string, quant: string | undefined, allowUnverified: boolean, revision?: string): Promise<number> {
  const queued = await downloadRepo(repo, quant, { allowUnverified, revision })
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
      UI.error(`Download failed: ${repo}${m.error ? `: ${m.error}` : ""}`)
      return 1
    }
    await sleep(1000)
  }
  if (last) process.stderr.write("\n")
  printVerification(verification)
  return pullExitCode(verification, allowUnverified)
}

function printFailingFiles(files: VerifyFile[] | undefined) {
  for (const f of files ?? []) {
    if (f.ok === false) UI.println(`  ${f.path}${f.reason ? `: ${f.reason}` : ""}`)
  }
}

/** Prints the line for a /models/verify result and returns the exit code. */
export function reportVerify(repo: string, r: VerifyResult): number {
  if (r.kind === "old_engine") {
    printVerification(undefined)
    return 2
  }
  if (r.kind === "not_downloaded") {
    UI.println(UI.Style.TEXT_WARNING_BOLD + `Not downloaded: run gizzi models pull ${repo}` + UI.Style.TEXT_NORMAL)
    return 2
  }
  printVerification(r.data.verification)
  printFailingFiles(r.data.files)
  const failing = (r.data.files ?? []).some((f) => f.ok === false) || r.data.ok === false
  return failing || describeVerification(r.data.verification).unverified ? 2 : 0
}

export async function verifyModel(repo: string, quant?: string, revision?: string): Promise<number> {
  return reportVerify(repo, await verifyRepo(repo, quant, revision))
}
