import path from "path"
import z from "zod/v4"
import { Config } from "@/runtime/context/config/config"
import { Instance } from "@/runtime/context/project/instance"
import { Filesystem } from "@/shared/util/filesystem"

/** Per-job model choice (`models.jobs` in config). Each job is optional:
 * an unset job returns undefined and the caller keeps the session model. */
export namespace ModelJobs {
  export const JOBS = [
    "chat",
    "explorer",
    "coder",
    "review",
    "goalJudge",
    "goalEscalate",
    "summarizer",
    "webResearch",
  ] as const
  export const Job = z.enum(JOBS)
  export type Job = z.infer<typeof Job>

  export const Jobs = z
    .object(Object.fromEntries(JOBS.map((job) => [job, z.string().min(1).optional()])) as Record<Job, z.ZodOptional<z.ZodString>>)
  export type Jobs = z.infer<typeof Jobs>

  export type ModelRef = { providerID: string; modelID: string }

  function parse(ref: string): ModelRef | undefined {
    const [providerID, ...rest] = ref.trim().split("/")
    const modelID = rest.join("/")
    if (!providerID || !modelID) return undefined
    return { providerID, modelID }
  }

  /** The configured model for a job, or undefined (fall back to the session model). */
  export async function jobModel(job: Job): Promise<ModelRef | undefined> {
    const cfg = await Config.get().catch(() => undefined)
    const ref = cfg?.models?.jobs?.[job]
    if (!ref) return undefined
    // Provider.parseModel resolves aliases ("sonnet" → provider/model); import
    // lazily so config stays free of a provider import cycle.
    const { Provider } = await import("@/runtime/providers/provider")
    const resolved = Provider.parseModel(ref)
    return resolved.providerID && resolved.modelID ? resolved : parse(ref)
  }

  /** Subagent name → job. Only the two native subagents map; custom agents keep their own model. */
  export function jobForAgent(agentName: string): Job | undefined {
    if (agentName === "explore") return "explorer"
    if (agentName === "general") return "coder"
    return undefined
  }

  export async function get(): Promise<Jobs> {
    const cfg = await Config.get()
    return { ...(cfg.models?.jobs ?? {}) }
  }

  /** Replace the project's `models.jobs` wholesale (a job left out is cleared).
   * Writes the same project file `Config.update` writes. */
  export async function set(jobs: Jobs): Promise<Jobs> {
    const cleaned = Object.fromEntries(
      Object.entries(Jobs.parse(jobs)).filter(([, value]) => typeof value === "string" && value.trim()),
    ) as Jobs
    const filepath = path.join(Instance.directory, "config.json")
    const existing: Record<string, any> = (await Filesystem.readJson<Record<string, any>>(filepath).catch(() => ({}) as Record<string, any>)) ?? {}
    const models = existing.models && typeof existing.models === "object" ? existing.models : {}
    existing.models = { ...models, jobs: cleaned }
    await Filesystem.writeJson(filepath, existing)
    await Instance.dispose()
    return cleaned
  }
}
