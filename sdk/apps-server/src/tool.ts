import type { ZodRawShape } from "zod";
import type { ViewDef } from "./view.js";

export interface ToolAnnotationsInput {
  /** Required: the tool only reads. */
  readOnlyHint: boolean;
  /** Required: the tool can delete or overwrite data it cannot restore. */
  destructiveHint: boolean;
  openWorldHint?: boolean;
  idempotentHint?: boolean;
}

export interface ToolResultShape {
  content: Array<{ type: "text"; text: string }>;
  structuredContent?: Record<string, unknown>;
  isError?: boolean;
}

export interface AppToolInput<Shape extends ZodRawShape = ZodRawShape> {
  name: string;
  title: string;
  description: string;
  input?: Shape;
  annotations: ToolAnnotationsInput;
  /** The View this tool renders into: a `view()` or its `ui://` URI. */
  view?: ViewDef | string;
  handler: (args: { [K in keyof Shape]: Shape[K]["_output"] }) => ToolResultShape | Promise<ToolResultShape>;
}

export interface AppToolDef extends Omit<AppToolInput, "view"> {
  viewUri?: string;
}

/**
 * Declare a tool. Unlike the raw SDK call, annotations are not optional: the
 * author has to say whether the tool reads or writes and whether it can destroy
 * data, because the host's permission prompts and directory review depend on it.
 */
export function appTool<Shape extends ZodRawShape = ZodRawShape>(input: AppToolInput<Shape>): AppToolDef {
  const where = `tool ${input.name || "(unnamed)"}`;
  if (!input.name) throw new Error("tool: name is required");
  if (!input.title?.trim()) throw new Error(`${where}: title is required`);
  if (!input.description?.trim()) throw new Error(`${where}: description is required`);
  const a = input.annotations as Partial<ToolAnnotationsInput> | undefined;
  if (typeof a?.readOnlyHint !== "boolean") throw new Error(`${where}: annotations.readOnlyHint must be stated (true or false)`);
  if (typeof a?.destructiveHint !== "boolean") throw new Error(`${where}: annotations.destructiveHint must be stated (true or false)`);
  if (a.readOnlyHint && a.destructiveHint) throw new Error(`${where}: a tool cannot be both readOnlyHint and destructiveHint`);
  const { view, ...rest } = input;
  return { ...(rest as AppToolInput), viewUri: typeof view === "string" ? view : view?.uri };
}
