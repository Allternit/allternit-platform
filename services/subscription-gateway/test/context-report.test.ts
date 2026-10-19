import { describe, expect, it } from "vitest";
import { chatgptContextReport, claudeContextReport, tokensOf } from "../adapters/_shared/context-report.js";

describe("claudeContextReport", () => {
  it("fills messages, memory (memory + project), skills, tools and limits from claude.ai's endpoints", () => {
    const r = claudeContextReport({
      conversation: {
        model: "claude-opus-5-5",
        settings: { enabled_web_search: true, enabled_artifacts: false },
        project_uuid: "p",
        chat_messages: [
          { sender: "human", content: [{ type: "text", text: "hello there" }], attachments: [{ file_name: "a.txt", extracted_content: "x".repeat(400) }] },
          { sender: "assistant", content: [{ type: "tool_use", name: "mcp__drive__search", input: { q: "a" } }, { type: "tool_use", name: "web_search", input: {} }, { type: "text", text: "done" }] },
        ],
      },
      memory: { memory: "Eoj likes short answers." },
      project: { name: "Allternit", prompt_template: "Be direct." },
      projectDocs: [{ file_name: "brief.md", content: "y".repeat(800) }],
      skills: [{ name: "pdf", description: "read pdfs" }, { name: "off", enabled: false }],
      usage: { five_hour: { utilization: 40, resets_at: "2026-10-20T00:00:00Z" }, seven_day: { utilization: 1, resets_at: null } },
    })!;
    expect(r.model).toBe("claude-opus-5-5");
    expect(r.rows.messages).toBeGreaterThan(tokensOf("x".repeat(400)));
    expect(r.memory_files.map((f) => f.path)).toEqual(["Claude memory", "Project instructions · Allternit", "Project file · brief.md"]);
    expect(r.rows.memory_files).toBe(r.memory_files.reduce((s, f) => s + f.tokens, 0));
    expect(r.skills.map((s) => s.name)).toEqual(["pdf"]);
    expect(r.mcp_tools).toEqual([{ name: "mcp__drive__search", server: "drive", tokens: 0 }]);
    expect(r.system_tools.map((t) => t.name).sort()).toEqual(["web search", "web_search"]);
    expect(r.limits?.map((l) => [l.label, l.used_ratio])).toEqual([["5-hour", 0.4], ["Weekly", 1]]);
    expect(r.hidden).toContain("system_prompt");
  });

  it("is null without a conversation", () => {
    expect(claudeContextReport({ conversation: null })).toBeNull();
  });
});

describe("chatgptContextReport", () => {
  it("walks the current branch: hidden memory/instructions to memory files, tool turns to tools", () => {
    const mapping = {
      root: { id: "root", parent: null, message: null },
      sys: { id: "sys", parent: "root", message: { author: { role: "system" }, content: { content_type: "text", parts: [""] }, metadata: { is_visually_hidden_from_conversation: true } } },
      ins: { id: "ins", parent: "sys", message: { author: { role: "user" }, content: { content_type: "user_editable_context", user_profile: "I am Eoj", user_instructions: "Be brief" } } },
      mem: { id: "mem", parent: "ins", message: { author: { role: "system" }, content: { content_type: "model_editable_context", model_set_context: "Eoj runs Allternit." } } },
      u1: { id: "u1", parent: "mem", message: { author: { role: "user" }, content: { content_type: "text", parts: ["search the news"] } } },
      t1: { id: "t1", parent: "u1", message: { author: { role: "tool", name: "web.run" }, content: { content_type: "text", parts: ["z".repeat(400)] } } },
      a1: { id: "a1", parent: "t1", message: { author: { role: "assistant" }, content: { content_type: "text", parts: ["here"] }, metadata: { model_slug: "gpt-6" } } },
      other: { id: "other", parent: "u1", message: { author: { role: "assistant" }, content: { content_type: "text", parts: ["w".repeat(9000)] } } },
    };
    const r = chatgptContextReport({ conversation: { current_node: "a1", mapping }, memories: { memories: [{ content: "not used: inline wins" }] } })!;
    expect(r.model).toBe("gpt-6");
    expect(r.memory_files.map((f) => f.path)).toEqual(["Custom instructions", "Saved memories"]);
    expect(r.system_tools).toEqual([{ name: "web.run", tokens: 0 }]);
    // the other branch is not in context
    expect(r.rows.messages).toBeLessThan(200);
  });

  it("falls back to the memories and instructions endpoints", () => {
    const mapping = { a: { id: "a", parent: null, message: { author: { role: "user" }, content: { content_type: "text", parts: ["hi"] } } } };
    const r = chatgptContextReport({
      conversation: { current_node: "a", mapping },
      memories: { memories: [{ content: "likes tea" }] },
      instructions: { enabled: true, about_user_message: "Eoj", about_model_message: "brief" },
    })!;
    expect(r.memory_files.map((f) => f.path)).toEqual(["Saved memories", "Custom instructions"]);
  });
});
