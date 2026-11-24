import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { getStatus } from "@/runtime/services/localEngine"

import { spawnSync } from "child_process"
import { hardwareLabel } from "@/runtime/services/studioClient"

function probe(cmd: string, args: string[]): string | undefined {
  try {
    const r = spawnSync(cmd, args, { encoding: "utf8", timeout: 3000 })
    return r.status === 0 && !r.error ? String(r.stdout) : undefined
  } catch {
    return undefined
  }
}

/** Cheap AMD (ROCm) and Intel (XPU) probes. Missing tools just mean "not found". */
export function extraGpus(): { rocm?: string; xpu?: string } {
  const res: { rocm?: string; xpu?: string } = {}
  const rocm = probe("rocminfo", []) ?? probe("rocm-smi", ["--showproductname"])
  if (rocm) res.rocm = (/Marketing Name:\s*(.+)/.exec(rocm)?.[1] ?? /Card (?:series|model):\s*(.+)/i.exec(rocm)?.[1] ?? "AMD GPU").trim()
  const xpu = probe("sycl-ls", []) ?? probe("xpu-smi", ["discovery"])
  if (xpu) res.xpu = (/\[(?:level_zero|opencl):gpu[^\]]*\]\s*(.+)/.exec(xpu)?.[1] ?? /Device Name:\s*(.+)/i.exec(xpu)?.[1] ?? "Intel GPU").trim()
  return res
}

export const HardwareCommand = cmd({
  command: "hardware",
  describe: "show the detected local hardware profile",
  builder: (yargs: Argv) => {
    return yargs.option("json", {
      describe: "output as JSON",
      type: "boolean",
      default: false,
    })
  },
  handler: async (args) => {
    try {
      const status = await getStatus()
      const extra = extraGpus()
      if (args.json) {
        UI.println(JSON.stringify({ ...status, ...(extra.rocm || extra.xpu ? { extra_gpus: extra } : {}) }, null, 2))
        process.exit(0)
      }

      UI.println(UI.Style.TEXT_SUCCESS_BOLD + "Local hardware profile" + UI.Style.TEXT_NORMAL)
      UI.println("")
      UI.println(`  OS:        ${status.platform.os} (${status.platform.arch})`)
      UI.println(`  CPU:       ${status.cpu.model} (${status.cpu.cores}c / ${status.cpu.threads}t)`)
      UI.println(`  RAM:       ${(status.ram.total_bytes / 1e9).toFixed(1)} GB`)
      if (status.gpu && status.gpu.length > 0) {
        const gpu = status.gpu[0]
        const mem = gpu.memory_total_mb ? `${(gpu.memory_total_mb / 1024).toFixed(1)} GB` : "unknown"
        UI.println(`  GPU:       ${gpu.name} (${mem})`)
      }
      if (extra.rocm) UI.println(`  AMD GPU:   ${extra.rocm} (ROCm)`)
      if (extra.xpu) UI.println(`  Intel GPU: ${extra.xpu} (XPU)`)
      UI.println(`  Hardware ID: ${status.hardware_id} (${hardwareLabel(status.hardware_id)})`)
      if (status.apple_chip) {
        UI.println(`  Apple chip:  ${status.apple_chip}`)
      }
      UI.println(`  Backends:  metal=${status.backends.metal}, cuda=${status.backends.cuda}, cpu=${status.backends.cpu_fallback}`)
      process.exit(0)
    } catch (err) {
      UI.error(err instanceof Error ? err.message : "Failed to reach Local Engine")
      process.exit(1)
    }
  },
})
