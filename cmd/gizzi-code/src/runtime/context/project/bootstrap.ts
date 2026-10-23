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
  // Who is editing what across Code sessions (GET /file-editors, file.contended).
  void import("@/runtime/session/file-editors")
    .then(({ FileEditors }) => FileEditors.init())
    .catch((e) => Log.Default.warn("file editors hook failed", { error: e instanceof Error ? e.message : String(e) }))
  // Keep the live repo map current as files change: never awaited by
  // the tool, debounced inside RepoMap.queue, and a no-op for projects whose
  // map was never loaded (the next load picks changes up by mtime).
  void import("@/codemap/repo-map")
    .then(({ RepoMap }) => {
      Bus.subscribe(File.Event.Edited, (payload) => {
        const { file, sessionID } = payload.properties
        if (sessionID) RepoMap.noteEdited(sessionID, file)
        RepoMap.queue([file])
      })
      // The app's live code map lights edited files from this.
      void import("@/codemap/activity").then(({ CodeMapActivity }) =>
        Bus.subscribe(File.Event.Edited, (payload) =>
          CodeMapActivity.publish(payload.properties.sessionID, payload.properties.file, "edit"),
        ),
      )
      // Deletes (apply_patch) and changes made outside gizzi's tools.
      Bus.subscribe(FileWatcher.Event.Updated, (payload) => RepoMap.queue([payload.properties.file]))
    })
    .catch((e) => Log.Default.warn("repo map hook failed", { error: e instanceof Error ? e.message : String(e) }))
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
