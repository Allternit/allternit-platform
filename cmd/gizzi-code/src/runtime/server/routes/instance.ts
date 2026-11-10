import { Hono } from "hono"
import { describeRoute, resolver, validator } from "@/runtime/server/openapi"
import z from "zod/v4"
import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Config } from "@/runtime/context/config/config"
import { LSP } from "@/runtime/integrations/lsp"
import { Format } from "@/shared/format"
import { Agent } from "@/runtime/loop/agent"
import { Command } from "@/runtime/loop/command"
import { ModelsDev } from "@/runtime/providers/adapters/models"
import { Provider } from "@/runtime/providers/provider"
import { ProviderAuth } from "@/runtime/providers/adapters/auth"
import { Vcs } from "@/runtime/context/project/vcs"
import { Global } from "@/runtime/context/global"
import { Instance } from "@/runtime/context/project/instance"
import * as Bridge from "@/runtime/kernel/bridge"
import { Workspace } from "@/runtime/workspace/workspace"
import { clearWorkspaceCache } from "@/runtime/session/session-context"
import { Tunnel } from "@/runtime/server/tunnel"

export function InstanceRoutes() {
  return new Hono()
    .get(
      "/status",
      describeRoute({
        summary: "Instance status",
        description: "Lightweight server/instance status, including the public cloudflared tunnel URL when --tunnel is active.",
        operationId: "instance.status",
        responses: {
          200: {
            description: "Instance status",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      async (c) => {
        return c.json({
          status: "ok",
          service: "gizzi-code",
          tunnel: {
            enabled: Tunnel.active() || Tunnel.url() !== undefined,
            url: Tunnel.url() ?? null,
          },
        })
      },
    )
    .get(
      "/sync",
      describeRoute({
        summary: "Sync TUI state",
        description: "Emit instance.sync event with full TUI hydration snapshot",
        operationId: "instance.sync",
        responses: {
          200: {
            description: "Sync emitted",
            content: {
              "application/json": {
                schema: resolver(z.boolean()),
              },
            },
          },
        },
      }),
      async (c) => {
        const [config, lsp, formatter, agents, commands, workspace] = await Promise.all([
          Config.get().catch(() => ({})),
          LSP.status().catch(() => []),
          Format.status().catch(() => []),
          Agent.list().catch(() => []),
          Command.list().catch(() => []),
          Bridge.detectWorkspace(Instance.directory).catch(() => null),
        ])

        const cfg = config as any
        const disabled = new Set(cfg.disabled_providers ?? [])
        const enabled = cfg.enabled_providers ? new Set(cfg.enabled_providers) : undefined
        const allProviderData = await ModelsDev.get().catch(() => ({}))
        const filtered: Record<string, any> = {}
        for (const [key, value] of Object.entries(allProviderData)) {
          if (key === "vercel") continue
          if ((enabled ? (enabled as Set<string>).has(key) : true) && !disabled.has(key)) {
            filtered[key] = value
          }
        }
        const connected = await Provider.list().catch(() => ({}))
        const providerMap: Record<string, any> = Object.assign(
          Object.fromEntries(
            Object.entries(filtered).map(([k, v]) => [k, Provider.fromModelsDevProvider(v as any)]),
          ),
          connected,
        )
        const providerAll = Object.values(providerMap)
        const providerDefault = Object.fromEntries(
          Object.entries(providerMap).map(([k, v]) => [
            k,
            Provider.sort(Object.values((v as any).models ?? {}) as { id: string }[])[0]?.id ?? "",
          ]),
        )
        const providerAuth = await ProviderAuth.methods().catch(() => ({}))
        const vcs = await Vcs.branch().catch(() => undefined)

        const payload = {
          provider: providerAll,
          provider_default: providerDefault,
          provider_next: {
            all: providerAll,
            default: providerDefault,
            connected: Object.keys(connected),
          },
          provider_auth: providerAuth,
          agent: agents,
          command: commands,
          config,
          workspace: workspace
            ? {
                type: workspace.type,
                format: workspace.format,
                path: workspace.path,
                name: workspace.identity?.name,
                emoji: workspace.identity?.emoji,
                vibe: workspace.identity?.vibe,
                hasMemory: !!workspace.identity?.memory,
                hasSoul: !!workspace.identity?.soul,
                hasBrain: !!workspace.identity?.brain,
                layered: workspace.identity?.layered ?? false,
              }
            : null,
          session: [],
          session_status: {},
          todo: {},
          message: {},
          part: {},
          lsp,
          mcp: {},
          mcp_resource: {},
          formatter,
          vcs: vcs ? { branch: vcs } : undefined,
          path: {
            home: Global.Path.home,
            state: Global.Path.state,
            config: Global.Path.config,
            worktree: Instance.worktree,
            directory: Instance.directory,
          },
          runs: {},
          cron_jobs: [],
          cron_runs: [],
          cron_status: { jobs: 0, active: 0, pendingRuns: 0, runningRuns: 0 },
          user: null,
        }

        const InstanceSyncEvent = BusEvent.define("instance.sync", z.any())
        await Bus.publish(InstanceSyncEvent, payload)
        return c.json(true)
      },
    )
    .post(
      "/dispose",
      describeRoute({
        summary: "Dispose instance",
        description: "Clean up and dispose the current GIZZI instance, releasing all resources.",
        operationId: "instance.dispose",
        responses: {
          200: {
            description: "Instance disposed",
            content: {
              "application/json": {
                schema: resolver(z.boolean()),
              },
            },
          },
        },
      }),
      async (c) => {
        await Instance.dispose()
        return c.json(true)
      },
    )
    .get(
      "/workspace",
      describeRoute({
        summary: "Get workspace identity",
        description: "Detect and return the active GizziClaw/.openclaw workspace identity.",
        operationId: "instance.workspace",
        responses: {
          200: {
            description: "Workspace identity or null",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
        },
      }),
      async (c) => {
        const ws = await Bridge.detectWorkspace(Instance.directory).catch(() => null)
        if (!ws) return c.json(null)
        return c.json({
          type: ws.type,
          format: ws.format,
          path: ws.path,
          name: ws.identity?.name,
          emoji: ws.identity?.emoji,
          vibe: ws.identity?.vibe,
          hasMemory: !!ws.identity?.memory,
          hasSoul: !!ws.identity?.soul,
          hasBrain: !!ws.identity?.brain,
          layered: ws.identity?.layered ?? false,
        })
      },
    )
}
// Workspace init/import/identity routes live in WorkspaceRoutes (/v1/workspace/)
