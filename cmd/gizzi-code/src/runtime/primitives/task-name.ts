/**
 * Task names, fork and effort (Primitives Wave B, lane L2).
 *
 * - TaskName: a child's name lives on its session (`session.task_name`) as a
 *   path such as "/root/build/tests", at most TASK_NAME_MAX_DEPTH levels,
 *   unique among siblings. `task_id` and agent_control targets accept a path.
 * - TaskFork: `fork: "all"` copies the parent's history up to its last
 *   finished message and refuses a model or effort override.
 * - TaskEffort: low/medium/high maps to the model's existing `variant`.
 *
 * Contract stub (B1): every function throws until L2 fills it in. Nothing
 * calls these yet; task.ts and agent-swarm.ts wire them in L2.
 */
import {
  notImplemented,
  type TaskEffort as TaskEffortValue,
  type TaskFork as TaskForkValue,
} from "@/runtime/primitives/wave-b-types"

export namespace TaskName {
  /** `name` when it matches TASK_NAME_PATTERN; throws a model-readable error otherwise. */
  export function validate(_name: string): string {
    return notImplemented("TaskName.validate")
  }

  /** Stores the child's path (parent path + "/" + name) on the child session and returns it. Throws when taken or too deep. */
  export function assign(_input: { parentSessionID: string; childSessionID: string; name: string }): Promise<string> {
    return notImplemented("TaskName.assign")
  }

  /** The descendant session id of `sessionID` that `target` (an id or a name path) names, or undefined. */
  export function resolve(_input: { sessionID: string; target: string }): Promise<string | undefined> {
    return notImplemented("TaskName.resolve")
  }

  /** The stored path of a session, or null when it has no name. */
  export function pathOf(_sessionID: string): Promise<string | null> {
    return notImplemented("TaskName.pathOf")
  }
}

export namespace TaskFork {
  /** Throws when `fork: "all"` comes with a model or effort override. */
  export function check(_input: { fork?: TaskForkValue; model?: string; effort?: TaskEffortValue }): void {
    return notImplemented("TaskFork.check")
  }

  /** Copies the parent's messages up to its last finished one into the child session. Returns how many were copied. */
  export function copyHistory(_input: { parentSessionID: string; childSessionID: string }): Promise<number> {
    return notImplemented("TaskFork.copyHistory")
  }
}

export namespace TaskEffort {
  /** The model's variant name for an effort, or undefined when the model has no matching variant. */
  export function variant(_input: { effort: TaskEffortValue; providerID: string; modelID: string }): Promise<string | undefined> {
    return notImplemented("TaskEffort.variant")
  }
}
