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
  | "gizziUsage"
  | "caching"
  | "tags"
  | "routingPolicy"
  | "routeCredentials";

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
  caching: {
    title: "Caching",
    caption:
      "Cache reporting isn't available on Allternit Cloud yet. Project usage shows your Platform API requests and tokens.",
    ctaLabel: "Go to project usage",
    to: "/platform/usage",
  },
  tags: {
    title: "Tags",
    caption:
      "Tags aren't available on Allternit Cloud yet. Use projects to separate apps and environments.",
    ctaLabel: "Go to projects",
    to: "/platform/projects",
  },
  routingPolicy: {
    title: "Routing policy",
    caption:
      "Provider routing policy applies to a paired Allternit runtime and isn't available on Allternit Cloud. Models lists what Allternit Cloud serves.",
    ctaLabel: "Go to models",
    to: "/models",
  },
  routeCredentials: {
    title: "Route credentials",
    caption:
      "Bring-your-own route credentials apply to a paired Allternit runtime and aren't available on Allternit Cloud. Project API keys call Allternit Cloud directly.",
    ctaLabel: "Go to API keys",
    to: "/api-keys",
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
