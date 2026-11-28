import type { Argv } from "yargs"
import { Instance } from "@/runtime/context/project/instance"
import { Provider } from "@/runtime/providers/provider"
import { ModelsDev } from "@/runtime/providers/adapters/models"
import { ModelList } from "@/runtime/providers/model-list"
import { currentDefaultBrain } from "@/runtime/providers/default-brain"
import { CloudContinue } from "@/runtime/session/cloud-continue"
import { cloudCreditsDisclosure } from "@/shared/util/provider-error"
import { cmd } from "@/cli/commands/cmd"
import { formatProviderLines, jsonRows } from "@/cli/commands/models-rows"
import { resetPickerCatalog } from "@/runtime/providers/discovery"
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
import { describeVerification } from "@/runtime/services/modelVerification"
import { pullModel, verifyModel } from "@/cli/commands/models-pull"
import { fitModel, runModel, psModels, stopModel } from "@/cli/commands/models-run"
import { fetchFreeweightsCatalog } from "@/runtime/services/freeweightsCatalog"
import { freeweightsKey, publishModel } from "@/runtime/services/freeweightsUpload"
import { Sidecar } from "@/runtime/sidecar"

const SOURCE_BADGE: Record<string, string> = {
  subprocess: UI.Style.TEXT_INFO_BOLD + "[cli]" + UI.Style.TEXT_NORMAL,
  local:      UI.Style.TEXT_WARNING_BOLD + "[local]" + UI.Style.TEXT_NORMAL,
  platform:   UI.Style.TEXT_SUCCESS_BOLD + "[platform]" + UI.Style.TEXT_NORMAL,
  plugin:     UI.Style.TEXT_SUCCESS_BOLD + "[plugin]" + UI.Style.TEXT_NORMAL,
}

export const ModelsCommand = cmd({
  command: "models [provider] [repo]",
  describe: "list, assess, and manage local models",
  builder: (yargs: Argv) => {
    return yargs
      .positional("repo", {
        describe: "Hugging Face repo id (org/repo) for `models pull` / `models verify`, or the query for `models search`",
        type: "string",
      })
      .option("allow-unverified", {
        describe: "pull: accept a model with no hash to check against (otherwise exits non-zero)",
        type: "boolean",
        default: false,
      })
      .option("catalog-source", {
        describe: "search: where to search",
        type: "string",
        choices: ["all", "freeweights"],
        default: "all",
      })
      .positional("provider", {
        describe: "provider ID to filter models by, or: pull | verify | search | fit | run | ps | stop | publish",
        type: "string",
        array: false,
      })
      .option("verbose", {
        describe: "use more verbose model output (includes metadata like costs)",
        type: "boolean",
      })
      .option("refresh", {
        describe: "refresh the models cache from models.dev and the picker catalog (bypasses the 6 h cache)",
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
      .option("as", {
        describe: "publish: target repo id (handle/name); the third word also works",
        type: "string",
      })
      .option("revision", {
        describe: "pull/verify: branch, tag or commit (default main)",
        type: "string",
      })
      .option("quant", {
        describe: "quantization tag (e.g. Q4_K_M)",
        type: "string",
      })
      .option("prompt", {
        describe: "run: send one prompt, print the reply and exit (default: open a chat)",
        type: "string",
      })
  },
  handler: async (args) => {
    // gizzi models publish <path> <handle/name>: upload a local folder to Freeweights.
    if (args.provider === "publish") {
      const src = args.repo
      const repoId = args.as ?? (args._ as unknown[]).map(String).find((w) => w.includes("/") && w !== src)
      if (!src || !repoId) {
        UI.error("Usage: gizzi models publish <path> <handle/name>   (needs an Allternit key: ALLTERNIT_API_KEY or FREEWEIGHTS_TOKEN)")
        process.exit(1)
      }
      const key = freeweightsKey()
      if (!key) {
        UI.error("No Allternit key found. Create one with the Freeweights upload scope at platform.allternit.com/api-keys?create=freeweights, then set FREEWEIGHTS_TOKEN.")
        process.exit(1)
      }
      try {
        const r = await publishModel(src, String(repoId), { key, log: (l) => process.stderr.write(l + "\n") })
        UI.println(UI.Style.TEXT_SUCCESS_BOLD + `Uploaded ${r.files} file${r.files === 1 ? "" : "s"}.` + UI.Style.TEXT_NORMAL)
        UI.println("Files are scanned before they are served. The repo shows as scanning until then.")
        UI.println(r.repoUrl)
        process.exit(0)
      } catch (err) {
        UI.error(err instanceof Error ? err.message : "Publish failed")
        process.exit(1)
      }
      return
    }

    if (args.provider === "pull" || args.provider === "verify") {
      const repo = args.repo
      if (!repo || !repo.includes("/")) {
        UI.error(`Usage: gizzi models ${args.provider} <org/repo>${args.provider === "pull" ? " [--quant Q4_K_M] [--allow-unverified]" : ""}`)
        process.exit(1)
      }
      try {
        if (args.provider === "verify") process.exit(await verifyModel(repo, args.quant, args.revision))
        process.exit(await pullModel(repo, args.quant, Boolean(args["allow-unverified"]), args.revision))
      } catch (err) {
        UI.error(err instanceof Error ? err.message : `Failed to ${args.provider}`)
        process.exit(1)
      }
      return
    }

    if (args.provider === "fit" || args.provider === "run" || args.provider === "ps" || args.provider === "stop") {
      try {
        const code =
          args.provider === "fit" ? await fitModel(args.repo, Boolean(args.json))
          : args.provider === "run" ? await runModel(args.repo, { quant: args.quant, prompt: args.prompt, allowUnverified: Boolean(args["allow-unverified"]) })
          : args.provider === "ps" ? await psModels(Boolean(args.json))
          : await stopModel(args.repo)
        process.exit(code)
      } catch (err) {
        UI.error(err instanceof Error ? err.message : `Failed to ${args.provider}`)
        process.exit(1)
      }
      return
    }

    if (args.provider === "search") {
      const q = (args.repo ?? "").trim().toLowerCase()
      try {
        const fw = await fetchFreeweightsCatalog()
        const fwHits = fw.filter((e) => !q || e.id.toLowerCase().includes(q))
        const badge = UI.Style.TEXT_SUCCESS_BOLD + "[freeweights]" + UI.Style.TEXT_NORMAL
        for (const e of fwHits.slice(0, args.limit)) UI.println(`  ${e.id}  ${badge}${e.section ? " " + e.section : ""}`)
        if (args["catalog-source"] !== "freeweights") {
          const seen = new Set(fwHits.map((e) => e.id.toLowerCase()))
          const hf = await Sidecar.searchHuggingFace(q, args.limit)
          for (const m of hf) {
            if (seen.has(m.repoId.toLowerCase())) continue
            UI.println(`  ${m.repoId}  ${m.downloads.toLocaleString()} downloads${m.freeweights ? " " + badge : ""}`)
          }
        }
        process.exit(0)
      } catch (err) {
        UI.error(err instanceof Error ? err.message : "Search failed")
        process.exit(1)
      }
      return
    }

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
      // Drop the shared picker catalog lookup so the Cloud rows re-fetch past the 6 h cache.
      resetPickerCatalog()
      if (!args.json) UI.println(UI.Style.TEXT_SUCCESS_BOLD + "Models cache refreshed" + UI.Style.TEXT_NORMAL)
    }

    await Instance.provide({
      directory: process.cwd(),
      async fn() {
        // With an explicit provider, show it even when the picker would not offer it.
        const sections = await ModelList.build({ all: Boolean(args.all || args.provider) })

        if (args.json) {
          process.stdout.write(JSON.stringify({ rows: jsonRows(sections), sections }, null, 2) + EOL)
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
        // Each row carries its access badge (plan / key / local / credits); locked rows
        // say why; hidden rows are skipped; an offline provider is one dim line.
        // Open-lane Cloud rows also show "open" and the price per 1M in/out (locks come from the catalog).
        const listsCloud = sections.some((s) =>
          s.providers.some((p) => p.id === CloudContinue.PROVIDER_ID && (!args.provider || p.id === args.provider)),
        )
        const openRows = new Map<string, CloudContinue.CatalogModel>()
        if (listsCloud) {
          const rows = (await CloudContinue.catalogRows().catch(() => undefined)) ?? []
          for (const m of rows) if (m.id && m.lane === "open") openRows.set(m.id, m)
        }
        const openTags = (providerId: string, modelID: string): string[] => {
          const open = providerId === CloudContinue.PROVIDER_ID ? openRows.get(modelID) : undefined
          if (!open) return []
          const tags = ["open"]
          const p = open.pricing
          if (typeof p?.input_per_1m_usd === "number" && typeof p?.output_per_1m_usd === "number")
            tags.push(`$${p.input_per_1m_usd}/$${p.output_per_1m_usd} per 1M in/out`)
          if (open.open?.access === "research") tags.push("research")
          return tags
        }
        let anyLocked = false
        for (const section of sections) {
          const providers = section.providers.filter((p) => !args.provider || p.id === args.provider)
          if (providers.length === 0) continue
          if (!args.provider) UI.println(dim(`# ${section.title}`))
          for (const provider of providers) {
            const lines = formatProviderLines(section.kind, provider, undefined, (row) => openTags(provider.id, row.modelID))
            const visible = provider.models.filter((m) => !m.hidden)
            lines.forEach((line, i) => {
              if (line.type === "offline") {
                process.stdout.write(dim(line.text) + EOL)
                return
              }
              if (line.locked) anyLocked = true
              const row = visible[i]
              const rest = line.text.slice(row.ref.length).trim()
              const ref = line.current ? UI.Style.TEXT_HIGHLIGHT_BOLD + row.ref + UI.Style.TEXT_NORMAL : row.ref
              process.stdout.write(ref + "  " + dim(rest) + EOL)
              if (args.verbose) {
                const info = configured[provider.id]?.models?.[row.modelID] ?? modelsDev[provider.id]?.models?.[row.modelID]
                const detail = info ?? { context: row.context, output: row.output }
                process.stdout.write(JSON.stringify(detail, null, 2) + EOL)
              }
            })
          }
        }

        if (anyLocked) UI.println(dim(cloudCreditsDisclosure(undefined)))

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
