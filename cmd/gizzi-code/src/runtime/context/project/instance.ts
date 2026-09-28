import { Log } from "@/shared/util/log"
import { Context } from "@/shared/util/context"
import { Project } from "@/runtime/context/project/project"
import { State } from "@/runtime/context/project/state"
import { iife } from "@/shared/util/iife"
import { GlobalBus } from "@/shared/bus/global"
import { Filesystem } from "@/shared/util/filesystem"
import { Flag } from "@/runtime/context/flag/flag"
import fs from "fs"
import path from "path"

interface Context {
  directory: string
  worktree: string
  project: Project.Info
}
const context = Context.create<Context>("instance")
// A session's working folder (e.g. its project's folder) when it differs from
// the instance directory. Kept apart from the instance context on purpose:
// instance state (bus, permissions, session status) is keyed by the instance
// directory, so a session keeps its events where its clients listen while its
// tools and CLI agents run in the folder.
const workdirContext = Context.create<string>("instance.workdir")
const cache = new Map<string, Promise<Context>>()

const disposal = {
  all: undefined as Promise<void> | undefined,
}

export const Instance = {
  async provide<R>(input: { directory: string; init?: () => Promise<any>; fn: () => R }): Promise<R> {
    let existing = cache.get(input.directory)
    if (!existing) {
      Log.Default.info("instance: creating new promise", { directory: input.directory })
      existing = iife(async () => {
        Log.Default.info("instance: calling Project.fromDirectory", { directory: input.directory })
        const { project, sandbox } = await Project.fromDirectory(input.directory)
        const effectiveWorktree = Flag.GIZZI_WORKTREE || sandbox
        Log.Default.info("instance: project identified", { id: project.id, sandbox, effectiveWorktree })
        const ctx = {
          directory: input.directory,
          worktree: effectiveWorktree,
          project,
        }
        await context.provide(ctx, async () => {
          await input.init?.()
        })
        return ctx
      })
      cache.set(input.directory, existing)
    }
    const ctx = await existing
    return context.provide(ctx, async () => {
      return input.fn()
    })
  },
  get directory() {
    return context.use().directory
  },
  get worktree() {
    return context.use().worktree
  },
  /** Where this turn's tools resolve paths and run: the session's folder, else the instance directory. */
  get workdir() {
    try {
      return workdirContext.use()
    } catch {
      return context.use().directory
    }
  },
  withWorkdir<R>(directory: string, fn: () => R): R {
    return workdirContext.provide(directory, fn)
  },
  get project() {
    return context.use().project
  },
  /**
   * Check if a path is within the project boundary.
   * Returns true if path is inside Instance.directory OR Instance.worktree.
   * Paths within the worktree but outside the working directory should not trigger external_directory permission.
   * Uses realpath to resolve symlinks so a symlink inside the project pointing outside cannot escape.
   */
  containsPath(filepath: string) {
    // Resolve symlinks to prevent escape via symlink traversal.
    // Also normalizes cross-drive paths on Windows.
    let resolved: string
    try {
      resolved = fs.realpathSync(filepath)
    } catch {
      // If the path doesn't exist yet, fall back to lexical check
      resolved = path.resolve(filepath)
    }
    let resolvedDir: string
    try {
      resolvedDir = fs.realpathSync(Instance.directory)
    } catch {
      resolvedDir = Instance.directory
    }
    if (Filesystem.contains(resolvedDir, resolved)) return true
    const workdir = Instance.workdir
    if (workdir !== Instance.directory) {
      let resolvedWorkdir: string
      try {
        resolvedWorkdir = fs.realpathSync(workdir)
      } catch {
        resolvedWorkdir = workdir
      }
      if (Filesystem.contains(resolvedWorkdir, resolved)) return true
    }
    // Non-git projects set worktree to "/" which would match ANY absolute path.
    // Skip worktree check in this case to preserve external_directory permissions.
    if (Instance.worktree === "/") return false
    let resolvedWorktree: string
    try {
      resolvedWorktree = fs.realpathSync(Instance.worktree)
    } catch {
      resolvedWorktree = Instance.worktree
    }
    return Filesystem.contains(resolvedWorktree, resolved)
  },
  state<S>(init: () => S, dispose?: (state: Awaited<S>) => Promise<void>): () => S {
    return State.create(() => Instance.directory, init, dispose)
  },
  async dispose() {
    Log.Default.info("disposing instance", { directory: Instance.directory })
    await State.dispose(Instance.directory)
    cache.delete(Instance.directory)
    GlobalBus.emit("event", {
      directory: Instance.directory,
      payload: {
        type: "server.instance.disposed",
        properties: {
          directory: Instance.directory,
        },
      },
    })
  },
  async disposeAll() {
    if (disposal.all) return disposal.all

    disposal.all = iife(async () => {
      Log.Default.info("disposing all instances")
      const entries = [...cache.entries()]
      for (const [key, value] of entries) {
        if (cache.get(key) !== value) continue

        const ctx = await value.catch((error) => {
          Log.Default.warn("instance dispose failed", { key, error })
          return undefined
        })

        if (!ctx) {
          if (cache.get(key) === value) cache.delete(key)
          continue
        }

        if (cache.get(key) !== value) continue

        await context.provide(ctx, async () => {
          await Instance.dispose()
        })
      }
    }).finally(() => {
      disposal.all = undefined
    })

    return disposal.all
  },
}
