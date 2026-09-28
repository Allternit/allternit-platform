// Image-chat history policy (Eoj, 2026-09-28): image tasks run in a provider
// project and reuse one chat per account until it holds `max` images, then a
// new chat opens in the same project.
import { createHash } from "node:crypto";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Task } from "@allternit/subscription-fabric-contracts";
import { loadConfig } from "../src/config.js";
import { EventLog } from "../src/events/log.js";
import { openDatabase, type Db } from "../src/store/db.js";
import { getActiveImageChat, insertTask, recordImageChatUse } from "../src/store/queries.js";
import { runAttempt, type WorkerDeps } from "../src/worker/worker.js";
import {
  cleanupDir,
  dummyResolver,
  fakeLease,
  sampleTask,
  scriptedAdapter,
  tmpStateDir,
} from "./helpers.js";

const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4]);

let dir: string;
let db: Db;
let log: EventLog;

beforeEach(() => {
  dir = tmpStateDir();
  db = openDatabase(":memory:");
  log = new EventLog(db);
});
afterEach(() => {
  db.close();
  cleanupDir(dir);
});

const use = (thread: string, images = 1) => ({
  provider: "chatgpt",
  account_id: "acct-1",
  provider_thread_id: thread,
  provider_url: `https://chatgpt.com/g/g-p-1-allternit/c/${thread}`,
  project: "Allternit",
  images,
});

describe("image_chats store", () => {
  it("advances the active chat and marks it full at max", () => {
    expect(getActiveImageChat(db, "chatgpt", "acct-1")).toBeNull();
    expect(recordImageChatUse(db, use("c1"), 3).image_count).toBe(1);
    expect(recordImageChatUse(db, use("c1"), 3).status).toBe("active");
    const full = recordImageChatUse(db, use("c1"), 3);
    expect(full.image_count).toBe(3);
    expect(full.status).toBe("full");
    expect(getActiveImageChat(db, "chatgpt", "acct-1")).toBeNull();
  });

  it("a different chat retires the active one and becomes active", () => {
    recordImageChatUse(db, use("c1"), 5);
    recordImageChatUse(db, use("c2"), 5);
    const active = getActiveImageChat(db, "chatgpt", "acct-1");
    expect(active?.provider_thread_id).toBe("c2");
    expect(active?.image_count).toBe(1);
  });

  it("is per account", () => {
    recordImageChatUse(db, use("c1"), 5);
    expect(getActiveImageChat(db, "chatgpt", "acct-2")).toBeNull();
  });
});

describe("image_chats config", () => {
  it("defaults to project Allternit, 20 images", () => {
    expect(loadConfig({ SUBS_GATEWAY_STATE_DIR: dir }).imageChats).toEqual({ project: "Allternit", max: 20 });
  });
  it("empty project disables it; max is validated", () => {
    expect(
      loadConfig({ SUBS_GATEWAY_STATE_DIR: dir, SUBS_GATEWAY_IMAGE_PROJECT: "", SUBS_GATEWAY_IMAGE_CHAT_MAX: "5" })
        .imageChats
    ).toEqual({ project: null, max: 5 });
    expect(() => loadConfig({ SUBS_GATEWAY_STATE_DIR: dir, SUBS_GATEWAY_IMAGE_CHAT_MAX: "0" })).toThrow(/positive integer/);
  });
});

describe("worker applies the image-chat policy", () => {
  const deps = (): WorkerDeps => ({
    db,
    log,
    artifactsDir: join(dir, "artifacts"),
    imageChats: { project: "Allternit", max: 2 },
  });

  // One image per run, in provider thread `thread`; records the options the
  // adapter was handed.
  async function runImage(thread: string): Promise<Task["options"]> {
    let seen: Task["options"] = {};
    const adapter = scriptedAdapter({
      execute: async function* (task, ctx) {
        seen = task.options;
        await ctx.markSubmitted(null);
        await ctx.markSubmitted(thread);
        yield { t: "submitted", provider_thread_id: thread, provider_url: `https://chatgpt.com/g/g-p-1-allternit/c/${thread}` };
        const bytes = new Uint8Array([...PNG, thread.length]);
        const id = await ctx.artifacts.begin(
          { provider: "fixture-web", provider_artifact_id: `img-${thread}`, provider_url: "https://chatgpt.com/", provider_url_expires_at: null } as never,
          { mime_type: "image/png", format: "png", title: "img" }
        );
        await ctx.artifacts.write(id, bytes);
        await ctx.artifacts.commit(id, {
          data: bytes,
          mime_type: "image/png",
          format: "png",
          sha256: createHash("sha256").update(bytes).digest("hex"),
          size_bytes: bytes.length,
        });
        yield { t: "done", outcome: "success" };
      },
    });
    const task = sampleTask({ task_id: `task-${Math.random().toString(36).slice(2)}`, capability: "image.generate" });
    insertTask(db, task);
    const outcome = await runAttempt(deps(), {
      taskId: task.task_id,
      adapter,
      accountId: "acct-1",
      page: fakeLease(),
      makeResolver: () => dummyResolver(),
    });
    expect(outcome).toEqual({ kind: "terminal", status: "completed" });
    return seen;
  }

  it("first image opens a chat in the project; the next reuses it; at max a new chat opens", async () => {
    const first = await runImage("chat-a");
    expect(first.image_project).toBe("Allternit");
    expect(first.image_chat_url).toBeUndefined();
    expect(getActiveImageChat(db, "fixture-web", "acct-1")?.image_count).toBe(1);

    const second = await runImage("chat-a");
    expect(second.image_chat_url).toBe("https://chatgpt.com/g/g-p-1-allternit/c/chat-a");
    // max 2 reached → full → no active chat
    expect(getActiveImageChat(db, "fixture-web", "acct-1")).toBeNull();

    const third = await runImage("chat-b");
    expect(third.image_project).toBe("Allternit");
    expect(third.image_chat_url).toBeUndefined();
    expect(getActiveImageChat(db, "fixture-web", "acct-1")?.provider_thread_id).toBe("chat-b");
  });
});
