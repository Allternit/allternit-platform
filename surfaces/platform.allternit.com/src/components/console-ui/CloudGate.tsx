import React from "react";
import { useNavigate } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import { CloudIcon } from "@hugeicons/core-free-icons";
import { EmptyState } from "./EmptyState";

export type CloudGateFeature =
  | "batches"
  | "files"
  | "serviceAccounts"
  | "security"
  | "gizziUsage";

interface GateCopy {
  title: string;
  caption: string;
  ctaLabel: string;
  /** Internal path (router) or absolute https URL (new tab). */
  to: string;
}

const GATE_COPY: Record<CloudGateFeature, GateCopy> = {
  serviceAccounts: {
    title: "Service accounts",
    caption:
      "Service accounts aren't available on Allternit Cloud yet. Use project API keys for server-to-server access.",
    ctaLabel: "Go to API keys",
    to: "/api-keys",
  },
  batches: {
    title: "Batches",
    caption:
      "Batches aren't available on Allternit Cloud. Send requests one at a time with chat completions.",
    ctaLabel: "Chat completions docs",
    to: "https://docs.allternit.com/api/chat-completions",
  },
  files: {
    title: "Files",
    caption:
      "File storage isn't available on Allternit Cloud. Send content inline with chat completions.",
    ctaLabel: "Chat completions docs",
    to: "https://docs.allternit.com/api/chat-completions",
  },
  security: {
    title: "Security",
    caption:
      "Data retention and residency settings aren't available on Allternit Cloud yet. The docs describe how data is handled today.",
    ctaLabel: "Read the security overview",
    to: "https://docs.allternit.com/security/overview",
  },
  gizziUsage: {
    title: "Gizzi Code usage",
    caption:
      "Gizzi Code usage isn't reported on Allternit Cloud yet. Project usage shows your Platform API usage.",
    ctaLabel: "Go to project usage",
    to: "/platform/usage",
  },
};

/**
 * Honest state for a console route that Allternit Cloud does not serve.
 * Renders no wrapped page and makes no API call.
 */
export function CloudGate({ feature }: { feature: CloudGateFeature }): React.ReactElement {
  const navigate = useNavigate();
  const copy = GATE_COPY[feature];
  const external = /^https?:\/\//.test(copy.to);
  return (
    <div className="flex min-h-[60vh] items-center justify-center">
      <EmptyState
        icon={<HugeiconsIcon icon={CloudIcon} size={32} />}
        title={copy.title}
        caption={copy.caption}
        ctaLabel={copy.ctaLabel}
        onCtaClick={() =>
          external ? window.open(copy.to, "_blank", "noopener,noreferrer") : navigate(copy.to)
        }
        className="max-w-md"
      />
    </div>
  );
}
