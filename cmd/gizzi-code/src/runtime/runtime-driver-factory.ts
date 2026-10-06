/**
 * RuntimeDriverFactory — resolves a registered runtime + CLI to the right driver.
 *
 * The factory is the single entry point for executing agent tasks:
 *   • "local" transports -> LocalCliDriver (spawns the CLI on this host).
 *   • "websocket"/"uds" transports -> remote drivers (Phase 3).
 *
 * It also provides auto-discovery fallback: if no runtime is registered for a
 * given CLI, the local runtime is discovered on-demand and persisted, so
 * gizzi-code works out of the box without a separate `gizzi agents harness register`.
 */

import type { RuntimeDriver } from "@/runtime/runtime-driver"
import { RuntimeService, type RegisteredRuntime } from "@/runtime/runtime-service"
import { discoverLocalRuntime } from "@/runtime/runtime-discovery"
import { LocalCliDriver } from "@/runtime/drivers/local-cli-driver"
import { WebSocketDriver } from "@/runtime/drivers/websocket-driver"
import { UdsDriver } from "@/runtime/drivers/uds-driver"
import { Log } from "@/shared/util/log"

const log = Log.create({ service: "runtime-driver-factory" })

const localDrivers = new Map<string, RuntimeDriver>()

export namespace RuntimeDriverFactory {
  export async function forRuntime(runtimeId: string, cliName: string): Promise<RuntimeDriver> {
    const runtime = await RuntimeService.get(runtimeId)
    if (!runtime) {
      throw new Error(`Runtime ${runtimeId} not found`)
    }
    return getDriver(runtime, cliName)
  }

  /**
   * Resolve a CLI name (e.g. "claude-cli") to a registered runtime and a driver
   * instance ready to execute against it. If no runtime advertises the CLI, a
   * local discovery pass is attempted and the result is persisted.
   */
  export async function resolveCli(
    cliName: string,
  ): Promise<{ runtime: RegisteredRuntime; driver: RuntimeDriver }> {
    let runtime = await findRuntimeWithCli(cliName)

    if (!runtime) {
      log.info("no runtime registered for CLI, running local discovery", { cli: cliName })
      const discovered = await discoverLocalRuntime()
      const match = discovered.agentClis.find((cli) => cli.name === cliName)
      if (match) {
        runtime = await RuntimeService.upsertByHost(discovered, { name: discovered.host })
      }
    }

    if (!runtime) {
      throw new Error(
        `No runtime found for CLI ${cliName}. Install and authenticate the CLI, then run \`gizzi agents harness register\`.`,
      )
    }

    const driver = await getDriver(runtime, cliName)
    return { runtime, driver }
  }
}

async function findRuntimeWithCli(cliName: string): Promise<RegisteredRuntime | undefined> {
  const runtimes = await RuntimeService.list()
  for (const runtime of runtimes) {
    if (runtime.agentClis.some((cli) => cli.name === cliName)) {
      return runtime
    }
  }
  return undefined
}

async function getDriver(runtime: RegisteredRuntime, cliName: string): Promise<RuntimeDriver> {
  if (runtime.transport === "local") {
    const key = `${runtime.id}:${cliName}`
    let driver = localDrivers.get(key)
    if (!driver) {
      driver = new LocalCliDriver(runtime.id, cliName)
      localDrivers.set(key, driver)
    }
    return driver
  }

  if (runtime.transport === "websocket") {
    const url = runtime.metadata?.websocketUrl
    const token = runtime.metadata?.token
    if (!url) {
      throw new Error(`Runtime ${runtime.id} has no websocketUrl in metadata`)
    }
    return new WebSocketDriver(runtime.id, cliName, url, token ?? "")
  }

  if (runtime.transport === "uds") {
    const socketPath = runtime.metadata?.udsSocket
    const token = runtime.metadata?.token
    if (!socketPath) {
      throw new Error(`Runtime ${runtime.id} has no udsSocket in metadata`)
    }
    return new UdsDriver(runtime.id, cliName, socketPath, token ?? "")
  }

  throw new Error(`Runtime transport ${runtime.transport} is not supported yet`)
}
