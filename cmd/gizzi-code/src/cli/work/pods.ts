import { gateway } from "@/cli/work/api"
import type { FloorState } from "@/cli/work/pods-model"

/** Pods = the Office floor's grouping (allternit-api `/api/v1/floor/state`, floor v2). */
export const getFloorState = () => gateway<FloorState>("/api/v1/floor/state")
