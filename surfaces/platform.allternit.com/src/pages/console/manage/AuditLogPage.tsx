import React from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import { Audit01Icon } from "@hugeicons/core-free-icons";
import { EmptyState } from "@/components/console-ui";

/**
 * Audit log placeholder. The governance lane (Wave 4) replaces this with the
 * real list over GET /api/v1/platform/audit-log. Makes no API call.
 */
export function AuditLogPage(): React.ReactNode {
  return (
    <div className="mx-auto w-full max-w-3xl px-4 py-6">
      <h1 className="m-0 text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">Audit log</h1>
      <p className="m-0 mt-1 text-[13px] text-[var(--text-secondary)]">
        Who changed keys, projects and organization settings, and when.
      </p>
      <div className="mt-5 rounded-xl border border-solid border-[var(--border-subtle)]">
        <EmptyState
          icon={<HugeiconsIcon icon={Audit01Icon} size={32} />}
          title="No audit events to show yet"
          caption="Changes to API keys, projects and organization policy will be listed here."
        />
      </div>
    </div>
  );
}

export default AuditLogPage;
