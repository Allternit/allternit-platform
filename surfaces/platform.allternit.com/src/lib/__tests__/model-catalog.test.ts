import { describe, expect, it } from "vitest";
import {
  liveModelToCatalog,
  modelCapabilities,
  type LiveModelInfo,
} from "../model-catalog";

// Rows from the live `GET https://api.allternit.com/v1/models` (2026-11-12),
// trimmed of fields the console does not read (benchmarks, description, ...).
const fireworks: LiveModelInfo = {
  id: "kimi-k3",
  object: "model",
  created: 1753929600,
  owned_by: "fireworks",
  upstream_id: "accounts/fireworks/models/kimi-k3",
  provider: "fireworks",
  completion_price: 2.25e-5,
  context_length: 1048576,
  default: true,
  lane: "standard",
  name: "Kimi K3",
  prompt_price: 4.5e-6,
  supports_chat: true,
  supports_image_input: true,
  supports_tools: true,
} as LiveModelInfo;

const openrouter: LiveModelInfo = {
  id: "claude-sonnet-5.5",
  object: "model",
  created: 1790618686,
  owned_by: "openrouter",
  upstream_id: "anthropic/claude-sonnet-5.5",
  provider: "openrouter",
  architecture: { input_modalities: ["text", "image", "file"] },
  completion_price: 1.5e-5,
  context_length: 1000000,
  default: false,
  expiration_date: null,
  lane: "standard",
  name: "Anthropic: Claude Sonnet 5.5",
  prompt_price: 3e-6,
  supported_parameters: ["max_tokens", "reasoning", "response_format", "structured_outputs", "tools"],
};

const together: LiveModelInfo = {
  id: "deepseek-v4-pro",
  object: "model",
  created: 1753929600,
  owned_by: "together",
  upstream_id: "deepseek-ai/DeepSeek-V4-Pro-0813",
  provider: "together",
  aliases: ["deepseek-v4"],
  completion_price: 5.940000000000001e-6,
  context_length: 1048576,
  default: false,
  lane: "standard",
  name: "DeepSeek V4 Pro 0813",
  prompt_price: 1.98e-6,
};

describe("liveModelToCatalog", () => {
  it("maps a Fireworks row: flags, per-1M prices, context, default", () => {
    const m = liveModelToCatalog(fireworks);
    expect(m.name).toBe("Kimi K3");
    expect(m.upstreamProvider).toBe("fireworks");
    expect(m.inputPrice).toBe("$4.50");
    expect(m.outputPrice).toBe("$22.50");
    expect(m.context).toBe("1.0M");
    expect(m.isDefault).toBe(true);
    expect(m.capabilities).toEqual(["Tools", "Vision"]);
    expect(m.retires).toBeUndefined();
  });

  it("maps an OpenRouter row from supported_parameters and architecture", () => {
    const m = liveModelToCatalog(openrouter);
    expect(m.inputPrice).toBe("$3.00");
    expect(m.outputPrice).toBe("$15.00");
    expect(m.context).toBe("1.0M");
    expect(m.isDefault).toBe(false);
    expect(m.capabilities).toEqual(["Tools", "Vision", "JSON"]);
    expect(m.retires).toBeUndefined();
  });

  it("shows no badges for a Together row without flags, and cleans float noise", () => {
    const m = liveModelToCatalog(together);
    expect(m.capabilities).toEqual([]);
    expect(m.inputPrice).toBe("$1.98");
    expect(m.outputPrice).toBe("$5.94");
  });

  it("falls back to the id and shows a dash for missing price or context", () => {
    const m = liveModelToCatalog({ id: "x-1", object: "model", created: 0, owned_by: "together" });
    expect(m.name).toBe("x-1");
    expect(m.inputPrice).toBe("—");
    expect(m.outputPrice).toBe("—");
    expect(m.context).toBe("—");
  });

  it("formats context in K below one million", () => {
    expect(liveModelToCatalog({ ...together, context_length: 200000 }).context).toBe("200K");
  });

  it("adds a Retires badge only for a non-null date", () => {
    expect(liveModelToCatalog({ ...openrouter, expiration_date: "2026-12-01" }).retires).toBe(
      "Dec 1, 2026",
    );
    expect(liveModelToCatalog({ ...openrouter, expiration_date: null }).retires).toBeUndefined();
    expect(liveModelToCatalog({ ...openrouter, expiration_date: "soon" }).retires).toBeUndefined();
  });
});

describe("modelCapabilities", () => {
  it("does not infer a capability from a false flag", () => {
    expect(
      modelCapabilities({ ...fireworks, supports_tools: false, supports_image_input: false }),
    ).toEqual([]);
  });

  it("treats structured_outputs alone as JSON", () => {
    expect(modelCapabilities({ ...together, supported_parameters: ["structured_outputs"] })).toEqual([
      "JSON",
    ]);
  });
});
