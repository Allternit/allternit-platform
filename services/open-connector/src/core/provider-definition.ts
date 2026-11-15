import type { ActionDefinition, JsonSchema } from "./types.ts";

/**
 * Input for defining one provider action without repeating provider-level
 * fields in every action object.
 */
export type DefineProviderActionInput<TName extends string = string> = {
  name: TName;
  description: string;
  inputSchema: JsonSchema;
  outputSchema: JsonSchema;
  requiredScopes?: string[];
  providerPermissions?: string[];
  followUpActions?: string[];
  asyncLifecycle?: ActionDefinition["asyncLifecycle"];
};

export type ProviderActionDefinition<TName extends string = string> = ActionDefinition & { name: TName };

/**
 * Create a full action definition for one provider.
 *
 * Provider modules use this helper so definitions read as business action
 * declarations instead of generated catalog JSON.
 */
export function defineProviderAction<TName extends string>(
  service: string,
  input: DefineProviderActionInput<TName>,
): ProviderActionDefinition<TName> {
  return {
    id: `${service}.${input.name}`,
    service,
    name: input.name,
    description: input.description,
    requiredScopes: input.requiredScopes ?? [],
    providerPermissions: input.providerPermissions ?? [],
    inputSchema: input.inputSchema,
    outputSchema: input.outputSchema,
    followUpActions: input.followUpActions,
    asyncLifecycle: input.asyncLifecycle,
  };
}

/**
 * Let actions reuse values the connection already saved, such as a workspace
 * slug, account id, or developer token entered at connect time or recorded by
 * credential validation.
 *
 * Each listed input that an action requires becomes optional in its input
 * schema, and the runtime fills it from the connection when the caller omits
 * it. `inputs` maps an action input key to the credential value or metadata
 * fields that hold the saved value, checked in order. Actions that do not
 * require a listed input are returned unchanged.
 */
export function withSavedConnectionInputs<TAction extends ActionDefinition>(
  actions: TAction[],
  inputs: Record<string, string[]>,
): TAction[] {
  return actions.map((action) => {
    const required = Array.isArray(action.inputSchema.required) ? (action.inputSchema.required as string[]) : [];
    const savedKeys = Object.keys(inputs).filter((key) => required.includes(key));
    if (savedKeys.length === 0) {
      return action;
    }

    const properties = { ...(action.inputSchema.properties as Record<string, JsonSchema> | undefined) };
    for (const key of savedKeys) {
      const property = properties[key] ?? {};
      const description = typeof property.description === "string" ? `${property.description} ` : "";
      properties[key] = { ...property, description: `${description}Defaults to the value saved on the connection.` };
    }

    return {
      ...action,
      inputSchema: {
        ...action.inputSchema,
        properties,
        required: required.filter((key) => !savedKeys.includes(key)),
      },
      savedConnectionInputs: {
        ...action.savedConnectionInputs,
        ...Object.fromEntries(savedKeys.map((key) => [key, inputs[key]])),
      },
    };
  });
}
