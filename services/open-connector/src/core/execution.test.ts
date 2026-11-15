import type { ActionDefinition, ExecutionContext } from "./types.ts";

import { describe, expect, it } from "vitest";
import { executeAction } from "./execution.ts";
import { withSavedConnectionInputs } from "./provider-definition.ts";

const action: ActionDefinition = {
  id: "example.echo",
  service: "example",
  name: "echo",
  description: "Echo input.",
  requiredScopes: [],
  providerPermissions: [],
  inputSchema: {
    type: "object",
    required: ["message"],
    properties: {
      message: { type: "string" },
    },
  },
  outputSchema: { type: "object" },
};

const context: ExecutionContext = {
  async getCredential() {
    return undefined;
  },
};

describe("executeAction", () => {
  it("returns executor_unavailable before validating catalog-only actions", async () => {
    await expect(executeAction(action, undefined, {}, context)).resolves.toMatchObject({
      ok: false,
      error: {
        code: "executor_unavailable",
      },
    });
  });

  it("validates input when an executor is available", async () => {
    await expect(executeAction(action, async () => ({ ok: true, output: {} }), {}, context)).resolves.toMatchObject({
      ok: false,
      error: {
        code: "invalid_input",
      },
    });
  });

  describe("saved connection inputs", () => {
    const [savedAction] = withSavedConnectionInputs(
      [
        {
          ...action,
          inputSchema: {
            type: "object",
            required: ["workspace", "message"],
            properties: {
              workspace: { type: "string", description: "Workspace slug." },
              message: { type: "string" },
            },
          },
        },
      ],
      { workspace: ["workspace", "workspaceSlug"] },
    );
    const echo = async (input: unknown) => ({ ok: true, output: input });
    const connectedContext = (values: Record<string, string>, metadata: Record<string, unknown>): ExecutionContext => ({
      async getCredential() {
        return {
          authType: "api_key",
          apiKey: "key",
          values,
          profile: { accountId: "acct", displayName: "Account" },
          metadata,
        } as never;
      },
    });

    it("makes the saved input optional in the schema", () => {
      expect(savedAction?.inputSchema.required).toEqual(["message"]);
      expect(savedAction?.savedConnectionInputs).toEqual({ workspace: ["workspace", "workspaceSlug"] });
    });

    it("fills an omitted input from saved values or validation metadata", async () => {
      await expect(
        executeAction(savedAction!, echo, { message: "hi" }, connectedContext({ workspace: "acme" }, {})),
      ).resolves.toEqual({ ok: true, output: { message: "hi", workspace: "acme" } });
      await expect(
        executeAction(savedAction!, echo, { message: "hi" }, connectedContext({}, { workspaceSlug: 42 })),
      ).resolves.toEqual({ ok: true, output: { message: "hi", workspace: "42" } });
    });

    it("keeps an explicit input", async () => {
      await expect(
        executeAction(
          savedAction!,
          echo,
          { message: "hi", workspace: "own" },
          connectedContext({ workspace: "acme" }, {}),
        ),
      ).resolves.toEqual({ ok: true, output: { message: "hi", workspace: "own" } });
    });

    it("rejects when the input is neither passed nor saved", async () => {
      await expect(executeAction(savedAction!, echo, { message: "hi" }, context)).resolves.toMatchObject({
        ok: false,
        error: { code: "invalid_input" },
      });
    });
  });
});
