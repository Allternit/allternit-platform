import { describe, expect, it } from "vitest";
import {
  DEFAULT_PARAMS,
  assembledToJson,
  buildRequestBody,
  computeCostUsd,
  exportRequest,
  imageContent,
  mergeChunk,
  messagesToEditable,
  paramsFromBody,
  newAssembly,
  toolNames,
} from "../playground-request";

const msgs = [{ role: "user" as const, content: "hi" }];
const tools = JSON.stringify([
  { type: "function", function: { name: "get_weather", parameters: { type: "object" } } },
]);

describe("buildRequestBody", () => {
  it("emits only set params", () => {
    const { body, errors } = buildRequestBody("kimi-k3", msgs, DEFAULT_PARAMS);
    expect(errors).toEqual([]);
    expect(body).toMatchObject({ model: "kimi-k3", temperature: 0.7, max_tokens: 4096, stream: true });
    expect(body).not.toHaveProperty("top_p");
    expect(body).not.toHaveProperty("tools");
    expect(body.stream_options).toEqual({ include_usage: true });
  });

  it("maps all params, stop lines, tools and tool_choice", () => {
    const { body } = buildRequestBody("m", msgs, {
      ...DEFAULT_PARAMS,
      topP: 0.9,
      stop: "END\n\nSTOP",
      presencePenalty: 0.5,
      frequencyPenalty: -0.5,
      seed: 7,
      stream: false,
      toolsJson: tools,
      toolChoice: "fn:get_weather",
    });
    expect(body.top_p).toBe(0.9);
    expect(body.stop).toEqual(["END", "STOP"]);
    expect(body.seed).toBe(7);
    expect(body.presence_penalty).toBe(0.5);
    expect(body.tool_choice).toEqual({ type: "function", function: { name: "get_weather" } });
    expect(body).not.toHaveProperty("stream");
  });

  it("builds json_schema and reports bad JSON", () => {
    const ok = buildRequestBody("m", msgs, {
      ...DEFAULT_PARAMS,
      responseFormat: "json_schema",
      schemaName: "out",
      schemaJson: '{"type":"object"}',
    });
    expect(ok.body.response_format).toEqual({
      type: "json_schema",
      json_schema: { name: "out", strict: true, schema: { type: "object" } },
    });
    const bad = buildRequestBody("m", msgs, { ...DEFAULT_PARAMS, toolsJson: "{nope" });
    expect(bad.errors[0]).toMatch(/Tools is not valid JSON/);
    expect(bad.body).not.toHaveProperty("tools");
  });
});

describe("helpers", () => {
  it("lists tool names", () => {
    expect(toolNames(tools)).toEqual(["get_weather"]);
    expect(toolNames("garbage")).toEqual([]);
  });

  it("builds image content parts", () => {
    expect(imageContent("a", [])).toBe("a");
    expect(imageContent("a", ["data:image/png;base64,x"])).toEqual([
      { type: "text", text: "a" },
      { type: "image_url", image_url: { url: "data:image/png;base64,x" } },
    ]);
  });

  it("prices a call per token", () => {
    expect(computeCostUsd({ prompt_tokens: 1000, completion_tokens: 100 }, 4.5e-6, 2.25e-5)).toBeCloseTo(0.00675);
    expect(computeCostUsd({ prompt_tokens: 1 }, undefined, undefined)).toBeNull();
  });
});

describe("mergeChunk", () => {
  it("assembles content and split tool call arguments", () => {
    let a = newAssembly();
    a = mergeChunk(a, { id: "c1", model: "m", choices: [{ index: 0, delta: { content: "Hel" } }] });
    a = mergeChunk(a, { choices: [{ index: 0, delta: { content: "lo" } }] });
    a = mergeChunk(a, {
      choices: [{ index: 0, delta: { tool_calls: [{ index: 0, id: "t1", function: { name: "get_weather", arguments: '{"ci' } }] } }],
    });
    a = mergeChunk(a, {
      choices: [{ index: 0, delta: { tool_calls: [{ index: 0, function: { arguments: 'ty":"Oslo"}' } }] }, finish_reason: "tool_calls" }],
      usage: { prompt_tokens: 3, completion_tokens: 4 },
    });
    expect(a.content).toBe("Hello");
    expect(a.toolCalls).toEqual([
      { id: "t1", type: "function", function: { name: "get_weather", arguments: '{"city":"Oslo"}' } },
    ]);
    expect(a.finishReason).toBe("tool_calls");
    expect(assembledToJson(a)).toMatchObject({ id: "c1", usage: { prompt_tokens: 3 } });
  });
});

describe("exportRequest", () => {
  const { body } = buildRequestBody("kimi-k3", msgs, { ...DEFAULT_PARAMS, stop: "it's" });
  it("curl uses the placeholder and escapes quotes", () => {
    const out = exportRequest("curl", body, "https://api.allternit.com/");
    expect(out).toContain("https://api.allternit.com/v1/chat/completions");
    expect(out).toContain("Bearer $ALLTERNIT_API_KEY");
    expect(out).toContain(`it'\\''s`);
  });
  it("python uses openai SDK with base_url and python literals", () => {
    const out = exportRequest("python", { ...body, stream: false }, "https://api.allternit.com");
    expect(out).toContain("from openai import OpenAI");
    expect(out).toContain('base_url="https://api.allternit.com/v1"');
    expect(out).toContain('os.environ["ALLTERNIT_API_KEY"]');
    expect(out).toContain('"role": "user"');
    expect(out).not.toContain("stream_options");
  });
  it("typescript uses baseURL and env key", () => {
    const out = exportRequest("typescript", body, "https://api.allternit.com");
    expect(out).toContain('baseURL: "https://api.allternit.com/v1"');
    expect(out).toContain("process.env.ALLTERNIT_API_KEY");
    expect(out).toContain("stream: true");
    expect(out).not.toMatch(/\balt_[a-z0-9]{6}/i);
  });
});

describe("replay", () => {
  it("round-trips a request body into editor state", () => {
    const { body } = buildRequestBody(
      "m",
      [
        { role: "system", content: "be brief" },
        { role: "user", content: imageContent("look", ["data:image/png;base64,abc"]) },
      ],
      { ...DEFAULT_PARAMS, topP: 0.5, toolsJson: tools, toolChoice: "required", responseFormat: "json_object", stream: false }
    );
    const m = messagesToEditable(body.messages);
    expect(m.systemPrompt).toBe("be brief");
    expect(m.messages[0]).toMatchObject({ role: "user", content: "look", images: ["data:image/png;base64,abc"] });
    const p = paramsFromBody(body);
    expect(p).toMatchObject({ topP: 0.5, toolChoice: "required", responseFormat: "json_object", stream: false });
    expect(toolNames(p.toolsJson)).toEqual(["get_weather"]);
  });
});
