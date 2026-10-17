import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import "@/styles/onboarding.css";
import { ConsoleHeader } from "@/components/onboarding/ConsoleHeader";
import { AOrb, CheckIcon, Gizzi, BlockField, type OrbState } from "@/components/onboarding/art";
import { GraphCanvas, type GraphNode } from "@/components/onboarding/GraphCanvas";
import { usePrefersReducedMotion } from "@/components/onboarding/useReducedMotion";
import {
  countText,
  hasAny,
  usePlatformSnapshot,
  type PlatformSnapshot,
} from "@/components/onboarding/usePlatformSnapshot";
import {
  useClerk,
  usePlatformAuth,
  usePlatformOrganization,
  usePlatformUser,
} from "@/lib/platform-auth-client";
import { api, formatApiError } from "@/lib/api-client";
import { createGatewayKey } from "@/lib/gateway-keys";
import { createApiKey } from "@/lib/api-keys";
import { createProject, isPlatformDisabled, listProjects } from "@/lib/platform-projects";
import { createPersonalOrganization } from "@/lib/design/current-user";
import {
  ORB_STATE_BY_STEP,
  blockStates,
  buildSnippet,
  describePlan,
  maskKey,
  paidPlans,
  rippleDelays,
  stepperStates,
  writeLocalSetupRecord,
  type KeyKind,
  type Lang,
  type LivePlan,
  type OrgKind,
  type Place,
  type SetupRecord,
} from "@/lib/platform-setup";

/* ─────────────────────────── types ─────────────────────────── */

interface CardData {
  title: string;
  chip: string;
  warn?: boolean;
  lines: string[];
  k: string;
  v: string;
}

interface DoneRow {
  title: string;
  summary: string;
  chip: string;
  tone: "" | "w" | "s";
}

interface Tile {
  id: string;
  mono: string;
  name: string;
  sub: string;
  tag?: string;
}

interface OrgResult {
  name: string;
  created: boolean;
  project: string;
}

interface KeyResult {
  kind: KeyKind;
  prefix: string;
}

interface CallResult {
  ok: boolean;
  status: number;
  reply?: string;
  model?: string;
  error?: string;
}

const PLACES: Array<Tile & { id: Place; node: string }> = [
  { id: "hw", mono: "H", name: "Your hardware", sub: "Pair a machine and run local models on it.", node: "Your hardware" },
  { id: "managed", mono: "M", name: "Allternit-managed compute", sub: "Managed machines, billed from your credits.", node: "Managed compute" },
  {
    id: "cloud",
    mono: "C",
    name: "Cloud model supplier",
    sub: "Together AI, Groq, Fireworks, DeepInfra, DeepSeek, OpenAI, Kimi.",
    node: "Cloud suppliers",
  },
];

const KEYS: Array<Tile & { id: KeyKind }> = [
  {
    id: "gateway",
    mono: "G",
    name: "Gateway API key",
    sub: "Calls the OpenAI-compatible API: /v1/chat/completions.",
    tag: "Recommended",
  },
  { id: "scoped", mono: "S", name: "Scoped key", sub: "For the cloud API with scopes: Read, Compute.", tag: "Scopes" },
];

const LANGS: Array<Tile & { id: Lang }> = [
  { id: "curl", mono: "$", name: "curl", sub: "Run it from any terminal.", tag: "Shell" },
  { id: "python", mono: "Py", name: "Python", sub: "The OpenAI SDK, pointed at Allternit.", tag: "SDK" },
  { id: "ts", mono: "TS", name: "TypeScript", sub: "The OpenAI SDK, pointed at Allternit.", tag: "SDK" },
];

const RAIL_CELL = 40;

function cloudApiBase(): string {
  return String(import.meta.env.VITE_ALLTERNIT_CLOUD_API_URL || "https://api.allternit.com").replace(/\/$/, "");
}

/* ─────────────────────────── small pieces ─────────────────────────── */

function ResultCard({ card, orb = "working" }: { card: CardData; orb?: OrbState }) {
  return (
    <div className="ob-rcard" role="status">
      <div className="ob-ch">
        <span className="ob-orb">
          <AOrb state={orb} />
        </span>
        <span className="ob-ch-t">
          A:// <span>·</span> {card.title}
        </span>
        <span className={`ob-chip ${card.warn ? "w" : ""}`}>{card.chip}</span>
      </div>
      <div className="ob-cb">
        {card.lines.map((l, i) => (
          <div key={`${i}-${l}`} className="ob-ln" style={{ animationDelay: `${(0.25 + i * 0.3).toFixed(2)}s` }}>
            <i>▸</i>
            {l}
            {i === card.lines.length - 1 && <b>✓</b>}
          </div>
        ))}
      </div>
      <div className="ob-kv">
        <span>{card.k}</span>
        <b>{card.v}</b>
      </div>
    </div>
  );
}

function Tiles({
  tiles,
  row,
  multi,
  selected,
  onPick,
  disabled,
  label,
}: {
  tiles: Tile[];
  row?: boolean;
  multi?: boolean;
  selected: string[];
  onPick: (id: string, el: HTMLElement) => void;
  disabled?: boolean;
  label: string;
}) {
  return (
    <div className={`ob-tiles ${row ? "row" : ""}`} role="group" aria-label={label}>
      {tiles.map((t) => {
        const on = selected.includes(t.id);
        return (
          <button
            key={t.id}
            type="button"
            className={`ob-tile ${on ? "on" : ""}`}
            aria-pressed={on}
            disabled={disabled}
            onClick={(e) => onPick(t.id, e.currentTarget)}
          >
            <span className="ob-mono">{t.mono}</span>
            <span className="ob-tile-t">
              <b>{t.name}</b>
              <small>{t.sub}</small>
            </span>
            {multi ? (
              <span className="ob-check">
                <CheckIcon />
              </span>
            ) : (
              t.tag && <em>{t.tag}</em>
            )}
          </button>
        );
      })}
    </div>
  );
}

/** Haptic block squares: fire only when a setup choice is picked. */
function Ripple({ origin, seq }: { origin: [number, number]; seq: number }) {
  const delays = useMemo(() => rippleDelays(14, 18, origin[0], origin[1]), [origin]);
  return (
    <div className="ob-ripbox" aria-hidden="true">
      <div key={seq} className="ob-rip">
        {delays.map((dd, i) => (
          <span key={i} className={dd === 0 ? "ob-rc" : ""} style={{ animationDelay: `${(dd * 0.045).toFixed(3)}s` }} />
        ))}
      </div>
    </div>
  );
}

/* ─────────────────────────── the page ─────────────────────────── */

export function SetupPage() {
  const navigate = useNavigate();
  const clerk = useClerk();
  const auth = usePlatformAuth();
  const { user } = usePlatformUser();
  const { organization } = usePlatformOrganization();
  const reduced = usePrefersReducedMotion();
  const railRef = useRef<HTMLElement | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const [step, setStep] = useState(1);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [ripple, setRipple] = useState<{ seq: number; origin: [number, number] } | null>(null);

  // 1 · organization
  const [orgKind, setOrgKind] = useState<OrgKind | null>(null);
  const [orgName, setOrgName] = useState("");
  const [org, setOrg] = useState<OrgResult | null>(null);
  // 2 · where it runs
  const [places, setPlaces] = useState<Place[]>([]);
  const [placesNote, setPlacesNote] = useState<string | null>(null);
  // 3 · API key (full token only in memory, only on this page)
  const [keyKind, setKeyKind] = useState<KeyKind | null>(null);
  const [key, setKey] = useState<KeyResult | null>(null);
  const [token, setToken] = useState<string | null>(null);
  const [revealed, setRevealed] = useState(false);
  const [copied, setCopied] = useState(false);
  // 4 · first call
  const [lang, setLang] = useState<Lang | null>(null);
  const [model, setModel] = useState<string>("");
  const [call, setCall] = useState<CallResult | null>(null);
  const [calling, setCalling] = useState(false);
  const [snipCopied, setSnipCopied] = useState(false);
  // 5 · plan
  const [plans, setPlans] = useState<LivePlan[] | null>(null);
  const [plansError, setPlansError] = useState(false);
  const [currentPlan, setCurrentPlan] = useState<string | null>(null);
  const [plan, setPlan] = useState<string>("free");
  // 6 · connected view
  const [focusId, setFocusId] = useState("key");

  const getToken = useCallback(() => auth.getToken(), [auth]);
  const snap = usePlatformSnapshot(step >= 2, getToken, step === 6 ? 1 : 0);

  // An org that already exists (invited users, or a revisit) is used as is.
  useEffect(() => {
    if (organization && !org) {
      setOrg({ name: organization.name, created: false, project: "" });
      setOrgKind("team");
    }
  }, [organization, org]);

  // Default model for the first call: the first model the live catalog lists.
  useEffect(() => {
    if (!model && snap.modelList.length > 0) setModel(snap.modelList[0].id);
  }, [snap.modelList, model]);

  // Plans + current subscription (public plans list; subscription needs the session).
  useEffect(() => {
    if (step < 5 || plans !== null) return;
    let active = true;
    const base = cloudApiBase();
    fetch(`${base}/api/v1/billing/plans`)
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
      .then((p: { plans?: LivePlan[] }) => active && setPlans(Array.isArray(p.plans) ? p.plans : []))
      .catch(() => {
        if (!active) return;
        setPlans([]);
        setPlansError(true);
      });
    getToken()
      .then((t) =>
        t ? fetch(`${base}/api/v1/billing/subscription`, { headers: { Authorization: `Bearer ${t}` } }) : null,
      )
      .then((r) => (r && r.ok ? r.json() : null))
      .then((s: { plan_id?: string; status?: string } | null) => {
        if (active && s?.plan_id && s.status) {
          setCurrentPlan(s.plan_id);
          setPlan(s.plan_id);
        }
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, [step, plans, getToken]);

  // The full key is shown once: hide it as soon as the user leaves step 3.
  useEffect(() => {
    if (step !== 3) setRevealed(false);
  }, [step]);

  const fire = useCallback(
    (el: HTMLElement) => {
      if (reduced) return;
      const rail = railRef.current;
      if (!rail) return;
      const r = rail.getBoundingClientRect();
      const t = el.getBoundingClientRect();
      const col = Math.max(0, Math.floor((t.left + Math.min(40, t.width / 4) - r.left) / RAIL_CELL));
      const row = Math.max(0, Math.floor((t.top + t.height / 2 - r.top + rail.scrollTop) / RAIL_CELL));
      setRipple((prev) => ({ seq: (prev?.seq ?? 0) + 1, origin: [col, row] }));
    },
    [reduced],
  );

  /* ── persistence ── */

  const saveRecord = useCallback(
    async (record: SetupRecord) => {
      writeLocalSetupRecord(user?.id, record);
      const cu = clerk?.user;
      if (!cu?.update) return;
      try {
        await cu.update({ unsafeMetadata: { ...(cu.unsafeMetadata ?? {}), platformSetup: record } });
      } catch {
        // The browser copy still keeps setup from reopening here.
      }
    },
    [clerk, user?.id],
  );

  const finish = useCallback(
    async (status: SetupRecord["status"]) => {
      await saveRecord({ status, at: new Date().toISOString(), places, plan });
      const paid = plans ? paidPlans(plans).find((p) => p.id === plan) : undefined;
      if (status === "completed" && plan === "sales") {
        window.location.href = "mailto:sales@allternit.com?subject=Allternit%20Platform";
        navigate("/", { replace: true });
        return;
      }
      if (status === "completed" && paid && paid.id !== currentPlan) {
        navigate(`/billing?plan=${encodeURIComponent(paid.id)}`, { replace: true });
        return;
      }
      navigate("/", { replace: true });
    },
    [saveRecord, places, plan, plans, currentPlan, navigate],
  );

  /* ── step actions (the real API calls) ── */

  const createOrg = useCallback(async () => {
    if (!orgKind) return;
    const name = orgName.trim() || (orgKind === "personal" ? user?.firstName || "Personal" : "");
    if (!name) {
      setError("Give the organization a name.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (clerk?.createOrganization) {
        const created = await clerk.createOrganization({ name });
        await clerk.setActive({ organization: created.id });
      } else {
        await createPersonalOrganization();
      }
      let project = "";
      try {
        const existing = await listProjects();
        if (existing.length > 0) project = `Project · ${existing[0].name}`;
        else {
          const p = await createProject({ name: "default", env: "sandbox" });
          project = `Created project · ${p.name} (sandbox)`;
        }
      } catch (err) {
        project = isPlatformDisabled(err)
          ? "Projects: not enabled on this deployment yet"
          : `Project not created: ${formatApiError(err, "request failed")}`;
      }
      setOrg({ name, created: true, project });
    } catch (err) {
      setError(formatApiError(err, "Could not create the organization."));
    } finally {
      setBusy(false);
    }
  }, [orgKind, orgName, user?.firstName, clerk]);

  const savePlaces = useCallback(async () => {
    const cu = clerk?.user;
    if (!cu?.update) {
      setPlacesNote("Saved in this browser only.");
      return;
    }
    try {
      const prefs = { ...((cu.unsafeMetadata?.platformPreferences as object) ?? {}), places };
      await cu.update({ unsafeMetadata: { ...(cu.unsafeMetadata ?? {}), platformPreferences: prefs } });
      setPlacesNote(null);
    } catch {
      setPlacesNote("Could not save to your account. You can set routing later in Gateway → Routing policy.");
    }
  }, [clerk, places]);

  const createKey = useCallback(async () => {
    if (!keyKind) return;
    setBusy(true);
    setError(null);
    try {
      if (keyKind === "gateway") {
        const k = await createGatewayKey({ name: "Setup key" });
        setToken(k.key);
        setKey({ kind: "gateway", prefix: k.key_prefix || k.key.slice(0, 7) });
      } else {
        const k = await createApiKey({ name: "Setup key", scopes: ["read", "compute"] });
        setToken(k.token);
        setKey({ kind: "scoped", prefix: k.prefix || k.token.slice(0, 7) });
      }
      setRevealed(true);
      setCopied(false);
    } catch (err) {
      setError(formatApiError(err, "Could not create the key."));
    } finally {
      setBusy(false);
    }
  }, [keyKind]);

  const copyText = useCallback(async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      return false;
    }
  }, []);

  const runCall = useCallback(async () => {
    if (!token || key?.kind !== "gateway" || !model) return;
    setCalling(true);
    setCall(null);
    try {
      const res = await api.raw("/v1/chat/completions", {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify({ model, messages: [{ role: "user", content: "Hello" }], max_tokens: 64 }),
      });
      const body = (await res.json().catch(() => ({}))) as {
        model?: string;
        choices?: Array<{ message?: { content?: string } }>;
        error?: { message?: string } | string;
        message?: string;
      };
      if (res.ok) {
        setCall({
          ok: true,
          status: res.status,
          model: body.model,
          reply: body.choices?.[0]?.message?.content?.trim() || "(empty reply)",
        });
      } else {
        const msg =
          (typeof body.error === "object" ? body.error?.message : body.error) || body.message || res.statusText;
        setCall({ ok: false, status: res.status, error: msg || "Request failed" });
      }
    } catch (err) {
      setCall({ ok: false, status: 0, error: formatApiError(err, "Network error") });
    } finally {
      setCalling(false);
    }
  }, [token, key?.kind, model]);

  /* ── navigation ── */

  const canNext =
    step === 1 ? !!org : step === 2 ? places.length > 0 : step === 3 ? !!key : step === 4 ? !!lang : step === 5 ? !!plan : true;

  const primary = (): { label: string; onClick: () => void; disabled: boolean } => {
    if (step === 1 && !org)
      return {
        label: busy ? "Creating…" : "Create organization",
        onClick: () => void createOrg(),
        disabled: busy || !orgKind || (orgKind === "team" && !orgName.trim()),
      };
    if (step === 3 && !key)
      return { label: busy ? "Creating…" : "Create key", onClick: () => void createKey(), disabled: busy || !keyKind };
    if (step === 2)
      return {
        label: "Next",
        onClick: () => {
          void savePlaces();
          setStep(3);
        },
        disabled: !canNext,
      };
    return {
      label: step === 5 ? "Show my platform" : "Next",
      onClick: () => {
        setError(null);
        setStep((s) => Math.min(6, s + 1));
      },
      disabled: !canNext,
    };
  };

  const skipStep = () => {
    setError(null);
    setStep((s) => Math.min(6, s + 1));
  };
  const back = () => {
    setError(null);
    setStep((s) => Math.max(1, s - 1));
  };

  /* ── derived view data ── */

  const placeNames = PLACES.filter((p) => places.includes(p.id)).map((p) => p.node);
  const maskedKey = key ? maskKey(key.prefix) : "";
  const livePaid = plans ? paidPlans(plans) : [];
  const planTiles: Tile[] = [
    {
      id: "free",
      mono: "F",
      name: "Free tier",
      sub: "Free tier includes local models and standard rate limits.",
      tag: currentPlan ? undefined : "Current",
    },
    ...livePaid.map((p) => ({
      id: p.id,
      mono: "$",
      name: p.label,
      sub: describePlan(p),
      tag: p.id === currentPlan ? "Current" : undefined,
    })),
    { id: "sales", mono: "S", name: "Contact sales", sub: "For teams that need custom terms.", tag: "Talk to us" },
  ];
  const planName = planTiles.find((t) => t.id === plan)?.name ?? "Free tier";

  const card: CardData | null = (() => {
    if (step === 1 && org)
      return {
        title: "Organization",
        chip: org.created ? "Created" : "In use",
        lines: [
          `${org.created ? "Created organization" : "Using organization"} · ${org.name}`,
          ...(org.project ? [org.project] : []),
          orgKind === "team" ? "Invite teammates in Manage → Members" : "Only you are in it. Add people later.",
        ],
        k: "Organization",
        v: org.name,
      };
    if (step === 2 && places.length > 0) {
      const lines: string[] = [];
      if (places.includes("hw"))
        lines.push(
          hasAny(snap.devices)
            ? `Your hardware · ${countText(snap.devices, "paired device")}`
            : 'Your hardware · run "allternit pair" on a machine to add it',
        );
      if (places.includes("managed"))
        lines.push(
          hasAny(snap.runtimes)
            ? `Managed compute · ${countText(snap.runtimes, "runtime")}`
            : "Managed compute · start a runtime in Compute",
        );
      if (places.includes("cloud"))
        lines.push(
          hasAny(snap.suppliers)
            ? `Cloud suppliers · ${countText(snap.suppliers, "supplier")} in the catalog`
            : "Cloud suppliers · routed through the gateway",
        );
      lines.push(placesNote ?? "Saved to your account as a preference");
      const pending = places.includes("hw") && !hasAny(snap.devices);
      return { title: "Where it runs", chip: pending ? "Pair pending" : "Saved", warn: pending, lines, k: "Runs on", v: placeNames.join(" · ") };
    }
    if (step === 3 && key)
      return {
        title: "API key",
        chip: copied ? "Copied" : "Shown once",
        warn: !copied,
        lines: [
          `Created ${key.kind === "gateway" ? "gateway API key" : "scoped key · read, compute"}`,
          key.kind === "gateway" ? "Works with /v1/chat/completions" : "Works with the cloud API (/api/v1)",
          `Key ${maskedKey} (shown once)`,
        ],
        k: "Key",
        v: key.kind === "gateway" ? "Gateway API key" : "Scoped key",
      };
    if (step === 4 && lang) {
      if (call)
        return call.ok
          ? {
              title: "First call",
              chip: `${call.status} OK`,
              lines: [`Model · ${call.model || model}`, `Reply: "${(call.reply ?? "").slice(0, 70)}"`, `POST /v1/chat/completions · ${call.status}`],
              k: "Model",
              v: call.model || model,
            }
          : {
              title: "First call",
              chip: call.status ? String(call.status) : "Failed",
              warn: true,
              lines: [`POST /v1/chat/completions · ${call.status || "no response"}`, call.error ?? "Request failed", "Nothing was faked: this is what the API returned"],
              k: "Model",
              v: model,
            };
      return {
        title: "First call",
        chip: "Ready",
        lines: [
          `Snippet · ${LANGS.find((l) => l.id === lang)?.name}`,
          `Base URL · api.allternit.com/v1`,
          model ? `Model · ${model}` : "Model · pick one in Models",
        ],
        k: "Model",
        v: model || "—",
      };
    }
    if (step === 5 && plan) {
      const lines =
        plan === "free"
          ? ["Plan: Free tier", "Local models and standard rate limits", "Add credits any time in Billing"]
          : plan === "sales"
            ? ["Contact sales selected", "Opens an email to sales@allternit.com after setup", "Free tier stays on meanwhile"]
            : plan === currentPlan
              ? [`Plan: ${planName}`, "This is your current plan", "Manage it any time in Billing"]
              : [`Plan: ${planName} selected`, "Checkout opens in Billing after setup", "Nothing is charged until checkout"];
      return {
        title: "Plan",
        chip: plan === "free" || plan === currentPlan ? "Active" : "Pending",
        warn: !(plan === "free" || plan === currentPlan),
        lines,
        k: "Plan",
        v: planName,
      };
    }
    return null;
  })();

  const done: DoneRow[] = [];
  if (step >= 2) done.push({ title: "Organization", summary: org ? org.name : "Not created", chip: org ? (org.created ? "Created" : "In use") : "Skipped", tone: org ? "" : "s" });
  if (step >= 3) done.push({ title: "Where it runs", summary: placeNames.length ? placeNames.join(" · ") : "Not set", chip: placeNames.length ? "Saved" : "Skipped", tone: placeNames.length ? "" : "s" });
  if (step >= 4) done.push({ title: "API key", summary: key ? `${key.kind === "gateway" ? "Gateway key" : "Scoped key"} · ${maskedKey}` : "No key yet", chip: key ? "Created" : "Skipped", tone: key ? "" : "s" });
  if (step >= 5)
    done.push({
      title: "First call",
      summary: call ? (call.ok ? `${call.status} OK` : `${call.status || "failed"}`) : lang ? "Snippet copied or ready" : "Not sent",
      chip: call ? String(call.status || "err") : lang ? "Ready" : "Skipped",
      tone: call?.ok ? "" : lang ? "" : "s",
    });

  const question = [
    "",
    org && !org.created
      ? `Hi, I'm Gizzi. A:// and I will set up your platform. You're already in ${org.name}.`
      : "Hi, I'm Gizzi. A:// and I will set up your platform. Is this for you or a team?",
    "Where can your workloads run? Pick any you want.",
    "Next, an API key. Which kind?",
    "Let's make your first call. How do you want to send it?",
    currentPlan ? `Last one. You're on ${planName}. Keep it?` : "Last one. You're on the free tier. Keep it, or pick a plan?",
  ][step];

  const orbState = (ORB_STATE_BY_STEP[step] ?? "idle") as OrbState;
  const hubLabel = org ? `${org.name}${org.project.includes("default") ? " · project default" : ""}` : "Your organization";

  // Left stage graph that builds as steps complete (694×620, hub 347,300).
  const stageNodes: GraphNode[] = [];
  if (step >= 1) {
    stageNodes.push({ id: "org", label: org ? `${org.name}` : "Organization", x: 347, y: 120, lit: !!org });
    stageNodes.push({ id: "project", label: "Project", x: 540, y: 170, lit: !!org && /project/i.test(org.project) && !/not/i.test(org.project) });
  }
  if (step >= 2) {
    stageNodes.push({ id: "hw", label: "Your hardware", x: 140, y: 220, lit: places.includes("hw") });
    stageNodes.push({ id: "managed", label: "Managed compute", x: 120, y: 310, lit: places.includes("managed") });
    stageNodes.push({ id: "cloud", label: "Cloud suppliers", x: 140, y: 400, lit: places.includes("cloud") });
  }
  if (step >= 3) stageNodes.push({ id: "key", label: key ? `API key · ${maskedKey}` : "API key", x: 560, y: 300, lit: !!key });
  if (step >= 4) {
    stageNodes.push({ id: "model", label: model || "Model", x: 540, y: 430, lit: !!call?.ok });
    stageNodes.push({ id: "call", label: lang ? `${LANGS.find((l) => l.id === lang)?.name} · first call` : "First call", x: 347, y: 490, lit: !!lang });
  }
  if (step >= 5) stageNodes.push({ id: "plan", label: planName, x: 170, y: 500, lit: !!plan });

  // Keep the newest result card in view as the log grows.
  const cardSig = card ? `${step}|${card.chip}|${card.lines.join("|")}` : `${step}`;
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const last = el.querySelector(".ob-rcard, .ob-keybox, .ob-snipwrap");
    if (el.scrollHeight > el.clientHeight + 4) el.scrollTo({ top: el.scrollHeight, behavior: reduced ? "auto" : "smooth" });
    else if (last && window.innerWidth < 1024) (last as HTMLElement).scrollIntoView({ block: "nearest", behavior: reduced ? "auto" : "smooth" });
  }, [cardSig, reduced]);

  const p = primary();
  const blocks = blockStates(step);

  if (step === 6) {
    return (
      <ConnectedView
        snap={snap}
        org={org}
        keyResult={key}
        maskedKey={maskedKey}
        places={places}
        lang={lang}
        model={model}
        call={call}
        planName={planName}
        focusId={focusId}
        setFocusId={setFocusId}
        onBack={() => setStep(5)}
        onLeave={() => void saveRecord({ status: "completed", at: new Date().toISOString(), places, plan })}
        onOpen={() => void finish("completed")}
      />
    );
  }

  return (
    <div className="ob min-h-screen flex flex-col bg-[var(--bg-primary)] text-[var(--text-primary)]">
      <ConsoleHeader
        right={
          <button type="button" className="ob-ghost ob-skipall" onClick={() => void finish("skipped")}>
            Skip setup
          </button>
        }
      />
      <main className="ob-setup">
        <section className="ob-studio ob-setup-studio" aria-label="Your platform">
          <div className="ob-shead">
            <div className="ob-llab">
              <i />
              Your platform
            </div>
            <span className="ob-hint">Builds as you answer</span>
          </div>
          <div className="ob-stage">
            <div className="ob-field">
              <BlockField id="ob-field-setup" />
            </div>
            <GraphCanvas
              width={694}
              height={620}
              hubX={347}
              hubY={300}
              hubLabel={hubLabel}
              nodes={stageNodes}
              ariaLabel="Platform graph, building as you answer"
            />
          </div>
        </section>

        <section className="ob-rail ob-setup-rail" aria-label="Setup" ref={railRef}>
          <div className="ob-duo">
            <div className="ob-gz" role="img" aria-label="Gizzi">
              <Gizzi />
            </div>
            <span className="ob-plus">+</span>
            <div className="ob-orb" role="img" aria-label={`A:// orb, ${orbState}`}>
              <AOrb state={orbState} />
            </div>
            <div className="ob-duo-t">
              <b>Gizzi + A://</b>
              <span>
                <i />
                A:// is {orbState}
              </span>
            </div>
          </div>
          <ol className="ob-steps5" aria-label="Setup steps">
            {stepperStates(step).map((t) => (
              <li key={t.label} className={`ob-st ${t.state}`} aria-current={t.state === "now" ? "step" : undefined}>
                <b />
                <span>{t.label}</span>
              </li>
            ))}
          </ol>
          <div className="ob-blocks" aria-hidden="true">
            {blocks.map((b, i) => (
              <i key={i} className={b} />
            ))}
          </div>

          <div className="ob-scroll" ref={scrollRef}>
            {done.map((d) => (
              <div key={d.title} className="ob-done2">
                <span className="ob-orb">
                  <AOrb state="idle" />
                </span>
                <span className="ob-done2-t">{d.title}</span>
                <span className="ob-done2-s">{d.summary}</span>
                <span className={`ob-chip ${d.tone}`}>{d.chip}</span>
              </div>
            ))}
            <div className="ob-say" key={`say-${step}`}>
              {question}
            </div>

            {step === 1 && (
              <>
                {!(org && !org.created) && (
                  <div className="ob-name">
                    <label className="ob-lbl" htmlFor="ob-org">
                      Name
                    </label>
                    <input
                      className="ob-in"
                      id="ob-org"
                      type="text"
                      placeholder={orgKind === "personal" ? user?.firstName || "Personal" : "Organization name"}
                      value={orgName}
                      disabled={!!org || busy}
                      onChange={(e) => setOrgName(e.target.value)}
                      maxLength={64}
                    />
                  </div>
                )}
                {!(org && !org.created) && (
                  <Tiles
                    label="Organization type"
                    row
                    tiles={[
                      { id: "personal", mono: "P", name: "Personal", sub: "Just you. Add a team later.", tag: "Solo" },
                      { id: "team", mono: "T", name: "Team", sub: "Shared credits, keys and runs.", tag: "Invite later" },
                    ]}
                    selected={orgKind ? [orgKind] : []}
                    disabled={!!org || busy}
                    onPick={(id, el) => {
                      setOrgKind(id as OrgKind);
                      fire(el);
                    }}
                  />
                )}
              </>
            )}

            {step === 2 && (
              <Tiles
                label="Where workloads can run"
                multi
                tiles={PLACES}
                selected={places}
                onPick={(id, el) => {
                  setPlaces((cur) => (cur.includes(id as Place) ? cur.filter((x) => x !== id) : [...cur, id as Place]));
                  fire(el);
                }}
              />
            )}

            {step === 3 && (
              <>
                <Tiles
                  label="API key type"
                  tiles={KEYS}
                  selected={keyKind ? [keyKind] : []}
                  disabled={!!key || busy}
                  onPick={(id, el) => {
                    setKeyKind(id as KeyKind);
                    fire(el);
                  }}
                />
                {key && (
                  <div className="ob-keybox">
                    <small>{revealed ? "Copy it now. You won't see the full key again." : "Copied keys stay hidden. Create more in API keys."}</small>
                    <div className="ob-keyrow">
                      <code aria-label={revealed ? "Your new key" : "Your new key, hidden"}>
                        {revealed && token ? token : maskedKey}
                      </code>
                      {revealed && token && (
                        <button
                          type="button"
                          className={`ob-copy ${copied ? "done" : ""}`}
                          onClick={async () => {
                            if (await copyText(token)) {
                              setCopied(true);
                              setRevealed(false);
                            }
                          }}
                        >
                          {copied ? "Copied" : "Copy"}
                        </button>
                      )}
                    </div>
                  </div>
                )}
              </>
            )}

            {step === 4 && (
              <>
                <Tiles
                  label="How to send the first call"
                  row
                  tiles={LANGS}
                  selected={lang ? [lang] : []}
                  onPick={(id, el) => {
                    setLang(id as Lang);
                    setSnipCopied(false);
                    fire(el);
                  }}
                />
                {lang && (
                  <div className="ob-snipwrap">
                    <div className="ob-sniptools">
                      <label className="ob-lbl" htmlFor="ob-model">
                        Model
                      </label>
                      {snap.modelList.length > 0 ? (
                        <select id="ob-model" className="ob-in ob-select" value={model} onChange={(e) => { setModel(e.target.value); setCall(null); }}>
                          {snap.modelList.map((m) => (
                            <option key={m.id} value={m.id}>
                              {m.id}
                            </option>
                          ))}
                        </select>
                      ) : (
                        <span className="ob-hint">{snap.models.state === "loading" ? "Loading models…" : "Model list unavailable. See Models."}</span>
                      )}
                    </div>
                    <pre className="ob-snip" aria-label="First call snippet">
                      {buildSnippet({ lang, model: model || "MODEL_ID", key: key ? maskedKey : "$ALLTERNIT_API_KEY" })}
                    </pre>
                    <div className="ob-snipact">
                      <button
                        type="button"
                        className={`ob-copy ${snipCopied ? "done" : ""}`}
                        onClick={async () => {
                          const real = token && key?.kind === "gateway" ? token : "$ALLTERNIT_API_KEY";
                          if (await copyText(buildSnippet({ lang, model: model || "MODEL_ID", key: real }))) setSnipCopied(true);
                        }}
                      >
                        {snipCopied ? "Copied" : token && key?.kind === "gateway" ? "Copy with your key" : "Copy"}
                      </button>
                      {token && key?.kind === "gateway" && model ? (
                        <button type="button" className="ob-btn ink ob-runbtn" disabled={calling} onClick={() => void runCall()}>
                          {calling ? "Running…" : call ? "Run it again" : "Run it"}
                        </button>
                      ) : (
                        <span className="ob-hint">
                          {key?.kind === "scoped"
                            ? "Scoped keys can't call /v1. Create a gateway key in API keys to run it."
                            : !key
                              ? "Create a gateway key to run it from here."
                              : "Pick a model to run it."}
                        </span>
                      )}
                    </div>
                  </div>
                )}
              </>
            )}

            {step === 5 && (
              <>
                <Tiles
                  label="Plan"
                  tiles={planTiles}
                  selected={[plan]}
                  onPick={(id, el) => {
                    setPlan(id);
                    fire(el);
                  }}
                />
                {plansError && <p className="ob-hint">Paid plans could not be loaded here. They are on the Billing page.</p>}
              </>
            )}

            {card && <ResultCard key={`${step}-${card.chip}-${card.lines.join("|")}`} card={card} orb={orbState} />}
            {error && (
              <p className="ob-err" role="alert">
                {error}
              </p>
            )}
          </div>

          <div className="ob-foot">
            <button type="button" className="ob-ghost" onClick={back} disabled={step === 1}>
              Back
            </button>
            <button type="button" className="ob-ghost" onClick={skipStep}>
              Skip
            </button>
            <button type="button" className="ob-btn ink" disabled={p.disabled} onClick={p.onClick}>
              {p.label}
            </button>
          </div>
          {ripple && !reduced && <Ripple key={ripple.seq} seq={ripple.seq} origin={ripple.origin} />}
        </section>
      </main>
    </div>
  );
}

/* ─────────────────────────── connected view ─────────────────────────── */

function useIsPhone(): boolean {
  const q = "(max-width: 640px)";
  const [m, setM] = useState(() => typeof window !== "undefined" && window.matchMedia?.(q).matches);
  useEffect(() => {
    const mq = window.matchMedia?.(q);
    if (!mq) return;
    const on = () => setM(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return !!m;
}

interface ConnNode {
  id: string;
  g: string;
  t: string;
  x: number;
  y: number;
  conn: boolean;
  status: string;
  desc: string;
  to?: string;
  href?: string;
}

function ConnectedView(props: {
  snap: PlatformSnapshot;
  org: OrgResult | null;
  keyResult: KeyResult | null;
  maskedKey: string;
  places: Place[];
  lang: Lang | null;
  model: string;
  call: CallResult | null;
  planName: string;
  focusId: string;
  setFocusId: (id: string) => void;
  onBack: () => void;
  onLeave: () => void;
  onOpen: () => void;
}) {
  const { snap, org, keyResult, maskedKey, places, lang, model, call, planName, focusId, setFocusId } = props;
  const keyCount =
    snap.gatewayKeys.state === "ok" && snap.scopedKeys.state === "ok" ? snap.gatewayKeys.n + snap.scopedKeys.n : null;
  const models = snap.models.state === "ok" ? (snap.models.names ?? []) : [];

  const N: ConnNode[] = [
    { id: "org", g: "Organization", t: org ? org.name : "Organization", x: 520, y: 235, conn: !!org, status: org ? (org.created ? "Set up just now" : "In use") : "Not set up", desc: "Owns your credits, keys, runs and members.", to: "/organizations" },
    { id: "project", g: "Organization", t: "Projects", x: 760, y: 235, conn: hasAny(snap.projects), status: countText(snap.projects, "project"), desc: "Project keys, spend caps and Platform API resources are grouped by project.", to: "/platform/projects" },
    { id: "key", g: "Organization", t: keyResult ? `API key · ${maskedKey}` : "API keys", x: 785, y: 425, conn: !!keyResult || (keyCount ?? 0) > 0, status: keyResult ? "Set up just now" : keyCount === null ? "Couldn't load" : keyCount === 0 ? "No keys yet" : `${keyCount} active key${keyCount === 1 ? "" : "s"}`, desc: keyResult ? "Created during setup. The full key was shown once." : "Create gateway and scoped keys in API keys.", to: "/api-keys" },
    { id: "plan", g: "Organization", t: planName, x: 495, y: 425, conn: true, status: "Current choice", desc: "Change plans or add credits any time in Billing.", to: "/billing" },
    ...[0, 1, 2].map((i) => ({
      id: `model-${i}`,
      g: "Models",
      t: models[i] ?? (i === 0 ? "Models" : ""),
      x: [470, 640, 810][i],
      y: [110, 90, 110][i],
      conn: !!call?.ok && models[i] === (call.model || model),
      status: snap.models.state === "ok" ? (models[i] === model && call?.ok ? "Used by your first call" : "In the catalog") : countText(snap.models, "model"),
      desc: "Every model in the live catalog, behind one OpenAI-compatible API.",
      to: "/models",
    })).filter((n) => n.t),
    { id: "hw", g: "Where it runs", t: "Your hardware", x: 330, y: 250, conn: hasAny(snap.devices), status: hasAny(snap.devices) ? countText(snap.devices, "paired device") : places.includes("hw") ? "Chosen · pair a machine" : "Available", desc: 'Paired machines run local models. Run "allternit pair" on the machine, then approve it in Devices.', to: "/devices" },
    { id: "managed", g: "Where it runs", t: "Managed compute", x: 330, y: 330, conn: hasAny(snap.runtimes), status: hasAny(snap.runtimes) ? countText(snap.runtimes, "runtime") : places.includes("managed") ? "Chosen · start a runtime" : "Available", desc: "Allternit-managed machines, billed from your credits.", to: "/compute" },
    { id: "cloud", g: "Where it runs", t: "Cloud suppliers", x: 330, y: 410, conn: places.includes("cloud") && hasAny(snap.suppliers), status: countText(snap.suppliers, "supplier"), desc: "Cloud model suppliers behind one API. Set provider order in Gateway → Routing policy.", to: "/gateway/routing-policy" },
    { id: "runs", g: "Workloads", t: "Runs", x: 960, y: 240, conn: hasAny(snap.runs), status: countText(snap.runs, "run"), desc: "Start agent runs from the console or the CLI: allternit run.", to: "/runs" },
    { id: "schedules", g: "Workloads", t: "Schedules", x: 960, y: 305, conn: hasAny(snap.schedules), status: countText(snap.schedules, "schedule"), desc: "Run agents on a schedule.", to: "/schedules" },
    { id: "webhooks", g: "Workloads", t: "Webhooks", x: 960, y: 370, conn: hasAny(snap.webhooks), status: countText(snap.webhooks, "webhook"), desc: "Get called when a run finishes or needs approval.", to: "/manage/webhooks" },
    { id: "batches", g: "Workloads", t: "Batches", x: 960, y: 435, conn: false, status: "Available", desc: "Send large volumes of requests asynchronously and retrieve results when they complete.", to: "/batches" },
    { id: "curl", g: "SDKs", t: "curl", x: 520, y: 545, conn: lang === "curl", status: lang === "curl" ? "Your first call" : "Available", desc: "Any HTTP client works. The API is OpenAI-compatible.", href: "https://docs.allternit.com" },
    { id: "python", g: "SDKs", t: "Python", x: 640, y: 560, conn: lang === "python", status: lang === "python" ? "Your first call" : "Available", desc: "Use the OpenAI SDK with base_url set to api.allternit.com/v1.", href: "https://docs.allternit.com" },
    { id: "ts", g: "SDKs", t: "TypeScript", x: 760, y: 545, conn: lang === "ts", status: lang === "ts" ? "Your first call" : "Available", desc: "Use the OpenAI SDK with baseURL set to api.allternit.com/v1.", href: "https://docs.allternit.com" },
  ];
  const focus = N.find((n) => n.id === focusId) ?? N[0];
  const phone = useIsPhone();
  const connCount = N.filter((n) => n.conn).length;
  const groups = ["Organization", "Models", "Where it runs", "Workloads", "SDKs"].map((g) => ({ g, n: N.filter((x) => x.g === g).length }));
  const nodes: GraphNode[] = N.map((n) => ({ id: n.id, label: n.t, x: n.x, y: n.y, lit: n.conn, focus: n.id === focus.id, status: n.status, onPick: () => setFocusId(n.id) }));
  // Phone: the eight nodes that matter most on a 370×330 board, the rest as group chips.
  const PHONE_PICK: Array<[string, string, number, number]> = [
    ["org", "Org", 185, 40], ["key", "API key", 316, 110], ["plan", "Plan", 316, 250], ["hw", "Hardware", 185, 310],
    ["managed", "Managed", 54, 250], ["cloud", "Suppliers", 54, 110], ["model-0", "Model", 90, 22], [lang ?? "curl", lang === "python" ? "Python" : lang === "ts" ? "TypeScript" : "curl", 280, 22],
  ];
  const phoneNodes: GraphNode[] = PHONE_PICK.flatMap(([id, label, x, y]) => {
    const n = N.find((m) => m.id === id);
    return n ? [{ id, label, x, y, lit: n.conn, focus: id === focus.id, status: n.status, onPick: () => setFocusId(id) }] : [];
  });
  const phoneRest = groups.map((g) => ({ g: g.g, n: N.filter((x) => x.g === g.g && !PHONE_PICK.some((pp) => pp[0] === x.id)) }));
  const summary = [
    `Organization: ${org ? org.name : "not set up"}`,
    `Runs on: ${places.length ? places.map((p) => PLACES.find((x) => x.id === p)?.node).join(", ") : "not set"}`,
    `API key: ${keyResult ? `${maskedKey} (shown once)` : "none yet"}`,
    `First call: ${call ? (call.ok ? `${call.status} OK` : `${call.status || "failed"}`) : lang ? "snippet ready" : "not sent"}`,
    `Plan: ${planName}`,
  ];

  return (
    <div className="ob min-h-screen flex flex-col bg-[var(--bg-primary)] text-[var(--text-primary)]">
      <ConsoleHeader />
      <div className="ob-conn">
        <div className="ob-ctop">
          <span className="ob-ctop-l">Your platform</span>
          <div className="ob-ctop-r">
            Setup complete
            <div className="ob-blocks" aria-hidden="true">
              {blockStates(6).map((b, i) => (
                <i key={i} className={b} />
              ))}
            </div>
          </div>
        </div>
        <div className="ob-cmain">
          <div className="ob-field">
            <BlockField id="ob-field-conn" />
          </div>
          <div className="ob-cgraph">
            {phone ? (
              <GraphCanvas width={370} height={330} hubX={185} hubY={170} hubLabel="" nodes={phoneNodes} ariaLabel="Everything connected to your platform" />
            ) : (
            <GraphCanvas
              width={1280}
              height={640}
              hubX={640}
              hubY={330}
              hubLabel={org ? org.name : "Your organization"}
              nodes={nodes}
              labels={[
                { t: "Models", x: 640, y: 52 },
                { t: "Where it runs", x: 330, y: 205 },
                { t: "Workloads", x: 960, y: 195 },
                { t: "SDKs", x: 640, y: 590 },
              ]}
              ariaLabel="Everything connected to your platform"
            />
            )}
          </div>
          {phone && (
            <div className="ob-pgroups" role="group" aria-label="More you can add">
              {phoneRest.flatMap((g) =>
                g.n.map((n) => (
                  <button key={n.id} type="button" className="ob-pg" aria-pressed={n.id === focus.id} onClick={() => setFocusId(n.id)}>
                    {n.t} <b>{n.conn ? "✓" : "+"}</b>
                  </button>
                )),
              )}
            </div>
          )}
          <aside className="ob-legend" aria-label="Legend">
            <h3>Your platform</h3>
            <div className="ob-lrow">
              <span>Set up</span>
              <b>{connCount}</b>
            </div>
            <div className="ob-lrow">
              <span>Ready to add</span>
              <b>{N.length - connCount}</b>
            </div>
            {groups.map((g) => (
              <div key={g.g} className="ob-lrow">
                <span>{g.g}</span>
                <b>{g.n}</b>
              </div>
            ))}
            <div className="ob-key">
              <span>
                <i />
                Set up
              </span>
              <span>
                <i className="d" />
                Available
              </span>
            </div>
          </aside>
          <div className="ob-detail" role="region" aria-live="polite" aria-label="Selected">
            <small>{focus.g}</small>
            <h4>{focus.t}</h4>
            <p>{focus.desc}</p>
            <div className="ob-detail-f">
              <span className={`ob-dst ${focus.conn ? "conn" : ""}`}>{focus.status}</span>
              {focus.to ? (
                <Link className="ob-dlink" to={focus.to} onClick={props.onLeave}>
                  Open
                </Link>
              ) : focus.href ? (
                <a className="ob-dlink" href={focus.href} target="_blank" rel="noopener noreferrer">
                  Docs
                </a>
              ) : null}
            </div>
          </div>
          <div className="ob-rcard ob-csum" role="status">
            <div className="ob-ch">
              <span className="ob-orb">
                <AOrb state="idle" />
              </span>
              <span className="ob-ch-t">
                A:// <span>·</span> Summary
              </span>
              <span className="ob-chip">Done</span>
            </div>
            <div className="ob-cb">
              {summary.map((l, i) => (
                <div key={l} className="ob-ln" style={{ animationDelay: `${(0.9 + i * 0.3).toFixed(2)}s` }}>
                  <i>▸</i>
                  {l}
                  {i === summary.length - 1 && <b>✓</b>}
                </div>
              ))}
            </div>
          </div>
          <div className="ob-gbar">
            <div className="ob-av">
              <Gizzi />
            </div>
            <div className="ob-bub">This is your platform so far. Tap any node to see what it does.</div>
            <span className="ob-sp" />
            <button className="ob-ghost" type="button" onClick={props.onBack}>
              Back
            </button>
            <button className="ob-cta" type="button" onClick={props.onOpen}>
              Open Console
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path d="M9 6l6 6-6 6" />
              </svg>
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
