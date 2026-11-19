import { Config } from "@/runtime/context/config/config"
import z from "zod/v4"
import { Provider } from "@/runtime/providers/provider"
import { generateObject, streamObject, type ModelMessage } from "ai"
import { SystemPrompt } from "@/runtime/session/system"
import { Instance } from "@/runtime/context/project/instance"
import { Truncate } from "@/runtime/tools/builtins/truncation"
import { Auth } from "@/runtime/integrations/auth"
import { ProviderTransform } from "@/runtime/providers/adapters/transform"

import PROMPT_GENERATE from "./prompts/generate.txt"
import PROMPT_COMPACTION from "./prompts/compaction.txt"
import PROMPT_EXPLORE from "./prompts/explore.txt"
import PROMPT_SUMMARY from "./prompts/summary.txt"
import PROMPT_TITLE from "./prompts/title.txt"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { mergeDeep, pipe, sortBy, values } from "remeda"
import { Global } from "@/runtime/context/global"
import path from "path"
import { Plugin } from "@/runtime/integrations/plugin"
import { Skill } from "@/runtime/skills/skill"
import { BUILTIN_CATALOG, CATALOG_ALIASES, catalogAgent, isModelClass, MODEL_CLASSES } from "@/runtime/agents/builtin-catalog"
import { CatalogSync } from "@/runtime/agents/catalog-sync"
import { CREDENTIAL_PATH_RULES } from "@/runtime/tools/guard/permission/credential-paths"

const HarnessConfigSchema = z.object({
  mode: z.enum(["byok", "cloud", "local", "subprocess"]),
  byok: z
    .object({
      keys: z
        .object({
          anthropic: z.string().optional(),
          openai: z.string().optional(),
          google: z.string().optional(),
        })
        .optional(),
      baseURLs: z
        .object({
          anthropic: z.string().optional(),
          openai: z.string().optional(),
          google: z.string().optional(),
        })
        .optional(),
    })
    .optional(),
  cloud: z
    .object({
      baseURL: z.string(),
      accessToken: z.string(),
      refreshToken: z.string().optional(),
    })
    .optional(),
  local: z
    .object({
      baseURL: z.string(),
    })
    .optional(),
  subprocess: z
    .object({
      command: z.string(),
      cwd: z.string().optional(),
      env: z.record(z.string(), z.string()).optional(),
    })
    .optional(),
})

export namespace Agent {
  export const Info = z
    .object({
      name: z.string(),
      description: z.string().optional(),
      mode: z.enum(["subagent", "primary", "all"]),
      native: z.boolean().optional(),
      hidden: z.boolean().optional(),
      topP: z.number().optional(),
      temperature: z.number().optional(),
      color: z.string().optional(),
      permission: PermissionNext.Ruleset,
      model: z
        .object({
          modelID: z.string(),
          providerID: z.string(),
        })
        .optional(),
      /** Logical model class (small/standard/deep/inherit); used when `model` is unset. */
      modelClass: z.enum(MODEL_CLASSES).optional(),
      /** Display name for the subagent's card (e.g. "Code Reviewer"). */
      title: z.string().optional(),
      /** Bot-avatar face slug the app draws on the subagent's card. */
      face: z.string().optional(),
      /** Set on legacy names (general, explore) that stand for a catalog type. */
      aliasOf: z.string().optional(),
      variant: z.string().optional(),
      prompt: z.string().optional(),
      options: z.record(z.string(), z.any()),
      steps: z.number().int().positive().optional(),
      // O12: per-turn overrides of the versioned defaults (guardrail-defaults.ts), clamped to the ceiling.
      maxToolCalls: z.number().int().positive().optional(),
      turnTimeoutMs: z.number().int().positive().optional(),
      summaryPolicy: z
        .object({
          minChars: z.number().int().nonnegative(),
          continuationPrompt: z.string().min(1),
          retries: z.number().int().min(0).max(3),
        })
        .optional(),
      harness: HarnessConfigSchema.optional(),
    })
    
  export type Info = z.infer<typeof Info>

  const state = Instance.state(async () => {
    const cfg = await Config.get()

    const skillDirs = await Skill.dirs()
    const whitelistedDirs = [Truncate.GLOB, ...skillDirs.map((dir) => path.join(dir, "*"))]
    const defaults = PermissionNext.fromConfig({
      "*": "ask",
      grep: "allow",
      glob: "allow",
      list: "allow",
      codesearch: "allow",
      websearch: "allow",
      webfetch: "allow",
      todoread: "allow",
      todowrite: "allow",
      lsp: "allow",
      skill: "allow",
      skill_evaluate: "allow",
      skill_manage: "ask",
      task: "allow",
      task_list: "allow",
      task_output: "allow",
      task_stop: "ask",
      goal_budget: "allow",
      current_time: "allow",
      // The tool itself is free; its network kinds (weather, places, currency,
      // link, images) ask under websearch/webfetch inside the tool, so they
      // follow the rules the user already set for the web.
      lookup: "allow",
      // Reaches only the session's owner; push is opt-in per call and rate-limited server side.
      send_user_message: "allow",
      // Scheduled tasks: the action is the pattern. Reading the list is free;
      // create, update, delete and run_now ask (not ALWAYS_ASK, so "always" holds).
      automation: {
        "*": "ask",
        list: "allow",
      },
      // A question that doesn't block: it waits in the thread; the turn goes on.
      // Allowed for every agent, unlike the blocking `question` (denied by default).
      ask_user_later: "allow",
      // Primitives Wave B. agent_control: the action is the pattern; closing an
      // agent (and everything under it) asks, the rest act on own children freely.
      wait_agents: "allow",
      agent_control: {
        "*": "allow",
        close: "ask",
      },
      tool_search: "allow",
      get_context_remaining: "allow",
      history_recall: "allow",
      // Wave D recall: reads of the user's own memory, past chats and this
      // session's saved outputs. thread_wait only waits; it changes nothing.
      memory_list: "allow",
      thread_search: "allow",
      thread_read: "allow",
      thread_recent: "allow",
      thread_wait: "allow",
      resource: "allow",
      // A short wait inside the turn (max 15 min) that wakes on the user's next message.
      sleep: "allow",
      // One worker per CSV row, so every call costs real model calls: always
      // asks (the card shows the rows, concurrency and model). The pattern is
      // `<mode>:<subagent_type>`, so allowing a sample never allows a full run.
      // Its workers can't fan out: agent-batch.ts gives each worker session
      // deny rules for WORKER_DENIED_TOOLS (task, agent_swarm, agent_batch,
      // batch, sleep...), which come after these and win.
      agent_batch: "ask",
      // Seeing the user's screen: the tool asks once per session ("always" holds).
      screen_context: "ask",
      // Hidden browser tasks: every call asks, naming the sites and goals.
      browser_tasks: "ask",
      // Wave D connect and settings. Tools a connector_search load mounts keep
      // their own rules. settings: the action is the pattern; the risk gate
      // (confirm/locked keys) is on the server. The plan agent denies write.
      connector_search: "allow",
      connector_suggest: "allow",
      settings: "allow",
      // Prompt-gap tools (G3 contract; each is registered only once its module
      // sets `available`). worktree_exit and pr_comments: the action is the
      // pattern. Removing a worktree and posting a PR reply ask; keep and list
      // are free. user_location: the precision is the pattern; approximate is
      // free, precise asks (its lane makes precise ask every time).
      worktree_enter: "ask",
      worktree_exit: {
        "*": "ask",
        keep: "allow",
      },
      // Read-only and advisory: a review on a deeper tier, like `task`.
      second_opinion: "allow",
      pr_comments: {
        "*": "ask",
        list: "allow",
      },
      status_update: "allow",
      // Stays on the device until the person sends it.
      feedback_note: "allow",
      user_location: {
        "*": "ask",
        approximate: "allow",
      },
      voice_control: "allow",
      // Asking the person who is being abusive defeats the purpose; the tool
      // itself requires an earlier warning and refuses outside Gizzi's own chats.
      end_conversation: "allow",
      doom_loop: "ask",
      external_directory: {
        "*": "ask",
        ...Object.fromEntries(whitelistedDirs.map((dir) => [dir, "allow"])),
      },
      question: "deny",
      plan_enter: "deny",
      plan_exit: "deny",
      // Sensitive credentials and git control paths require review by default.
      // Public key and environment-template variants remain ordinary project files.
      // One list (credential-paths.ts): send_user_message refuses the same files.
      read: {
        "*": "allow",
        ...CREDENTIAL_PATH_RULES,
      },
      edit: {
        "*": "allow",
        ...CREDENTIAL_PATH_RULES,
      },
    }, "default")
    // Tagged "user" so auto mode in app sessions keeps the person's own asks.
    const user = PermissionNext.fromConfig(cfg.permission ?? {}, "user")

    const result: Record<string, Info> = {
      build: {
        name: "build",
        description: "The default agent. Executes tools based on configured permissions.",
        options: {},
        permission: PermissionNext.merge(
          defaults,
          PermissionNext.fromConfig({
            question: "allow",
            plan_enter: "allow",
          }),
          user,
        ),
        mode: "primary",
        native: true,
      },
      plan: {
        name: "plan",
        description: "Plan mode. Disallows all edit tools.",
        options: {},
        permission: PermissionNext.merge(
          defaults,
          PermissionNext.fromConfig({
            question: "allow",
            plan_exit: "allow",
            // Plan mode changes nothing: settings reads work, writes are refused.
            settings: { write: "deny" },
            // No new worktree and no PR replies while planning.
            worktree_enter: "deny",
            pr_comments: { reply: "deny" },
            external_directory: {
              [path.join(Global.Path.data, "plans", "*")]: "allow",
            },
            edit: {
              "*": "deny",
              [path.join(".gizzi", "plans", "*.md")]: "allow",
              [path.relative(Instance.worktree, path.join(Global.Path.data, path.join("plans", "*.md")))]: "allow",
            },
          }),
          user,
        ),
        mode: "primary",
        native: true,
      },
      general: {
        name: "general",
        description: `General-purpose agent for researching complex questions and executing multi-step tasks. Use this agent to execute multiple units of work in parallel.`,
        permission: PermissionNext.merge(
          defaults,
          PermissionNext.fromConfig({
            todoread: "deny",
            todowrite: "deny",
            // The parent's chat and call belong to the parent: a subagent never ends them.
            end_conversation: "deny",
            voice_control: "deny",
          }),
          user,
        ),
        options: {},
        summaryPolicy: {
          minChars: 200,
          retries: 1,
          continuationPrompt:
            "Expand your handoff for the parent. Include concrete findings or changes, relevant file paths, verification performed, and any remaining risks or follow-up work.",
        },
        mode: "subagent",
        native: true,
      },
      explore: {
        name: "explore",
        permission: PermissionNext.merge(
          defaults,
          PermissionNext.fromConfig({
            "*": "deny",
            grep: "allow",
            glob: "allow",
            list: "allow",
            bash: "allow",
            webfetch: "allow",
            websearch: "allow",
            codesearch: "allow",
            read: "allow",
            external_directory: {
              "*": "ask",
              ...Object.fromEntries(whitelistedDirs.map((dir) => [dir, "allow"])),
            },
          }),
          user,
        ),
        description: `Fast agent specialized for exploring codebases. Use this when you need to quickly find files by patterns (eg. "src/components/**/*.tsx"), search code for keywords (eg. "API endpoints"), or answer questions about the codebase (eg. "how do API endpoints work?"). When calling this agent, specify the desired thoroughness level: "quick" for basic searches, "medium" for moderate exploration, or "very thorough" for comprehensive analysis across multiple locations and naming conventions.`,
        prompt: PROMPT_EXPLORE,
        options: {},
        summaryPolicy: {
          minChars: 200,
          retries: 1,
          continuationPrompt:
            "Expand your handoff for the parent. Include concrete findings, relevant file paths and symbols, evidence for your conclusions, and unresolved questions.",
        },
        mode: "subagent",
        native: true,
      },
      // Independent reviewer (runtime/review): fresh context, read-only. Its
      // rules come after the user's so config can't hand it edit tools.
      review: {
        name: "review",
        description: "Independent read-only reviewer that returns a PASS/FAIL/PARTIAL verdict on changes.",
        mode: "subagent",
        native: true,
        hidden: true,
        permission: PermissionNext.merge(
          defaults,
          user,
          PermissionNext.fromConfig({
            "*": "deny",
            read: {
              "*": "allow",
              "*.env": "deny",
              "*.env.*": "deny",
              "*.env.example": "allow",
            },
            grep: "allow",
            glob: "allow",
            list: "allow",
            lsp: "allow",
            bash: {
              "*": "deny",
              "git diff*": "allow",
              "git log*": "allow",
              "git show*": "allow",
              "git status*": "allow",
              "git blame*": "allow",
              "git ls-files*": "allow",
              "ls*": "allow",
              "cat *": "allow",
              "head *": "allow",
              "tail *": "allow",
              "wc *": "allow",
              "rg *": "allow",
              "grep *": "allow",
            },
          }),
        ),
        options: {},
      },
      compaction: {
        name: "compaction",
        mode: "primary",
        native: true,
        hidden: true,
        prompt: PROMPT_COMPACTION,
        permission: PermissionNext.merge(
          defaults,
          PermissionNext.fromConfig({
            "*": "deny",
          }),
          user,
        ),
        options: {},
      },
      title: {
        name: "title",
        mode: "primary",
        options: {},
        native: true,
        hidden: true,
        temperature: 0.5,
        permission: PermissionNext.merge(
          defaults,
          PermissionNext.fromConfig({
            "*": "deny",
          }),
          user,
        ),
        prompt: PROMPT_TITLE,
      },
      summary: {
        name: "summary",
        mode: "primary",
        options: {},
        native: true,
        hidden: true,
        permission: PermissionNext.merge(
          defaults,
          PermissionNext.fromConfig({
            "*": "deny",
          }),
          user,
        ),
        prompt: PROMPT_SUMMARY,
      },
    }

    // The 11-type subagent catalog (runtime/agents/builtin-catalog.ts).
    // Scoped types ("*": deny) layer their rules after the user's so config
    // can't widen them (e.g. hand a reviewer edit); builder lets the user's win.
    for (const entry of BUILTIN_CATALOG) {
      const own = PermissionNext.fromConfig({
        ...entry.permission,
        // After the type's "*" rule: outside the worktree asks, except skill
        // dirs and truncated-output files.
        external_directory: {
          "*": "ask",
          ...Object.fromEntries(whitelistedDirs.map((dir) => [dir, "allow"])),
        },
      } as Config.Permission)
      const readOnly = entry.permission["*"] === "deny"
      result[entry.name] = {
        name: entry.name,
        title: entry.title,
        description: entry.description,
        mode: entry.mode,
        native: true,
        modelClass: entry.modelClass,
        face: entry.face,
        steps: entry.steps,
        prompt: entry.prompt,
        permission: readOnly ? PermissionNext.merge(defaults, user, own) : PermissionNext.merge(defaults, own, user),
        options: {},
        summaryPolicy: {
          minChars: 200,
          retries: 1,
          continuationPrompt:
            "Expand your handoff for the parent. Include concrete findings or changes, relevant file paths, evidence or verification performed, and any remaining risks or follow-up work.",
        },
      }
    }
    // Legacy subagent names stay callable (configs, @mentions, models.jobs)
    // but now carry their catalog type's identity and model class. Hidden
    // from the task tool listing so the model sees each type once.
    for (const [legacy, target] of Object.entries(CATALOG_ALIASES)) {
      const item = result[legacy]
      const entry = catalogAgent(target)
      if (!item || !entry || item.mode !== "subagent") continue
      item.title = entry.title
      item.face = entry.face
      item.modelClass = entry.modelClass
      item.aliasOf = target
    }

    for (const [key, value] of Object.entries(cfg.agent ?? {})) {
      if (value.disable) {
        delete result[key]
        continue
      }
      let item = result[key]
      if (!item)
        item = result[key] = {
          name: key,
          mode: "all",
          permission: PermissionNext.merge(defaults, user),
          options: {},
          native: false,
        }
      // `model: small|standard|deep|inherit` (config or markdown frontmatter)
      // is a model class; anything else is a concrete provider/model.
      if (value.model && isModelClass(value.model)) {
        item.modelClass = value.model.trim().toLowerCase() as Info["modelClass"]
        item.model = undefined
      } else if (value.model) item.model = Provider.parseModel(value.model)
      if (value.model_class && isModelClass(value.model_class))
        item.modelClass = value.model_class.trim().toLowerCase() as Info["modelClass"]
      if (value.title?.trim()) item.title = value.title.trim()
      if (value.face?.trim()) item.face = value.face.trim()
      item.variant = value.variant ?? item.variant
      item.prompt = value.prompt ?? item.prompt
      item.description = value.description ?? item.description
      item.temperature = value.temperature ?? item.temperature
      item.topP = value.top_p ?? item.topP
      item.mode = value.mode ?? item.mode
      item.color = value.color ?? item.color
      item.hidden = value.hidden ?? item.hidden
      item.name = value.name ?? item.name
      item.steps = value.steps ?? item.steps
      item.maxToolCalls = value.max_tool_calls ?? item.maxToolCalls
      item.turnTimeoutMs = value.turn_timeout_ms ?? item.turnTimeoutMs
      item.summaryPolicy = value.summary_policy
        ? {
            minChars: value.summary_policy.min_chars,
            continuationPrompt: value.summary_policy.continuation_prompt,
            retries: value.summary_policy.retries,
          }
        : item.summaryPolicy
      item.harness = value.harness ?? item.harness
      item.options = mergeDeep(item.options, value.options ?? {})
      item.permission = PermissionNext.merge(item.permission, PermissionNext.fromConfig(value.permission ?? {}, "user"))
    }

    // Ensure Truncate.GLOB is allowed unless explicitly configured
    for (const name in result) {
      const agent = result[name]
      const explicit = agent.permission.some((r) => {
        if (r.permission !== "external_directory") return false
        if (r.action !== "deny") return false
        return r.pattern === Truncate.GLOB
      })
      if (explicit) continue

      result[name].permission = PermissionNext.merge(
        result[name].permission,
        PermissionNext.fromConfig({ external_directory: { [Truncate.GLOB]: "allow" } }),
      )
    }

    return result
  })

  /** Account-level overrides (Allternit catalog sync) on a built-in catalog type. */
  function withOverride(agent: Info | undefined): Info | undefined {
    if (!agent) return agent
    const key = agent.aliasOf ?? agent.name
    if (!agent.native || !catalogAgent(key)) return agent
    const o = CatalogSync.override(key)
    if (!o) return agent
    return {
      ...agent,
      title: o.title ?? agent.title,
      face: o.face ?? agent.face,
      description: o.description ?? agent.description,
      modelClass: agent.model ? agent.modelClass : (o.modelClass ?? agent.modelClass),
    }
  }

  export async function get(agent: string) {
    return state().then((x) => withOverride(x[agent]))
  }

  /** Resolve a `subagent_type`, following legacy aliases (explore → explorer) when
   * the name is unknown or names a primary agent. */
  export async function getSubagent(name: string) {
    const agents = await state()
    const direct = agents[name]
    if (direct && direct.mode !== "primary") return withOverride(direct)
    const target = CATALOG_ALIASES[name]
    if (target && agents[target]) return withOverride(agents[target])
    return withOverride(direct)
  }

  export async function list() {
    const cfg = await Config.get()
    return pipe(
      await state(),
      values(),
      (items) => items.map((x) => withOverride(x)!),
      sortBy([(x) => (cfg.default_agent ? x.name === cfg.default_agent : x.name === "build"), "desc"]),
    )
  }

  export async function defaultAgent() {
    const cfg = await Config.get()
    const agents = await state()

    if (cfg.default_agent) {
      const agent = agents[cfg.default_agent]
      if (!agent) throw new Error(`default agent "${cfg.default_agent}" not found`)
      if (agent.mode === "subagent") throw new Error(`default agent "${cfg.default_agent}" is a subagent`)
      if (agent.hidden === true) throw new Error(`default agent "${cfg.default_agent}" is hidden`)
      return agent.name
    }

    const primaryVisible = Object.values(agents).find((a) => a.mode !== "subagent" && a.hidden !== true)
    if (!primaryVisible) throw new Error("no primary visible agent found")
    return primaryVisible.name
  }

  export async function generate(input: { description: string; model?: { providerID: string; modelID: string } }) {
    const cfg = await Config.get()
    const defaultModel = input.model ?? (await Provider.defaultModelConcrete())
    const model = await Provider.getModel(defaultModel.providerID, defaultModel.modelID)
    const language = await Provider.getLanguage(model)

    const system = [PROMPT_GENERATE]
    await Plugin.trigger("experimental.chat.system.transform", { model }, { system })
    const existing = await list()

    const params = {
      experimental_telemetry: {
        isEnabled: cfg.experimental?.openTelemetry,
        metadata: {
          userId: cfg.username ?? "unknown",
        },
      },
      temperature: 0.3,
      messages: [
        ...system.map(
          (item): ModelMessage => ({
            role: "system",
            content: item,
          }),
        ),
        {
          role: "user",
          content: `Create an agent configuration based on this request: \"${input.description}\".\n\nIMPORTANT: The following identifiers already exist and must NOT be used: ${existing.map((i) => i.name).join(", ")}\n  Return ONLY the JSON object, no other text, do not wrap in backticks`,
        },
      ],
      model: language,
      schema: z.object({
        identifier: z.string(),
        whenToUse: z.string(),
        systemPrompt: z.string(),
      }),
    } satisfies Parameters<typeof generateObject>[0]

    if (defaultModel.providerID === "openai" && (await Auth.get(defaultModel.providerID))?.type === "oauth") {
      const result = streamObject({
        ...params,
        providerOptions: ProviderTransform.providerOptions(model, {
          instructions: SystemPrompt.instructions(model),
          store: false,
        }),
        onError: () => {},
      })
      for await (const part of result.fullStream) {
        if (part.type === "error") throw part.error
      }
      return result.object
    }

    const result = await generateObject(params)
    return result.object
  }
}
