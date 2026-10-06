import { cmd } from "@/cli/commands/cmd"
import { bootstrap } from "@/cli/bootstrap"
import { UI } from "@/cli/ui"
import { Global } from "@/runtime/context/global"
import { Provider } from "@/runtime/providers/provider"
import { Instance } from "@/runtime/context/project/instance"
import { Installation } from "@/shared/installation"
import fs from "fs"
import path from "path"
import {
  checkCredentialSecurity,
  checkCronDaemon,
  checkGatewayReachability,
  checkProjectInstructions,
  checkVoiceEngine,
  type DoctorCheck,
  checkFactoryEngine,
} from "@/cli/commands/doctorChecks"
import { isDaemonRunning } from "@/runtime/automation/cron/daemon"
import { supervisionState } from "@/runtime/automation/cron/supervision"
import { defaultCredentialDir } from "@/runtime/context/config/credential-store"

function pass(msg: string) {
  UI.println(UI.Style.TEXT_SUCCESS_BOLD + "✓" + UI.Style.TEXT_NORMAL, msg)
}

function fail(msg: string) {
  UI.println(UI.Style.TEXT_DANGER_BOLD + "✗" + UI.Style.TEXT_NORMAL, msg)
}

function warn(msg: string) {
  UI.println(UI.Style.TEXT_WARNING_BOLD + "!" + UI.Style.TEXT_NORMAL, msg)
}

function info(msg: string) {
  UI.println(UI.Style.TEXT_DIM + "  " + msg + UI.Style.TEXT_NORMAL)
}

function header(title: string) {
  UI.println("")
  UI.println(UI.Style.TEXT_INFO_BOLD + `── ${title} ──` + UI.Style.TEXT_NORMAL)
}

function renderCheck(check: DoctorCheck) {
  switch (check.status) {
    case "pass":
      pass(check.message)
      break
    case "fail":
      fail(check.message)
      break
    case "warn":
      warn(check.message)
      break
    default:
      info(check.message)
  }
}

function summarize(checks: DoctorCheck[]) {
  const summary = { pass: 0, warn: 0, fail: 0, info: 0 }
  for (const check of checks) summary[check.status]++
  return summary
}

export const DoctorCommand = cmd({
  command: "doctor",
  describe: "check system health and configuration",
  builder: (yargs) =>
    yargs.option("json", {
      type: "boolean",
      describe: "emit structured JSON instead of formatted text",
      default: false,
    }),
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      const checks: DoctorCheck[] = []

      // ── Runtime ──
      {
        const section = "Runtime"
        const bunVersion = Bun.version
        checks.push(
          bunVersion
            ? { id: "bun", section, status: "pass", message: `Bun ${bunVersion}` }
            : { id: "bun", section, status: "fail", message: "Bun version not detected" },
        )
        try {
          const nodeVersion = process.versions.node
          checks.push(
            nodeVersion
              ? { id: "node-compat", section, status: "pass", message: `Node compatibility: v${nodeVersion}` }
              : { id: "node-compat", section, status: "fail", message: "Node compatibility layer not available" },
          )
        } catch {
          checks.push({ id: "node-compat", section, status: "fail", message: "Node compatibility layer not available" })
        }
      }

      // ── Dependencies ──
      {
        const section = "Dependencies"
        const rg = Bun.which("rg")
        checks.push(
          rg
            ? { id: "ripgrep", section, status: "pass", message: `Ripgrep found: ${rg}` }
            : { id: "ripgrep", section, status: "fail", message: "Ripgrep (rg) not found — file search will be unavailable" },
        )
        const git = Bun.which("git")
        checks.push(
          git
            ? { id: "git", section, status: "pass", message: `Git found: ${git}` }
            : { id: "git", section, status: "fail", message: "Git not found — version control features will be unavailable" },
        )
      }

      // ── Providers ──
      {
        const section = "Providers"
        try {
          const providers = await Provider.list()
          const providerIDs = Object.keys(providers)
          if (providerIDs.length > 0) {
            checks.push({ id: "providers", section, status: "pass", message: `${providerIDs.length} provider(s) configured` })
            for (const id of providerIDs.sort()) {
              const p = providers[id]
              const modelCount = p.models ? Object.keys(p.models).length : 0
              checks.push({ id: `provider:${id}`, section, status: "info", message: `${id} (${modelCount} models)` })
            }
          } else {
            checks.push({ id: "providers", section, status: "fail", message: "No providers configured — run `gizzi connect login`" })
          }
        } catch (e: any) {
          checks.push({ id: "providers", section, status: "fail", message: `Provider check failed: ${e.message}` })
        }
      }

      // ── Database ──
      {
        const section = "Database"
        try {
          const dataDir = Global.Path.data
          const dbPath = path.join(dataDir, "gizzi.db")
          if (fs.existsSync(dbPath)) {
            const stat = fs.statSync(dbPath)
            const sizeMB = (stat.size / 1024 / 1024).toFixed(1)
            checks.push({ id: "database", section, status: "pass", message: `Database exists: ${dbPath} (${sizeMB} MB)` })
          } else if (fs.existsSync(dataDir)) {
            checks.push({ id: "database", section, status: "fail", message: `Database not found at ${dbPath}` })
          } else {
            checks.push({ id: "database", section, status: "fail", message: `Data directory does not exist: ${dataDir}` })
          }
        } catch (e: any) {
          checks.push({ id: "database", section, status: "fail", message: `Database check failed: ${e.message}` })
        }
      }

      // ── Config ──
      {
        const section = "Config"
        try {
          const configDir = Global.Path.config
          if (fs.existsSync(configDir)) {
            checks.push({ id: "config-dir", section, status: "pass", message: `Global config directory exists: ${configDir}` })
          } else {
            checks.push({ id: "config-dir", section, status: "fail", message: `Global config directory not found: ${configDir}` })
          }
        } catch (e: any) {
          checks.push({ id: "config-dir", section, status: "fail", message: `Config check failed: ${e.message}` })
        }
      }

      // ── Credentials (permissions + inline keys) ──
      checks.push(
        ...(await checkCredentialSecurity({
          credentialsPath: path.join(defaultCredentialDir(), "credentials.json"),
          configTomlPath: path.join(Global.Path.config, "config.toml"),
        })),
      )

      // ── Cron daemon ──
      checks.push(
        ...(await checkCronDaemon({
          isRunning: () => isDaemonRunning(3031),
          supervised: () => supervisionState(),
        })),
      )

      // ── Cloud gateway reachability ──
      checks.push(await checkGatewayReachability())
      checks.push(await checkVoiceEngine())

      // ── Allternit Factory engine ──
      checks.push(...(await checkFactoryEngine()))

      // ── Project ──
      {
        const section = "Project"
        try {
          const cwd = process.cwd()
          checks.push(await checkProjectInstructions(cwd))

          const gizziDir = path.join(cwd, ".gizzi")
          if (fs.existsSync(gizziDir)) {
            checks.push({ id: "gizzi-dir", section, status: "pass", message: `.gizzi directory found: ${gizziDir}` })
          } else {
            checks.push({ id: "gizzi-dir", section, status: "info", message: ".gizzi directory not found in current directory" })
          }

          const gitDir = path.join(cwd, ".git")
          if (fs.existsSync(gitDir)) {
            checks.push({ id: "git-repo", section, status: "pass", message: "Git repository detected" })
          } else {
            checks.push({ id: "git-repo", section, status: "info", message: "Not a git repository" })
          }
        } catch (e: any) {
          checks.push({ id: "project", section, status: "fail", message: `Project check failed: ${e.message}` })
        }
      }

      if (args.json) {
        const summary = summarize(checks)
        UI.println(
          JSON.stringify(
            {
              tool: "gizzi-doctor",
              version: Installation.VERSION,
              checks,
              summary: { pass: summary.pass, warn: summary.warn, fail: summary.fail },
            },
            null,
            2,
          ),
        )
        return
      }

      UI.println(UI.Style.TEXT_INFO_BOLD + "gizzi doctor" + UI.Style.TEXT_NORMAL)
      const sections = [...new Set(checks.map((c) => c.section))]
      for (const section of sections) {
        header(section)
        for (const check of checks.filter((c) => c.section === section)) {
          renderCheck(check)
        }
      }
      UI.println("")
    })
  },
})
