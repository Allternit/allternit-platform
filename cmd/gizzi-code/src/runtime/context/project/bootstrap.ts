import { Plugin } from "@/runtime/integrations/plugin"
import { Format } from "@/shared/format"
import { LSP } from "@/runtime/integrations/lsp"
import { FileWatcher } from "@/shared/file/watcher"
import { File } from "@/shared/file"
import { Project } from "@/runtime/context/project/project"
import { Bus } from "@/shared/bus"
import { Command } from "@/runtime/loop/command"
import { Instance } from "@/runtime/context/project/instance"
import { Vcs } from "@/runtime/context/project/vcs"
import { Log } from "@/shared/util/log"
import { ShareNext } from "@/runtime/session/share/share-next"
import { Snapshot } from "@/runtime/session/snapshot"
import { Truncate } from "@/runtime/tools/builtins/truncation"
import { Sidecar } from "@/runtime/sidecar"
import { initRemoteControlPush } from "@/runtime/integrations/remote-control-push"
import { ProcessRegistry } from "@/runtime/process-registry"
import { registerCleanup } from "@/shared/utils/cleanupRegistry"

export async function InstanceBootstrap() {
  Log.Default.info("bootstrapping", { directory: Instance.directory })
  ProcessRegistry.install()
  registerCleanup(async () => {
    ProcessRegistry.killAll()
    await Sidecar.stop()
  })
  await Plugin.init()
  ShareNext.init()
  Format.init()
  await LSP.init()
  FileWatcher.init()
  File.init()
  Vcs.init()
  Snapshot.init()
  Truncate.init()
  // Sessions paused before a usage limit resume on their own (P3.17).
  void import("@/runtime/session/pause")
    .then(({ SessionPause }) => SessionPause.restore())
    .catch((e) => Log.Default.warn("paused-session restore failed", { error: e instanceof Error ? e.message : String(e) }))

  // Initialize Agent Communication Runtime
  try {
    const { AgentCommunicationRuntime } = await import("@/runtime/agents/communication-runtime-fixed")
    await AgentCommunicationRuntime.initialize()
  } catch (e) {
    Log.Default.warn("agent communication runtime setup failed", { error: e instanceof Error ? e.message : String(e) })
  }

  // Start embedded model sidecar in the background (non-blocking)
  Sidecar.ensure().catch((e) => {
    Log.Default.warn("sidecar setup failed", { error: e instanceof Error ? e.message : String(e) })
  })

  // Cloud catalog + installed CLI brains — default picker sources.
  // Paid Plus/Super/Ultra auto-provisions Allternit Cloud as the default brain;
  // unpaid falls through to the first installed CLI.
  void import("@/runtime/providers/discovery")
    .then(async ({ Discovery }) => {
      const providers = await Discovery.run()
      const { applyDefaultBrain } = await import("@/runtime/providers/default-brain")
      await applyDefaultBrain(providers)
    })
    .catch((e) => {
      Log.Default.warn("provider discovery failed", { error: e instanceof Error ? e.message : String(e) })
    })

  Bus.subscribe(Command.Event.Executed, async (payload) => {
    if (payload.properties.name === Command.Default.INIT) {
      await Project.setInitialized(Instance.project.id)
    }
  })

  initRemoteControlPush()
}
