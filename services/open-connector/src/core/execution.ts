import type { ActionPolicyService } from "./action-policy.ts";
import type {
  ActionDefinition,
  ActionExecutor,
  ExecutionContext,
  ExecutionResult,
  ResolvedCredential,
} from "./types.ts";

import { optionalRecord, optionalScalarString } from "./cast.ts";
import { validateActionInput } from "./validation.ts";

/**
 * Validate input and run a local executor for an action.
 *
 * When `executor` is undefined, the action is treated as catalog-only and a
 * stable `executor_unavailable` error is returned.
 */
export async function executeAction(
  action: ActionDefinition,
  executor: ActionExecutor | undefined,
  input: unknown,
  context: ExecutionContext,
  policy?: ActionPolicyService,
): Promise<ExecutionResult> {
  const decision = policy?.evaluate(action);
  if (decision && !decision.allowed) {
    return {
      ok: false,
      error: {
        code: decision.code,
        message: decision.message,
      },
    };
  }

  if (!executor) {
    return {
      ok: false,
      error: {
        code: "executor_unavailable",
        message: "Execution for this action is not available in the local runtime yet.",
      },
    };
  }

  const validation = validateActionInput(action, input);
  if (!validation.valid) {
    return {
      ok: false,
      error: {
        code: "invalid_input",
        message: "Action input does not match the action schema.",
        details: validation.errors,
      },
    };
  }

  const filled = await fillSavedConnectionInputs(action, input, context);
  if (filled.missing.length > 0) {
    return {
      ok: false,
      error: {
        code: "invalid_input",
        message: `Pass ${filled.missing.join(", ")} or save it on the ${action.service} connection.`,
        details: filled.missing.map((key) => ({ path: key, message: "missing and not saved on the connection" })),
      },
    };
  }

  return executor(filled.input, context);
}

interface FilledActionInput {
  input: unknown;
  missing: string[];
}

/**
 * Fill omitted inputs that the action declares as saved on the connection.
 */
async function fillSavedConnectionInputs(
  action: ActionDefinition,
  input: unknown,
  context: ExecutionContext,
): Promise<FilledActionInput> {
  const saved = action.savedConnectionInputs;
  const record = optionalRecord(input);
  if (!saved || !record) {
    return { input, missing: [] };
  }

  const omitted = Object.keys(saved).filter((key) => isOmitted(record[key]));
  if (omitted.length === 0) {
    return { input, missing: [] };
  }

  const credential = await context.getCredential(action.service);
  const filled: Record<string, unknown> = { ...record };
  const missing: string[] = [];
  for (const key of omitted) {
    const value = readSavedConnectionValue(credential, saved[key] ?? []);
    if (value === undefined) {
      missing.push(key);
    } else {
      filled[key] = value;
    }
  }

  return { input: filled, missing };
}

/**
 * Read the first non-empty saved value for `fields` from credential values,
 * validation metadata, or OAuth client config extras.
 */
function readSavedConnectionValue(credential: ResolvedCredential | undefined, fields: string[]): string | undefined {
  if (!credential || credential.authType === "no_auth") {
    return undefined;
  }

  const sources = [
    "values" in credential ? credential.values : undefined,
    credential.metadata,
    optionalRecord(credential.metadata.oauthClientExtra),
    optionalRecord(credential.metadata.oauthClientSecretExtra),
  ];
  for (const field of fields) {
    for (const source of sources) {
      const value = optionalScalarString(source?.[field])?.trim();
      if (value) {
        return value;
      }
    }
  }

  return undefined;
}

function isOmitted(value: unknown): boolean {
  return value === undefined || value === null || (typeof value === "string" && value.trim() === "");
}
