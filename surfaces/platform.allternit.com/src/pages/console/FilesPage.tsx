import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  TrashIcon,
  AlertCircleIcon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { api, formatApiError } from "@/lib/api-client";
import {
  ListPage,
  EmptyState,
  MonoChip,
  Badge,
  SkeletonRow,
  CodeTemplateBlock,
  DESTRUCTIVE_BUTTON_CLASS,
} from "@/components/console-ui";
import {
  ensureConsoleGatewayKey,
  gatewayAuthOptions,
  type OpenAiFileObject,
  type OpenAiListResponse,
} from "@/lib/console-gateway";

const PURPOSES = [
  "assistants",
  "batch",
  "fine-tune",
  "embeddings",
  "vision",
  "user_data",
];

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatDate(unixSeconds: number): string {
  if (!unixSeconds) return "—";
  return new Date(unixSeconds * 1000).toLocaleString();
}

function arrayBufferToBase64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return window.btoa(binary);
}

function gatewayBase(): string {
  return String(
    import.meta.env.VITE_ALLTERNIT_GATEWAY_URL || "https://api.allternit.com"
  ).replace(/\/+$/, "");
}

const UPLOAD_CURL = `curl ${gatewayBase()}/v1/files \\
  -H "Authorization: Bearer $ALLTERNIT_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{
    "filename": "report.pdf",
    "purpose": "assistants",
    "file": "'"$(base64 -i report.pdf)"'"
  }'

# Note: this gateway accepts a JSON body with base64-encoded content,
# not multipart/form-data.`;

export function FilesPage(): React.ReactNode {
  const [files, setFiles] = useState<OpenAiFileObject[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [showUpload, setShowUpload] = useState(false);
  const [purpose, setPurpose] = useState("assistants");
  const [uploading, setUploading] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const key = await ensureConsoleGatewayKey();
      const data = await api.get<OpenAiListResponse<OpenAiFileObject>>(
        "/v1/files",
        gatewayAuthOptions(key)
      );
      setFiles(data.data ?? []);
    } catch (err) {
      setError(formatApiError(err, "Unable to load files"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return files;
    return files.filter(
      (f) =>
        f.filename.toLowerCase().includes(q) ||
        f.id.toLowerCase().includes(q) ||
        f.purpose.toLowerCase().includes(q)
    );
  }, [files, search]);

  const handleUpload = useCallback(
    async (file: File) => {
      setUploading(true);
      setError(null);
      try {
        const buffer = await file.arrayBuffer();
        const key = await ensureConsoleGatewayKey();
        await api.post(
          "/v1/files",
          {
            file: arrayBufferToBase64(buffer),
            filename: file.name,
            purpose,
          },
          gatewayAuthOptions(key)
        );
        setShowUpload(false);
        await load();
      } catch (err) {
        setError(formatApiError(err, "Upload failed"));
      } finally {
        setUploading(false);
        if (fileInputRef.current) fileInputRef.current.value = "";
      }
    },
    [purpose, load]
  );

  const handleDelete = useCallback(
    async (file: OpenAiFileObject) => {
      if (!window.confirm(`Delete ${file.filename} (${file.id})?`)) return;
      setBusyId(file.id);
      setError(null);
      try {
        const key = await ensureConsoleGatewayKey();
        await api.delete(`/v1/files/${file.id}`, gatewayAuthOptions(key));
        await load();
      } catch (err) {
        setError(formatApiError(err, "Delete failed"));
      } finally {
        setBusyId(null);
      }
    },
    [load]
  );

  return (
    <ListPage
      title="Files"
      subtitle="Upload and manage files for the Responses and Batches APIs — scoped to your organization."
      searchPlaceholder="Search by filename, id, or purpose…"
      onSearch={setSearch}
      primaryAction={{
        label: "Upload file",
        onClick: () => setShowUpload((v) => !v),
      }}
    >
      {error && (
        <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}

      {showUpload && (
        <div className="mb-4 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <div className="flex flex-wrap items-end gap-3">
            <div className="min-w-[260px] flex-1 space-y-1">
              <label
                htmlFor="file-upload"
                className="text-[12px] font-semibold text-[var(--text-secondary)]"
              >
                File
              </label>
              <input
                id="file-upload"
                ref={fileInputRef}
                type="file"
                onChange={(e) => {
                  const file = e.target.files?.[0];
                  if (file) void handleUpload(file);
                }}
                disabled={uploading}
                className="block w-full text-[13px] text-[var(--text-secondary)] file:mr-3 file:rounded-lg file:border-0 file:bg-[var(--accent-primary)] file:px-3 file:py-1.5 file:text-[12px] file:font-semibold file:text-[var(--ui-text-inverse)]"
              />
            </div>
            <div className="space-y-1">
              <label
                htmlFor="file-purpose"
                className="text-[12px] font-semibold text-[var(--text-secondary)]"
              >
                Purpose
              </label>
              <select
                id="file-purpose"
                value={purpose}
                onChange={(e) => setPurpose(e.target.value)}
                className="block rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-2 px-3 text-[13px] text-[var(--text-primary)]"
              >
                {PURPOSES.map((p) => (
                  <option key={p} value={p}>
                    {p}
                  </option>
                ))}
              </select>
            </div>
          </div>
          <p className="mt-3 mb-0 text-[12px] text-[var(--text-tertiary)]">
            The file is base64-encoded in the browser and sent as JSON — this
            gateway's <code className="font-mono">POST /v1/files</code> accepts
            a JSON body rather than multipart.
          </p>
        </div>
      )}

      {loading ? (
        <SkeletonRow lines={5} />
      ) : filtered.length === 0 ? (
        <EmptyState
          art="search"
          title={search ? "No files match your search" : "No files yet"}
          caption={
            search
              ? "Try a different filename, id, or purpose."
              : "Upload a file to use it with the Responses and Batches APIs, or create one from the command line."
          }
          ctaLabel={search ? undefined : "Upload file"}
          onCtaClick={search ? undefined : () => setShowUpload(true)}
        />
      ) : (
        <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
          <table className="w-full border-collapse text-left text-[13px]">
            <thead>
              <tr className="border-b border-solid border-[var(--border-subtle)] text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                <th className="px-3 py-2 font-medium">ID</th>
                <th className="px-3 py-2 font-medium">Filename</th>
                <th className="px-3 py-2 font-medium">Purpose</th>
                <th className="px-3 py-2 font-medium">Bytes</th>
                <th className="px-3 py-2 font-medium">Created</th>
                <th className="px-3 py-2 text-right font-medium">Actions</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((file) => (
                <tr
                  key={file.id}
                  className="border-b border-solid border-[var(--border-subtle)] last:border-b-0 hover:bg-[var(--surface-hover)]"
                >
                  <td className="px-3 py-2.5">
                    <MonoChip>{file.id}</MonoChip>
                  </td>
                  <td className="px-3 py-2.5 font-medium text-[var(--text-primary)]">
                    {file.filename}
                  </td>
                  <td className="px-3 py-2.5">
                    <Badge>{file.purpose}</Badge>
                  </td>
                  <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                    {formatBytes(file.bytes)}
                  </td>
                  <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                    {formatDate(file.created_at)}
                  </td>
                  <td className="px-3 py-2.5 text-right">
                    <button
                      type="button"
                      onClick={() => void handleDelete(file)}
                      disabled={busyId === file.id}
                      className={cn(DESTRUCTIVE_BUTTON_CLASS, "px-2.5 py-1 text-[12px]")}
                    >
                      <HugeiconsIcon icon={TrashIcon} size={12} />
                      {busyId === file.id ? "Deleting…" : "Delete"}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {!loading && files.length === 0 && !search && (
        <div className="mx-auto mt-6 max-w-xl">
          <CodeTemplateBlock language="bash" code={UPLOAD_CURL} />
        </div>
      )}
    </ListPage>
  );
}

export default FilesPage;
