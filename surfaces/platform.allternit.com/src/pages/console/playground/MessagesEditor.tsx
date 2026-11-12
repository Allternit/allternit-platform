import React, { useState } from "react";
import type { ChatToolCall } from "@/lib/console-gateway";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowUp01Icon,
  ArrowDown01Icon,
  Delete01Icon,
  PlusSignIcon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { SETTINGS_SELECT_CLASS } from "@/components/console-ui";

export interface EditableMessage {
  id: string;
  role: "user" | "assistant" | "tool";
  content: string;
  /** Image data URLs attached to a user message (vision models only). */
  images?: string[];
  /** Assistant message that asked for tools. */
  toolCalls?: ChatToolCall[];
  /** Tool message: the call this result answers. */
  toolCallId?: string;
}

interface MessagesEditorProps {
  messages: EditableMessage[];
  onChange: (messages: EditableMessage[]) => void;
  /** Image upload is offered only when the selected model supports vision. */
  supportsVision?: boolean;
}

const MAX_IMAGE_BYTES = 4 * 1024 * 1024;

function readDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error ?? new Error("Could not read the image."));
    reader.readAsDataURL(file);
  });
}

const TEXTAREA_CLASS =
  "w-full resize-y rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 font-mono text-[12px] leading-relaxed text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";

const ICON_BUTTON_CLASS =
  "inline-flex size-7 items-center justify-center rounded-md border border-solid border-[var(--border-subtle)] text-[var(--text-secondary)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)] disabled:opacity-40 disabled:cursor-not-allowed";

/**
 * Ordered chat message list with role editing, reorder, and add/remove —
 * the console's "Build your request" messages block.
 */
export function MessagesEditor({
  messages,
  onChange,
  supportsVision = false,
}: MessagesEditorProps): React.ReactNode {
  const [imageError, setImageError] = useState<string | null>(null);

  const attach = async (id: string, files: FileList | null) => {
    if (!files || files.length === 0) return;
    setImageError(null);
    const urls: string[] = [];
    for (const file of Array.from(files)) {
      if (!file.type.startsWith("image/")) {
        setImageError("Only image files can be attached.");
        continue;
      }
      if (file.size > MAX_IMAGE_BYTES) {
        setImageError("Images must be 4 MB or smaller.");
        continue;
      }
      try {
        urls.push(await readDataUrl(file));
      } catch (err) {
        setImageError(err instanceof Error ? err.message : "Could not read the image.");
      }
    }
    if (urls.length > 0) {
      const target = messages.find((m) => m.id === id);
      update(id, { images: [...(target?.images ?? []), ...urls] });
    }
  };

  const update = (id: string, patch: Partial<EditableMessage>) =>
    onChange(
      messages.map((m) => (m.id === id ? { ...m, ...patch } : m))
    );

  const move = (index: number, delta: number) => {
    const target = index + delta;
    if (target < 0 || target >= messages.length) return;
    const next = messages.slice();
    const [item] = next.splice(index, 1);
    next.splice(target, 0, item);
    onChange(next);
  };

  const remove = (id: string) => onChange(messages.filter((m) => m.id !== id));

  const add = (role: "user" | "assistant") =>
    onChange([
      ...messages,
      {
        id: `msg-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
        role,
        content: "",
      },
    ]);

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <span className="text-[12px] font-semibold text-[var(--text-secondary)]">
          Messages
        </span>
        <div className="flex gap-1.5">
          <button type="button" onClick={() => add("user")} className={ICON_BUTTON_CLASS} title="Add user message">
            <HugeiconsIcon icon={PlusSignIcon} size={13} />
          </button>
          <button
            type="button"
            onClick={() => add("assistant")}
            className={cn(ICON_BUTTON_CLASS, "w-auto px-2 text-[11px] font-semibold")}
            title="Add assistant message"
          >
            <HugeiconsIcon icon={PlusSignIcon} size={13} /> assistant
          </button>
        </div>
      </div>

      {messages.length === 0 && (
        <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
          No messages yet — add a user message to get started.
        </p>
      )}

      {messages.map((message, index) => (
        <div
          key={message.id}
          className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-3"
        >
          <div className="mb-2 flex items-center justify-between gap-2">
            {message.role === "tool" ? (
              <span className="font-mono text-[12px] text-[var(--text-secondary)]">
                tool result{message.toolCallId ? ` · ${message.toolCallId}` : ""}
              </span>
            ) : (
              <select
                value={message.role}
                onChange={(e) =>
                  update(message.id, {
                    role: e.target.value as "user" | "assistant",
                  })
                }
                className={SETTINGS_SELECT_CLASS}
                aria-label={`Message ${index + 1} role`}
              >
                <option value="user">user</option>
                <option value="assistant">assistant</option>
              </select>
            )}
            <div className="flex gap-1">
              <button
                type="button"
                onClick={() => move(index, -1)}
                disabled={index === 0}
                className={ICON_BUTTON_CLASS}
                aria-label="Move message up"
              >
                <HugeiconsIcon icon={ArrowUp01Icon} size={13} />
              </button>
              <button
                type="button"
                onClick={() => move(index, 1)}
                disabled={index === messages.length - 1}
                className={ICON_BUTTON_CLASS}
                aria-label="Move message down"
              >
                <HugeiconsIcon icon={ArrowDown01Icon} size={13} />
              </button>
              <button
                type="button"
                onClick={() => remove(message.id)}
                className={ICON_BUTTON_CLASS}
                aria-label="Remove message"
              >
                <HugeiconsIcon icon={Delete01Icon} size={13} />
              </button>
            </div>
          </div>
          <textarea
            value={message.content}
            onChange={(e) => update(message.id, { content: e.target.value })}
            placeholder={`${message.role} message content…`}
            rows={Math.min(10, Math.max(3, message.content.split("\n").length))}
            className={TEXTAREA_CLASS}
          />
          {message.toolCalls && message.toolCalls.length > 0 && (
            <pre className="m-0 mt-2 overflow-x-auto rounded-lg bg-[var(--bg-primary)] p-2 font-mono text-[11px] text-[var(--text-secondary)]">
              {message.toolCalls
                .map((t) => `${t.function.name}(${t.function.arguments})`)
                .join("\n")}
            </pre>
          )}
          {message.role === "user" && (
            <div className="mt-2 space-y-2">
              {(message.images ?? []).length > 0 && (
                <div className="flex flex-wrap gap-2">
                  {message.images!.map((url, i) => (
                    <div key={i} className="relative">
                      <img
                        src={url}
                        alt={`Attached image ${i + 1}`}
                        className="size-16 rounded-lg border border-solid border-[var(--border-subtle)] object-cover"
                      />
                      <button
                        type="button"
                        aria-label="Remove image"
                        onClick={() =>
                          update(message.id, {
                            images: message.images!.filter((_, j) => j !== i),
                          })
                        }
                        className="absolute -right-1.5 -top-1.5 inline-flex size-5 items-center justify-center rounded-full border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] text-[11px] text-[var(--text-secondary)]"
                      >
                        ×
                      </button>
                    </div>
                  ))}
                </div>
              )}
              {supportsVision ? (
                <label className="inline-flex cursor-pointer items-center gap-1 text-[12px] font-semibold text-[var(--accent-primary)]">
                  Attach image
                  <input
                    type="file"
                    accept="image/*"
                    multiple
                    className="sr-only"
                    onChange={(e) => {
                      void attach(message.id, e.target.files);
                      e.target.value = "";
                    }}
                  />
                </label>
              ) : (
                (message.images ?? []).length > 0 && (
                  <p className="m-0 text-[11px] text-[var(--status-error)]">
                    The selected model does not accept images. Pick a model with vision or remove them.
                  </p>
                )
              )}
            </div>
          )}
        </div>
      ))}
      {imageError && (
        <p className="m-0 text-[12px] text-[var(--status-error)]">{imageError}</p>
      )}
    </div>
  );
}
