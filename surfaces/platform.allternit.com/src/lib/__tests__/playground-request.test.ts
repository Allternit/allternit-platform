import { describe, expect, it } from "vitest";
import {
  DEFAULT_PARAMS,
  assembledToJson,
  buildModeRequestBody,
  buildRequestBody,
  computeCostUsd,
  exportModeRequest,
  mergeModeEvent,
  modeAssemblyToJson,
  newModeAssembly,
  readModeReply,
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

describe("API modes", () => {
  const editor = [
    { role: "system" as const, content: "be brief" },
    { role: "user" as const, content: "hi" },
    { role: "assistant" as const, content: "hello" },
    { role: "user" as const, content: "weather?" },
  ];
  const params = { ...DEFAULT_PARAMS, maxTokens: 30, temperature: 0.2, topP: 0.9, toolsJson: tools, toolChoice: "required" };

  it("chat mode is exactly buildRequestBody", () => {
    const a = buildModeRequestBody("chat", "m", editor, params);
    expect(a.body).toEqual(buildRequestBody("m", editor, params).body);
    expect(a.ignored).toEqual([]);
  });

  it("responses maps system to instructions and max tokens to max_output_tokens", () => {
    const { body, errors, ignored } = buildModeRequestBody("responses", "m", editor, params);
    expect(errors).toEqual([]);
    expect(body).toEqual({
      model: "m",
      input: [
        { role: "user", content: "hi" },
        { role: "assistant", content: "hello" },
        { role: "user", content: "weather?" },
      ],
      instructions: "be brief",
      max_output_tokens: 30,
      temperature: 0.2,
      tools: [{ type: "function", name: "get_weather", parameters: { type: "object" } }],
      tool_choice: "required",
      stream: true,
    });
    expect(ignored).toEqual(["top_p"]);
  });

  it("responses with previous_response_id sends only the new turn", () => {
    const { body } = buildModeRequestBody("responses", "m", editor, DEFAULT_PARAMS, { previousResponseId: "resp_1" });
    expect(body.previous_response_id).toBe("resp_1");
    expect(body.input).toEqual([{ role: "user", content: "weather?" }]);
    const none = buildModeRequestBody("responses", "m", editor.slice(0, 3), DEFAULT_PARAMS, { previousResponseId: "resp_1" });
    expect(none.errors[0]).toMatch(/continue resp_1/);
  });

  it("messages maps system, max_tokens, tools, tool calls and results", () => {
    const { body } = buildModeRequestBody(
      "messages",
      "m",
      [
        ...editor,
        { role: "assistant", content: null, tool_calls: [{ id: "t1", type: "function", function: { name: "get_weather", arguments: '{"city":"Oslo"}' } }] },
        { role: "tool", content: "18C", tool_call_id: "t1" },
      ],
      { ...params, toolChoice: "fn:get_weather", stream: false }
    );
    expect(body).toEqual({
      model: "m",
      max_tokens: 30,
      system: "be brief",
      messages: [
        { role: "user", content: "hi" },
        { role: "assistant", content: "hello" },
        { role: "user", content: "weather?" },
        { role: "assistant", content: [{ type: "tool_use", id: "t1", name: "get_weather", input: { city: "Oslo" } }] },
        { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: "18C" }] },
      ],
      temperature: 0.2,
      tools: [{ name: "get_weather", input_schema: { type: "object" } }],
      tool_choice: { type: "tool", name: "get_weather" },
    });
  });

  it("parses a Responses stream that ends incomplete", () => {
    const events: Array<[string, unknown]> = [
      ["response.created", { type: "response.created", response: { id: "resp_9", model: "m", status: "in_progress", output: [] } }],
      ["response.output_text.delta", { type: "response.output_text.delta", delta: "Hel" }],
      ["response.output_text.delta", { type: "response.output_text.delta", delta: "lo" }],
      [
        "response.incomplete",
        {
          type: "response.incomplete",
          response: {
            id: "resp_9",
            object: "response",
            model: "m",
            status: "incomplete",
            output: [{ type: "message", role: "assistant", content: [{ type: "output_text", text: "Hello" }] }],
            incomplete_details: { reason: "max_output_tokens" },
            usage: { input_tokens: 5, output_tokens: 30 },
          },
        },
      ],
    ];
    let a = newModeAssembly();
    a = mergeModeEvent("responses", a, ...events[0]);
    a = mergeModeEvent("responses", a, ...events[1]);
    a = mergeModeEvent("responses", a, ...events[2]);
    expect(a.content).toBe("Hello");
    a = mergeModeEvent("responses", a, ...events[3]);
    expect(a).toMatchObject({
      id: "resp_9",
      content: "Hello",
      finishReason: "incomplete",
      incompleteReason: "max_output_tokens",
      usage: { prompt_tokens: 5, completion_tokens: 30, total_tokens: 35 },
      error: null,
    });
    expect(modeAssemblyToJson("responses", a)).toMatchObject({ id: "resp_9", status: "incomplete" });
  });

  it("parses a Responses function call and a failed stream", () => {
    let a = newModeAssembly();
    a = mergeModeEvent("responses", a, "response.output_item.done", {
      item: { type: "function_call", call_id: "call_1", name: "get_weather", arguments: '{"city":"Oslo"}' },
    });
    expect(a.toolCalls).toEqual([{ id: "call_1", type: "function", function: { name: "get_weather", arguments: '{"city":"Oslo"}' } }]);
    a = mergeModeEvent("responses", a, "response.failed", { response: { id: "resp_2", status: "failed", output: [], error: { message: "model down" } } });
    expect(a.error).toBe("model down");
    expect(a.toolCalls).toHaveLength(1);
  });

  it("parses a Messages stream with text deltas, a tool block and stop_reason", () => {
    const events: Array<[string, unknown]> = [
      ["message_start", { type: "message_start", message: { id: "mdl_1", model: "m", content: [], usage: { input_tokens: 0, output_tokens: 0 } } }],
      ["content_block_start", { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } }],
      ["content_block_delta", { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "Hi " } }],
      ["content_block_delta", { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "there" } }],
      ["content_block_stop", { type: "content_block_stop", index: 0 }],
      ["content_block_start", { type: "content_block_start", index: 1, content_block: { type: "tool_use", id: "toolu_1", name: "f", input: {} } }],
      ["content_block_delta", { type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: '{"a"' } }],
      ["content_block_delta", { type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: ":1}" } }],
      ["message_delta", { type: "message_delta", delta: { stop_reason: "max_tokens" }, usage: { input_tokens: 7, output_tokens: 30 } }],
      ["message_stop", { type: "message_stop" }],
    ];
    const a = events.reduce((acc, [e, d]) => mergeModeEvent("messages", acc, e, d), newModeAssembly());
    expect(a).toMatchObject({
      id: "mdl_1",
      content: "Hi there",
      finishReason: "max_tokens",
      usage: { prompt_tokens: 7, completion_tokens: 30, total_tokens: 37 },
    });
    expect(a.toolCalls).toEqual([{ id: "toolu_1", type: "function", function: { name: "f", arguments: '{"a":1}' } }]);
    expect(modeAssemblyToJson("messages", a)).toMatchObject({
      content: [{ type: "text", text: "Hi there" }, { type: "tool_use", id: "toolu_1", input: { a: 1 } }],
      stop_reason: "max_tokens",
    });
    const err = mergeModeEvent("messages", a, "error", { error: { type: "api_error", message: "model failed" } });
    expect(err.error).toBe("model failed");
  });

  it("reads non-streamed Responses and Messages bodies", () => {
    expect(
      readModeReply("responses", {
        id: "resp_3",
        status: "completed",
        output: [{ type: "message", content: [{ type: "output_text", text: "ok" }] }],
        usage: { input_tokens: 1, output_tokens: 2 },
        incomplete_details: null,
      })
    ).toMatchObject({ content: "ok", finishReason: "completed", incompleteReason: null, usage: { total_tokens: 3 } });
    expect(
      readModeReply("messages", {
        id: "mdl_2",
        content: [{ type: "text", text: "yo" }, { type: "tool_use", id: "t", name: "f", input: { x: 1 } }],
        stop_reason: "tool_use",
        usage: { input_tokens: 4, output_tokens: 5 },
      })
    ).toMatchObject({ content: "yo", finishReason: "tool_use", toolCalls: [{ id: "t", function: { name: "f", arguments: '{"x":1}' } }] });
  });

  it("exports the right path and SDK per mode", () => {
    const root = "https://api.allternit.com/";
    const chat = buildModeRequestBody("chat", "m", editor, DEFAULT_PARAMS).body;
    expect(exportModeRequest("chat", "curl", chat, root)).toBe(exportRequest("curl", chat, root));
    const resp = buildModeRequestBody("responses", "m", editor, DEFAULT_PARAMS).body;
    expect(exportModeRequest("responses", "curl", resp, root)).toContain("curl https://api.allternit.com/v1/responses \\");
    const py = exportModeRequest("responses", "python", resp, root);
    expect(py).toContain("from allternit_platform import AllternitPlatform");
    expect(py).toContain("client.responses.create_stream(");
    expect(py).toContain('instructions="be brief"');
    expect(py).not.toContain("stream=");
    const msg = buildModeRequestBody("messages", "m", editor, { ...DEFAULT_PARAMS, stream: false }).body;
    expect(exportModeRequest("messages", "curl", msg, root)).toContain("/v1/messages");
    const ts = exportModeRequest("messages", "typescript", msg, root);
    expect(ts).toContain('import { AllternitPlatform } from "@allternit/platform";');
    expect(ts).toContain("await client.messages.create({");
    expect(ts).toContain("max_tokens: 4096");
    expect(ts).toContain("process.env.ALLTERNIT_API_KEY");
  });
});
