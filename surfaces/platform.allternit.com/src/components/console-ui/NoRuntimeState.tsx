import React from "react";
import { Link } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import { ComputerIcon } from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { QUIET_BUTTON_CLASS } from "./buttonStyles";

export const NO_RUNTIME_DOCS_URL = "https://docs.allternit.com/byoc/managed-agents";

/**
 * Shown in place of an error banner when cloud-api answers 428
 * (no data-plane node for this account). Pages check `isNoRuntime(error)`
 * from `@/lib/api-client` and render this; every other error keeps the
 * page's normal banner.
 */
export function NoRuntimeState({
  className,
  compact = false,
}: {
  className?: string;
  /** Tighter padding for use inside a panel or drawer. */
  compact?: boolean;
}): React.ReactNode {
  return (
    <div
      role="status"
      className={cn(
        "flex flex-col items-center justify-center rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] text-center",
        compact ? "px-4 py-6" : "px-6 py-10",
        className
      )}
    >
      <div className="mb-3 text-[var(--text-tertiary)]">
        <HugeiconsIcon icon={ComputerIcon} size={compact ? 22 : 28} />
      </div>
      <h3 className="m-0 mb-1 text-[15px] font-semibold text-[var(--text-primary)]">
        Managed Agents run on a computer you connect.
      </h3>
      <p className="m-0 max-w-sm text-[13px] leading-relaxed text-[var(--text-secondary)]">
        Pair your Mac, PC or a cloud computer to run agents, sessions and vaults.
      </p>
      <div className="mt-4 flex flex-wrap items-center justify-center gap-2">
        <Link
          to="/devices"
          className="inline-flex items-center rounded-lg bg-[var(--accent-primary)] px-3 py-1.5 text-[13px] font-semibold text-[var(--ui-text-inverse)] no-underline transition-colors hover:brightness-110"
        >
          Connect a device
        </Link>
        <a
          href={NO_RUNTIME_DOCS_URL}
          target="_blank"
          rel="noopener noreferrer"
          className={cn(QUIET_BUTTON_CLASS, "no-underline")}
        >
          Docs
        </a>
      </div>
    </div>
  );
}
