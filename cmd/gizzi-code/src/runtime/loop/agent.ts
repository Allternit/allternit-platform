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
      read: {
        "*": "allow",
        "*.env": "ask",
        "*.env.*": "ask",
        "*id_rsa*": "ask",
        "*id_ed25519*": "ask",
        "*id_ecdsa*": "ask",
        "*credentials*": "ask",
        ".git/*": "ask",
        "*/.git/*": "ask",
        "*.env.example": "allow",
        "*.env.sample": "allow",
        "*.env.template": "allow",
        "*id_rsa.pub": "allow",
        "*id_ed25519.pub": "allow",
        "*id_ecdsa.pub": "allow",
      },
      edit: {
        "*": "allow",
        "*.env": "ask",
        "*.env.*": "ask",
        "*id_rsa*": "ask",
        "*id_ed25519*": "ask",
        "*id_ecdsa*": "ask",
        "*credentials*": "ask",
        ".git/*": "ask",
        "*/.git/*": "ask",
        "*.env.example": "allow",
        "*.env.sample": "allow",
        "*.env.template": "allow",
        "*id_rsa.pub": "allow",
        "*id_ed25519.pub": "allow",
        "*id_ecdsa.pub": "allow",
      },
    }, "default")
    const user = PermissionNext.fromConfig(cfg.permission ?? {})

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
      if (value.model) item.model = Provider.parseModel(value.model)
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
      item.permission = PermissionNext.merge(item.permission, PermissionNext.fromConfig(value.permission ?? {}))
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

  export async function get(agent: string) {
    return state().then((x) => x[agent])
  }

  export async function list() {
    const cfg = await Config.get()
    return pipe(
      await state(),
      values(),
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
          instructions: SystemPrompt.instructions(),
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
