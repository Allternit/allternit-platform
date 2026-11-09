import type { Argv } from "yargs"
import { Instance } from "@/runtime/context/project/instance"
import { Provider } from "@/runtime/providers/provider"
import { ModelsDev } from "@/runtime/providers/adapters/models"
import { Discovery } from "@/runtime/providers/discovery"
import { ProviderOffer } from "@/runtime/providers/offer"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { EOL } from "os"
import {
  getCatalog,
  refreshCatalog,
  assessRepo,
  recommendRepos,
  downloadRepo,
  type RecommendationIntent,
} from "@/runtime/services/localEngine"

const SOURCE_BADGE: Record<string, string> = {
  subprocess: UI.Style.TEXT_INFO_BOLD + "[cli]" + UI.Style.TEXT_NORMAL,
  local:      UI.Style.TEXT_WARNING_BOLD + "[local]" + UI.Style.TEXT_NORMAL,
  platform:   UI.Style.TEXT_SUCCESS_BOLD + "[platform]" + UI.Style.TEXT_NORMAL,
  plugin:     UI.Style.TEXT_SUCCESS_BOLD + "[plugin]" + UI.Style.TEXT_NORMAL,
}

export const ModelsCommand = cmd({
  command: "models [provider]",
  describe: "list, assess, and manage local models",
  builder: (yargs: Argv) => {
    return yargs
      .positional("provider", {
        describe: "provider ID to filter models by",
        type: "string",
        array: false,
      })
      .option("verbose", {
        describe: "use more verbose model output (includes metadata like costs)",
        type: "boolean",
      })
      .option("refresh", {
        describe: "refresh the models cache from models.dev",
        type: "boolean",
      })
      .option("all", {
        describe: "show every models.dev provider, aggregators like OpenRouter included (default: what the model picker offers)",
        type: "boolean",
        default: false,
      })
      .option("catalog", {
        describe: "show the dynamic Hugging Face catalog from the Local Engine",
        type: "boolean",
      })
      .option("source", {
        describe: "catalog source filter",
        type: "string",
        choices: ["all", "polled", "seed"],
        default: "all",
      })
      .option("recommend", {
        describe: "show models recommended for this machine",
        type: "boolean",
      })
      .option("intent", {
        describe: "recommendation intent",
        type: "string",
        choices: ["balanced", "smartest", "fastest", "lightweight"],
        default: "balanced",
      })
      .option("limit", {
        describe: "max recommendations or catalog entries",
        type: "number",
        default: 10,
      })
      .option("assess", {
        describe: "assess a Hugging Face repo id (e.g. bartowski/Llama-3.2-3B-Instruct-GGUF)",
        type: "string",
      })
      .option("download", {
        describe: "download a model by Hugging Face repo id",
        type: "string",
      })
      .option("run", {
        describe: "download and run a model by Hugging Face repo id",
        type: "string",
      })
      .option("quant", {
        describe: "quantization tag (e.g. Q4_K_M)",
        type: "string",
      })
  },
  handler: async (args) => {
    // Local-engine actions take precedence over the provider listing.
    if (args.catalog) {
      try {
        const result = await getCatalog(args.source as "all" | "polled" | "seed", args.limit)
        UI.println(UI.Style.TEXT_SUCCESS_BOLD + `Catalog (${args.source})` + UI.Style.TEXT_NORMAL)
        for (const m of result.models) {
          UI.println(`  ${m.repo_id}  ${m.downloads.toLocaleString()} downloads · ${m.likes.toLocaleString()} likes · ${m.source}`)
        }
        process.exit(0)
      } catch (err) {
        UI.error(err instanceof Error ? err.message : "Failed to load catalog")
        process.exit(1)
      }
      return
    }

    if (args.refresh && args.catalog) {
      try {
        const result = await refreshCatalog()
        UI.println(UI.Style.TEXT_SUCCESS_BOLD + `Refreshed catalog: ${result.count} models` + UI.Style.TEXT_NORMAL)
        process.exit(0)
      } catch (err) {
        UI.error(err instanceof Error ? err.message : "Failed to refresh catalog")
        process.exit(1)
      }
      return
    }

    if (args.recommend) {
      try {
        const result = await recommendRepos(args.intent as RecommendationIntent, args.limit)
        UI.println(UI.Style.TEXT_SUCCESS_BOLD + `Recommended models (${args.intent})` + UI.Style.TEXT_NORMAL)
        for (const r of result.recommendations) {
          UI.println(`  ${r.repo_id}`)
          UI.println(`    fit: ${r.fit} · ${r.estimated_tok_per_second.context_4k.toFixed(1)} tok/s (4K) · ${(r.estimated_download_bytes / 1e9).toFixed(1)} GB · ${r.confidence}`)
          UI.println(`    ${r.explanation}`)
        }
        process.exit(0)
      } catch (err) {
        UI.error(err instanceof Error ? err.message : "Failed to load recommendations")
        process.exit(1)
      }
      return
    }

    if (args.assess) {
      try {
        const a = await assessRepo(args.assess, args.quant)
        UI.println(UI.Style.TEXT_SUCCESS_BOLD + `Assessment: ${a.repo_id}` + UI.Style.TEXT_NORMAL)
        UI.println(`  fit:        ${a.fit}`)
        UI.println(`  reason:     ${a.fit_reason}`)
        UI.println(`  download:   ${(a.estimated_download_bytes / 1e9).toFixed(2)} GB`)
        UI.println(`  loaded 4K:  ${(a.estimated_loaded_bytes / 1e9).toFixed(2)} GB`)
        UI.println(`  tok/s 4K:   ${a.estimated_tok_per_second.context_4k.toFixed(1)}`)
        UI.println(`  backend:    ${a.recommended_backend}`)
        UI.println(`  confidence: ${a.confidence}`)
        process.exit(0)
      } catch (err) {
        UI.error(err instanceof Error ? err.message : "Failed to assess model")
        process.exit(1)
      }
      return
    }

    if (args.download) {
      try {
        await downloadRepo(args.download, args.quant)
        UI.println(UI.Style.TEXT_SUCCESS_BOLD + `Download queued: ${args.download}` + UI.Style.TEXT_NORMAL)
        process.exit(0)
      } catch (err) {
        UI.error(err instanceof Error ? err.message : "Failed to queue download")
        process.exit(1)
      }
      return
    }

    if (args.run) {
      try {
        await downloadRepo(args.run, args.quant)
        UI.println(UI.Style.TEXT_SUCCESS_BOLD + `Download queued: ${args.run}` + UI.Style.TEXT_NORMAL)
        UI.println(UI.Style.TEXT_WARNING_BOLD + "Use the Model Lab UI or `gizzi models` to launch the runtime once ready." + UI.Style.TEXT_NORMAL)
        process.exit(0)
      } catch (err) {
        UI.error(err instanceof Error ? err.message : "Failed to queue download")
        process.exit(1)
      }
      return
    }

    if (args.refresh) {
      await ModelsDev.refresh()
      UI.println(UI.Style.TEXT_SUCCESS_BOLD + "Models cache refreshed" + UI.Style.TEXT_NORMAL)
    }

    await Instance.provide({
      directory: process.cwd(),
      async fn() {
        const modelsDev = await ModelsDev.get()
        const configured = await Provider.list()

        // Auto-discovered CLI/local providers
        const discovered = await Discovery.run()
        const discoveredMap: Record<string, typeof discovered[0]> = {}
        for (const dp of discovered) discoveredMap[dp.id] = dp

        function printModels(providerID: string, verbose?: boolean) {
          const devProvider = modelsDev[providerID]
          const configuredProvider = configured[providerID]
          const provider = configuredProvider || devProvider
          if (provider) {
            const sortedModels = Object.entries(provider.models).sort(([a], [b]) => a.localeCompare(b))
            for (const [modelID, model] of sortedModels) {
              process.stdout.write(`${providerID}/${modelID}`)
              process.stdout.write(EOL)
              if (verbose) {
                process.stdout.write(JSON.stringify(model, null, 2))
                process.stdout.write(EOL)
              }
            }
            return
          }

          // Discovered but not in models.dev (CLI/local providers)
          const dp = discoveredMap[providerID]
          if (dp) {
            const badge = SOURCE_BADGE[dp.source] ?? ""
            for (const m of dp.models) {
              process.stdout.write(`${providerID}/${m.id}  ${badge}`)
              process.stdout.write(EOL)
              if (verbose) {
                process.stdout.write(JSON.stringify({ context: m.context, output: m.output }, null, 2))
                process.stdout.write(EOL)
              }
            }
          }
        }

        // Build combined provider ID list
        const allProviderIDs = new Set<string>([
          ...(args.all ? Object.keys(modelsDev) : Object.keys(configured)),
          ...Object.keys(discoveredMap),
        ])

        if (args.provider) {
          if (!allProviderIDs.has(args.provider)) {
            UI.error(`Provider not found: ${args.provider}`)
            return
          }
          printModels(args.provider, args.verbose)
          return
        }

        // The picker's rule (runtime/providers/offer.ts), same as the TUI and
        // Desktop: Allternit Cloud, your subscriptions, API keys, local;
        // aggregators (OpenRouter) only with --all or --provider.
        const candidate = (id: string) => {
          const dp = discoveredMap[id]
          const info = configured[id]
          return {
            id,
            source: dp?.source ?? info?.source,
            auth_type: dp?.auth_type ?? info?.auth_type,
            baseURL: dp?.base_url ?? (info?.options?.baseURL as string | undefined),
            options: dp?.options ?? info?.options,
          }
        }
        // A key or sign-in on file: an env var, `gizzi connect`, or one in gizzi.json.
        const connected = (id: string) => {
          if (discoveredMap[id]) return true
          const info = configured[id]
          if (!info) return false
          return (
            info.source === "env" ||
            info.source === "api" ||
            Boolean(info.key || info.token || info.options?.apiKey) ||
            info.env.some((name) => Boolean(process.env[name]))
          )
        }
        const providerIDs = [...allProviderIDs]
          .filter((id) => args.all || ProviderOffer.isOffered(candidate(id), { connected: connected(id) }))
          .sort((a, b) => {
            const d = ProviderOffer.rank(candidate(a)) - ProviderOffer.rank(candidate(b))
            if (d !== 0) return d
            return a.localeCompare(b)
          })

        let lastGroup = ""
        for (const providerID of providerIDs) {
          const group = ProviderOffer.TITLE[ProviderOffer.kind(candidate(providerID))]
          if (group !== lastGroup) {
            UI.println(UI.Style.TEXT_DIM + `# ${group}` + UI.Style.TEXT_NORMAL)
            lastGroup = group
          }
          printModels(providerID, args.verbose)
        }
      },
    })
  },
})
