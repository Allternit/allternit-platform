import React from "react";
import { Link } from "react-router-dom";
import {
  BookOpen,
  ArrowSquareOut,
  Code,
  Cloud,
  Gear,
  Key,
  Terminal,
  Lightning,
} from "@phosphor-icons/react";

const QUICK_START_STEPS = [
  {
    title: "Create an API key",
    body: "Go to API Keys and create a scoped key. Copy the token immediately — it is shown only once.",
    to: "/api-keys",
  },
  {
    title: "Pick a model",
    body: "Browse the model catalog to see which models and upstream providers are available to your workspace.",
    to: "/models",
  },
  {
    title: "Send a request",
    body: "Use the OpenAI-compatible chat completions endpoint with your Allternit API key.",
  },
];

const ENDPOINTS = [
  {
    method: "GET",
    path: "/v1/models",
    description: "List available models, pricing, and context lengths.",
  },
  {
    method: "POST",
    path: "/v1/chat/completions",
    description: "OpenAI-compatible chat completions with streaming support.",
  },
  {
    method: "GET",
    path: "/api/v1/billing/credits",
    description: "Check prepaid credit balance and recent transactions.",
  },
  {
    method: "GET",
    path: "/api/v1/runtime-devices",
    description: "List paired data-plane nodes (desktop, VPS, provisioned).",
  },
  {
    method: "GET",
    path: "/api/v1/agent-sessions",
    description: "List agent sessions (relayed to your paired node; 428 if none).",
  },
  {
    method: "GET",
    path: "/api/v1/provisioned-instances",
    description: "List Allternit-provisioned data-plane instances.",
  },
];

export function DocsPage() {
  return (
    <div className="space-y-8">
      <div>
        <h1 className="text-[22px] font-semibold tracking-tight text-[var(--text-primary)]">
          Documentation
        </h1>
        <p className="text-[13px] text-[var(--text-secondary)] mt-1">
          Quick reference for the Allternit Cloud API. The only public origin is{" "}
          <code className="font-mono text-[12px]">https://api.allternit.com</code>.
          Allternit API on port 8013 is the private data plane — browsers never call it
          directly.
        </p>
      </div>

      {/* Quick start */}
      <section className="space-y-4">
        <div className="flex items-center gap-2 text-[14px] font-semibold text-[var(--text-primary)]">
          <Lightning size={18} className="text-[var(--accent-primary)]" /> Quick start
        </div>
        <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
          {QUICK_START_STEPS.map((step, idx) => (
            <div
              key={step.title}
              className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4"
            >
              <div className="flex items-center gap-2 text-[13px] font-semibold text-[var(--text-primary)]">
                <span className="flex size-5 items-center justify-center rounded-full bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] text-[11px]">
                  {idx + 1}
                </span>
                {step.title}
              </div>
              <p className="mt-2 text-[12px] leading-relaxed text-[var(--text-secondary)]">
                {step.body}
              </p>
              {step.to && (
                <Link
                  to={step.to}
                  className="mt-3 inline-flex items-center gap-1 text-[12px] font-medium text-[var(--accent-primary)] hover:underline"
                >
                  Open <ArrowSquareOut size={12} />
                </Link>
              )}
            </div>
          ))}
        </div>
      </section>

      {/* Example */}
      <section className="space-y-4">
        <div className="flex items-center gap-2 text-[14px] font-semibold text-[var(--text-primary)]">
          <Terminal size={18} className="text-[var(--accent-primary)]" /> Example request
        </div>
        <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-4 overflow-x-auto">
          <pre className="text-[12px] font-mono leading-relaxed text-[var(--text-secondary)]">
            <code>{`curl https://api.allternit.com/v1/chat/completions \\
  -H "Authorization: Bearer \$ALLTERNIT_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "deepseek-v4.1-flash",
    "messages": [{"role": "user", "content": "Hello, Allternit"}],
    "stream": false
  }'`}</code>
          </pre>
        </div>
      </section>

      {/* Endpoints */}
      <section className="space-y-4">
        <div className="flex items-center gap-2 text-[14px] font-semibold text-[var(--text-primary)]">
          <Code size={18} className="text-[var(--accent-primary)]" /> Common endpoints
        </div>
        <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 overflow-hidden">
          <table className="w-full text-left text-[13px]">
            <thead className="bg-[var(--bg-primary)] text-[11px] uppercase tracking-wider text-[var(--text-tertiary)]">
              <tr>
                <th className="px-4 py-3 font-medium">Method</th>
                <th className="px-4 py-3 font-medium">Path</th>
                <th className="px-4 py-3 font-medium">Description</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-[var(--border-subtle)]">
              {ENDPOINTS.map((ep) => (
                <tr key={ep.path}>
                  <td className="px-4 py-3">
                    <span className="rounded px-1.5 py-0.5 text-[11px] font-semibold bg-[var(--accent-primary)]/10 text-[var(--accent-primary)]">
                      {ep.method}
                    </span>
                  </td>
                  <td className="px-4 py-3 font-mono text-[12px] text-[var(--text-primary)]">
                    {ep.path}
                  </td>
                  <td className="px-4 py-3 text-[var(--text-secondary)]">{ep.description}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      {/* Resource cards */}
      <section className="space-y-4">
        <div className="flex items-center gap-2 text-[14px] font-semibold text-[var(--text-primary)]">
          <BookOpen size={18} className="text-[var(--accent-primary)]" /> Console guides
        </div>
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
          <Link
            to="/compute"
            className="group rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4 hover:border-[var(--accent-primary)]/30 hover:bg-[var(--bg-secondary)] transition-colors"
          >
            <div className="flex items-start justify-between gap-3">
              <div className="size-9 shrink-0 rounded-lg bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center">
                <Cloud size={18} />
              </div>
            </div>
            <div className="text-[14px] font-semibold text-[var(--text-primary)] mt-3">Hosted compute</div>
            <p className="text-[12px] text-[var(--text-secondary)] mt-1 leading-relaxed">
              Provision managed runtimes, set limits, and monitor usage.
            </p>
          </Link>

          <Link
            to="/billing"
            className="group rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4 hover:border-[var(--accent-primary)]/30 hover:bg-[var(--bg-secondary)] transition-colors"
          >
            <div className="flex items-start justify-between gap-3">
              <div className="size-9 shrink-0 rounded-lg bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center">
                <Gear size={18} />
              </div>
            </div>
            <div className="text-[14px] font-semibold text-[var(--text-primary)] mt-3">Organization & billing</div>
            <p className="text-[12px] text-[var(--text-secondary)] mt-1 leading-relaxed">
              Manage members, plans, credits, and usage.
            </p>
          </Link>

          <Link
            to="/api-keys"
            className="group rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4 hover:border-[var(--accent-primary)]/30 hover:bg-[var(--bg-secondary)] transition-colors"
          >
            <div className="flex items-start justify-between gap-3">
              <div className="size-9 shrink-0 rounded-lg bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center">
                <Key size={18} />
              </div>
            </div>
            <div className="text-[14px] font-semibold text-[var(--text-primary)] mt-3">API keys</div>
            <p className="text-[12px] text-[var(--text-secondary)] mt-1 leading-relaxed">
              Create scoped keys and rotate credentials.
            </p>
          </Link>
        </div>
      </section>

      <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4">
        <div className="flex items-center gap-2 text-[14px] font-semibold text-[var(--text-primary)] mb-2">
          <BookOpen size={18} className="text-[var(--accent-primary)]" /> Full API reference
        </div>
        <p className="text-[13px] text-[var(--text-secondary)] leading-relaxed m-0">
          Canonical docs live at{" "}
          <a
            href="https://docs.allternit.com/api/overview"
            target="_blank"
            rel="noopener noreferrer"
            className="text-[var(--accent-primary)] hover:underline"
          >
            docs.allternit.com/api/overview
          </a>
          . Cloud API (control plane) vs Allternit API (data plane), pairing, and
          relay are documented there. Need help? Open the in-app help from{" "}
          <a
            href="https://ai.allternit.com/shell"
            target="_blank"
            rel="noopener noreferrer"
            className="text-[var(--accent-primary)] hover:underline"
          >
            ai.allternit.com
          </a>
          .
        </p>
      </div>
    </div>
  );
}
