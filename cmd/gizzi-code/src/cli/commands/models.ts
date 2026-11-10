import type { Argv } from "yargs"
import { Instance } from "@/runtime/context/project/instance"
import { Provider } from "@/runtime/providers/provider"
import { ModelsDev } from "@/runtime/providers/adapters/models"
import { ModelList } from "@/runtime/providers/model-list"
import { currentDefaultBrain } from "@/runtime/providers/default-brain"
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
      .option("json", {
        describe: "print the sections and models as JSON",
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
      // Make discovery re-read live CLI model lists instead of the cache.
      process.env.GIZZI_MODELS_REFRESH = "1"
      if (!args.json) UI.println(UI.Style.TEXT_SUCCESS_BOLD + "Models cache refreshed" + UI.Style.TEXT_NORMAL)
    }

    await Instance.provide({
      directory: process.cwd(),
      async fn() {
        // With an explicit provider, show it even when the picker would not offer it.
        const sections = await ModelList.build({ all: Boolean(args.all || args.provider) })

        if (args.json) {
          process.stdout.write(JSON.stringify(sections, null, 2) + EOL)
          return
        }

        const configured = args.verbose ? await Provider.list().catch(() => ({}) as Record<string, any>) : {}
        const modelsDev = args.verbose ? await ModelsDev.get().catch(() => ({}) as Record<string, any>) : {}

        if (args.provider) {
          const found = sections.flatMap((s) => s.providers).find((p) => p.id === args.provider)
          if (!found) {
            UI.error(`Provider not found: ${args.provider}`)
            return
          }
        }

        const dim = (text: string) => UI.Style.TEXT_DIM + text + UI.Style.TEXT_NORMAL
        for (const section of sections) {
          const providers = section.providers.filter((p) => !args.provider || p.id === args.provider)
          if (providers.length === 0) continue
          if (!args.provider) UI.println(dim(`# ${section.title}`))
          for (const provider of providers) {
            for (const row of provider.models) {
              const tags: string[] = []
              if (row.default) tags.push("default")
              if (row.current) tags.push("current")
              if (provider.signedIn === false) tags.push("not signed in")
              const ref = row.current ? UI.Style.TEXT_HIGHLIGHT_BOLD + row.ref + UI.Style.TEXT_NORMAL : row.ref
              process.stdout.write(ref + (tags.length ? "  " + dim(tags.join(" ")) : "") + EOL)
              if (args.verbose) {
                const info = configured[provider.id]?.models?.[row.modelID] ?? modelsDev[provider.id]?.models?.[row.modelID]
                const detail = info ?? { context: row.context, output: row.output }
                process.stdout.write(JSON.stringify(detail, null, 2) + EOL)
              }
            }
          }
        }

        if (!args.provider) {
          const brain = await currentDefaultBrain().catch(() => ({ model: null, reason: "", pinned: false }))
          UI.println(
            dim(
              brain.model
                ? `default: ${brain.model} (${brain.reason})`
                : "default: none — pick one with /model or --model",
            ),
          )
        }
      },
    })
  },
})
