// Every sentence the automation writes, in one place, for Eoj's approval at gate G1.
// Register 1: plain, direct, no hype, no promises about uptime or timing.
// {name} is the component name from contract/targets.json (e.g. "API", "Sign-in").

export const AUTO_INCIDENT = {
  /** New incident, component confirmed down by both vantages. */
  downTitle: (name: string) => `${name} is down`,
  downBody: (name: string) => `Our checks cannot reach ${name}. We are looking into it.`,

  /** New incident, component degraded (slow, partly failing, or failing from one place). */
  degradedTitle: (name: string) => `${name} is having problems`,
  degradedBody: (name: string) => `Our checks show ${name} is slow or partly failing. We are looking into it.`,

  /** Update on an open auto incident when it gets worse (degraded -> down). */
  escalatedBody: (name: string) => `${name} has stopped responding to our checks. We are still working on it.`,

  /** Update on an open auto incident when it improves but is not fixed (down -> degraded). */
  improvedBody: (name: string) => `${name} is responding again but is still slow or partly failing. We are watching it.`,

  /** Final update, appended when every component in the incident passes again. */
  resolvedBody: (names: string) => `${names} is working again. Our checks have passed for the last few minutes.`,
} as const;

export const MAINTENANCE = {
  /** Appended when a scheduled maintenance window reaches its end time. */
  completeBody: "The maintenance is complete.",
} as const;

/** Used when an admin changes the status without writing a message. */
export const ADMIN_DEFAULT_BODY: Record<"investigating" | "identified" | "monitoring" | "resolved", string> = {
  investigating: "We are looking into this.",
  identified: "We have found the cause and are working on a fix.",
  monitoring: "A fix is in place. We are watching it.",
  resolved: "This is resolved.",
};

/** Alert email subjects and lines (to the team inbox, not to customers). */
export const ALERT = {
  subjectPrefix: "[Allternit status]",
  down: "Down",
  degraded: "Degraded",
  recovered: "Recovered",
  stillDown: "Still down",
  stillDegraded: "Still degraded",
  footerDraft:
    "Auto incidents are off: a draft incident is waiting at {site}/admin. Publish it there if customers should see it.",
  footerLive: "The incident is public at {site}.",
} as const;
