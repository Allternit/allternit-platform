/**
 * Capability catalog for the local gizzi-code harness.
 *
 * This module declares what capabilities the running node exposes to the
 * Allternit Fabric. Over time it should be populated by introspecting loaded
 * plugins, registered tools, available hardware, and installed applications.
 * For the convergence milestone we publish the built-in harness capabilities
 * (session, shell, file, pty) plus the network transport capabilities the
 * node is configured to use.
 */
import os from "node:os"
import path from "node:path"
import {
  type FabricCapability,
  type NodeIdentity,
  type NodeResource,
  fabricCapabilityKindSchema,
} from "./transport"
import { Installation } from "@/shared/installation"

/** Path to the canonical AllternitOS v0.2 worker manifest for this runtime. */
export const WORKER_MANIFEST_PATH = path.resolve(import.meta.dir, "../../../runtime.fabric.worker.json")

/** Canonical worker metadata for this harness (mirrors runtime.fabric.worker.json). */
export const WORKER_MANIFEST = {
  worker_id: "wrk_gizzi_session_harness_v1",
  name: "gizzi.session.harness",
  version: "0.2.0",
  runtime_class: "native_service" as const,
  state: "active" as const,
  functions: [
    "session.list",
    "session.message",
    "session.abort",
    "session.events",
    "node.capabilities",
    "shell.exec",
    "file.read",
    "file.write",
    "file.list",
    "file.search",
    "browser.navigate",
  ],
}

/** Minimal canonical AllternitOS NodeCapabilityRecord shape. */
export interface NodeCapabilityRecord {
  schema_version: "1.1.0"
  node_id: string
  recorded_at: string
  hardware: {
    cpu: {
      vendor: string
      model: string
      cores: number
      threads: number
    }
    memory: {
      total_bytes: number
      type: string
    }
  }
  software: {
    fabric_os_version: string
    kernel_version: string
  }
  fabric: {
    wireguard_public_key: string
  }
  workers?: {
    workers?: HostedWorker[]
    functions?: HostedFunction[]
    capabilities?: HostedCapability[]
  }
}

export interface HostedWorker {
  worker_id: string
  name: string
  version: string
  runtime_class: string
  state: "active" | "degraded" | "unavailable"
  functions?: string[]
}

export interface HostedFunction {
  function_id: string
  worker_id: string
  name: string
  capability: string
}

export interface HostedCapability {
  capability: string
  worker_ids?: string[]
  resource?: string
}

const kind = fabricCapabilityKindSchema.enum

/**
 * Built-in capabilities every harness provides. These are worker functions the
 * agent or a client can invoke without assuming remote control of a desktop.
 */
export function builtInCapabilities(): FabricCapability[] {
  return [
    {
      id: "cap:harness:session",
      name: "harness.session",
      version: "0.1.0",
      kind: kind.execute,
      resource: "session",
      description: "Create, resume, and list durable agent sessions.",
    },
    {
      id: "cap:harness:session-message",
      name: "harness.session.message",
      version: "0.1.0",
      kind: kind.write,
      resource: "session",
      description: "Send a user message into a session and start the agent loop.",
    },
    {
      id: "cap:harness:session-events",
      name: "harness.session.events",
      version: "0.1.0",
      kind: kind.stream,
      resource: "session",
      description: "Stream session events (messages, status, approvals, questions).",
    },
    {
      id: "cap:harness:session-abort",
      name: "harness.session.abort",
      version: "0.1.0",
      kind: kind.execute,
      resource: "session",
      description: "Cancel the running agent loop for a session.",
    },
    {
      id: "cap:harness:shell",
      name: "harness.shell",
      version: "0.1.0",
      kind: kind.execute,
      resource: "shell",
      description: "Execute shell commands and return structured output.",
    },
    {
      id: "cap:harness:shell-exec",
      name: "harness.shell.exec",
      version: "0.1.0",
      kind: kind.execute,
      resource: "shell",
      description: "Execute a one-shot shell command and return stdout and exit code.",
    },
    {
      id: "cap:harness:shell-stream",
      name: "harness.shell.stream",
      version: "0.1.0",
      kind: kind.stream,
      resource: "shell",
      description: "Open a streaming pseudo-terminal session.",
    },
    {
      id: "cap:harness:file",
      name: "harness.file",
      version: "0.1.0",
      kind: kind.execute,
      resource: "file",
      description: "Read, write, list, and watch files in project directories.",
    },
    {
      id: "cap:harness:file-read",
      name: "harness.file.read",
      version: "0.1.0",
      kind: kind.read,
      resource: "file",
      description: "Read a file within the project directory.",
    },
    {
      id: "cap:harness:file-write",
      name: "harness.file.write",
      version: "0.1.0",
      kind: kind.write,
      resource: "file",
      description: "Write a file within the project directory.",
    },
    {
      id: "cap:harness:file-list",
      name: "harness.file.list",
      version: "0.1.0",
      kind: kind.read,
      resource: "file",
      description: "List files and directories within the project directory.",
    },
    {
      id: "cap:harness:file-search",
      name: "harness.file.search",
      version: "0.1.0",
      kind: kind.read,
      resource: "file",
      description: "Search for files by name within the project directory.",
    },
    {
      id: "cap:harness:browser-navigate",
      name: "harness.browser.navigate",
      version: "0.1.0",
      kind: kind.observe,
      resource: "browser",
      description: "Navigate to a URL and return structured page metadata.",
    },
    {
      id: "cap:image:auto",
      name: "image.auto",
      version: "0.1.0",
      kind: kind.execute,
      resource: "image",
      description: "Make a picture from the person's own Image-mode send (metadata.image_prompt); the runtime picks the engine.",
    },
    {
      id: "cap:network:tailscale",
      name: "network.tailscale",
      version: "0.1.0",
      kind: kind.observe,
      resource: "network",
      description: "Tailscale/Headscale mesh membership for direct peer reachability.",
    },
  ]
}

/** Static resources for this node. Eventually this comes from hardware probes. */
export function nodeResources(): NodeResource[] {
  const totalMemory = os.totalmem()
  const cpus = os.cpus()
  const resources: NodeResource[] = [
    { kind: "compute.cpu", name: "cores", value: cpus.length, unit: "cores" },
    { kind: "compute.memory", name: "total", value: Math.round(totalMemory / 1024 / 1024 / 1024), unit: "GiB" },
    { kind: "platform.os", name: "platform", value: `${process.platform}-${process.arch}` },
  ]
  if (process.env.GIZZI_GPU) {
    resources.push({ kind: "compute.gpu", name: "gpu", value: process.env.GIZZI_GPU })
  }
  return resources
}

/** Build a NodeIdentity for the local harness. */
export function buildNodeIdentity(opts: { nodeId?: string; name?: string; endpoints?: NodeIdentity["endpoints"] }): NodeIdentity {
  const host = os.hostname() || "allternit-host"
  return {
    nodeId: normalizeNodeId(opts.nodeId) ?? `node_${sanitizeNodeId(host)}`,
    name: opts.name ?? host,
    runtimeType: "desktop",
    platform: `${process.platform}-${process.arch}`,
    version: Installation.VERSION,
    endpoints: opts.endpoints ?? [],
    capabilities: builtInCapabilities(),
    resources: nodeResources(),
  }
}

function sanitizeNodeId(input: string): string {
  return input.toLowerCase().replace(/[^a-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "")
}

function normalizeNodeId(id?: string): string | undefined {
  if (!id) return undefined
  if (id.startsWith("node_")) return id
  return `node_${sanitizeNodeId(id)}`
}

/** Map a donor capability to a canonical hosted capability record. */
function toHostedCapability(cap: FabricCapability): HostedCapability {
  return {
    capability: cap.name,
    resource: cap.resource,
  }
}

/** Build a canonical NodeCapabilityRecord from the local NodeIdentity. */
export function buildNodeCapabilityRecord(identity: NodeIdentity): NodeCapabilityRecord {
  const cpus = os.cpus()
  const cpu = cpus[0]
  // Node's os.cpus() does not expose vendor on all platforms; treat it as best-effort.
  const vendor = (cpu as unknown as { vendor?: string })?.vendor ?? "unknown"
  const model = cpu?.model ?? "unknown"
  const cores = cpus.length
  // os.cpus() returns one entry per hardware thread on most platforms.
  const threads = cpus.length
  const totalBytes = os.totalmem()

  return {
    schema_version: "1.1.0",
    node_id: identity.nodeId,
    recorded_at: new Date().toISOString(),
    hardware: {
      cpu: { vendor, model, cores, threads },
      memory: { total_bytes: totalBytes, type: "unknown" },
    },
    software: {
      fabric_os_version: identity.version,
      kernel_version: os.release(),
    },
    fabric: {
      wireguard_public_key: "",
    },
    workers: {
      workers: [WORKER_MANIFEST],
      functions: identity.capabilities.map((cap) => ({
        function_id: `fun_${WORKER_MANIFEST.worker_id}_${cap.name.replace(/\./g, "_")}`,
        worker_id: WORKER_MANIFEST.worker_id,
        name: cap.name,
        capability: cap.name,
      })),
      capabilities: identity.capabilities.map(toHostedCapability),
    },
  }
}

/** Build a canonical NodeDirectory entry from the local NodeIdentity. */
export function buildNodeDirectoryEntry(
  identity: NodeIdentity,
  opts: { reachable?: boolean; health?: "healthy" | "degraded" | "unavailable"; transportKind?: string; fabricAddress?: string } = {},
): unknown {
  return {
    schema_version: "1.0.0",
    node_id: identity.nodeId,
    capability_record: buildNodeCapabilityRecord(identity),
    workers: identity.capabilities.map(toHostedCapability),
    last_seen: new Date().toISOString(),
    reachable: opts.reachable ?? true,
    health: opts.health ?? "healthy",
    transport_kind: opts.transportKind ?? "tailscale",
    fabric_address: opts.fabricAddress ?? identity.endpoints.find((e) => e.transport === "tailscale")?.url,
  }
}

/** Build a canonical NodeDirectory containing only the local node. */
export function buildNodeDirectory(identity: NodeIdentity): unknown {
  return {
    schema_version: "1.0.0",
    recorded_at: new Date().toISOString(),
    entries: [buildNodeDirectoryEntry(identity)],
  }
}
