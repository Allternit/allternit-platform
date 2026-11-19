import React, { useCallback, useEffect, useState } from "react";
import { Link, Route, Routes, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, Plus, ShieldCheck } from "@phosphor-icons/react";
import { formatApiError } from "@/lib/api-client";
import { EmptyState, SkeletonRow } from "@/components/console-ui";
import { QUIET_BUTTON_CLASS } from "@/components/console-ui/buttonStyles";
import { CreatedNotice, OAUTH_INPUT_CLASS as INPUT, OAUTH_PRIMARY_CLASS as PRIMARY, RedirectEditor, SecretBox, StateActions, StateBadge } from "@/components/oauth/parts";
import {
  MAX_REDIRECT_URIS,
  MAX_TEST_USERS,
  OAUTH_DOCS_URL,
  OAUTH_SCOPES,
  OAUTH_TOKEN_URL,
  TESTING_NOTICE,
  addRedirectUri,
  addTestUser,
  createOAuthApp,
  deleteOAuthApp,
  getOAuthApp,
  listOAuthApps,
  listTestUsers,
  removeRedirectUri,
  removeTestUser,
  rotateOAuthSecret,
  sampleAuthorizeUrl,
  submitOAuthApp,
  updateOAuthApp,
  validateHttpsUrl,
  validateRedirectUri,
  validateTestEmail,
  withdrawOAuthApp,
  type OAuthApp,
  type OAuthClientType,
} from "@/lib/oauth-apps";
import { CARD_CLASS, ErrorBanner, PlatformHeader, ProjectGate, formatDate, usePlatformProjects } from "./shared";

/** "Sign in with Allternit" apps for the selected project: list, create, detail. */
export function OAuthAppsPage() {
  const state = usePlatformProjects();
  return (
    <div className="mx-auto w-full max-w-5xl">
      <ProjectGate state={state}>
        {({ project }) => (
          <Routes>
            <Route index element={<AppList key={project.id} projectId={project.id} />} />
            <Route path="new" element={<CreateApp key={project.id} projectId={project.id} />} />
            <Route path=":app" element={<AppDetail key={project.id} projectId={project.id} />} />
          </Routes>
        )}
      </ProjectGate>
    </div>
  );
}

function AppList({ projectId }: { projectId: string }) {
  const [apps, setApps] = useState<OAuthApp[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    setError(null);
    try {
      setApps(await listOAuthApps(projectId));
    } catch (err) {
      setApps([]);
      setError(formatApiError(err, "Unable to load OAuth apps"));
    }
  }, [projectId]);
  useEffect(() => {
    void load();
  }, [load]);

  return (
    <>
      <PlatformHeader
        title="OAuth apps"
        subtitle="Let people sign in to your app with Allternit and spend their own credits."
        actions={
          <Link to="/platform/oauth-apps/new" className={PRIMARY}>
            <Plus size={14} aria-hidden /> New OAuth app
          </Link>
        }
      />
      {error && <ErrorBanner message={error} className="mb-3" />}
      {apps === null ? (
        <div className={CARD_CLASS} role="status" aria-label="Loading OAuth apps">
          <SkeletonRow />
          <SkeletonRow />
        </div>
      ) : apps.length === 0 && !error ? (
        <div className={CARD_CLASS}>
          <EmptyState
            icon={<ShieldCheck size={32} aria-hidden />}
            title="No OAuth apps yet"
            caption="Create one to get a client ID and let users link your app to their Allternit account."
          />
        </div>
      ) : (
        <div className={`${CARD_CLASS} overflow-x-auto`}>
          <table className="w-full text-left text-[13px]">
            <thead>
              <tr className="border-b border-solid border-[var(--border-subtle)] text-[12px] text-[var(--text-tertiary)]">
                <th scope="col" className="px-4 py-3 font-medium">Name</th>
                <th scope="col" className="px-4 py-3 font-medium hidden sm:table-cell">Client ID</th>
                <th scope="col" className="px-4 py-3 font-medium">State</th>
                <th scope="col" className="px-4 py-3 font-medium hidden md:table-cell">Created</th>
              </tr>
            </thead>
            <tbody>
              {apps.map((a) => (
                <tr key={a.id} className="border-b border-solid border-[var(--border-subtle)] last:border-0">
                  <td className="px-4 py-3">
                    <Link to={`/platform/oauth-apps/${encodeURIComponent(a.id)}`} className="font-medium text-[var(--text-primary)] hover:underline">
                      {a.name}
                    </Link>
                  </td>
                  <td className="px-4 py-3 hidden sm:table-cell text-[var(--text-secondary)]"><code>{a.client_id}</code></td>
                  <td className="px-4 py-3"><StateBadge state={a.state} /></td>
                  <td className="px-4 py-3 hidden md:table-cell text-[var(--text-secondary)]">{formatDate(a.created_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

function Field({ label, error, children }: { label: string; error?: string | null; children: React.ReactNode }) {
  return (
    <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
      {label}
      <div className="mt-1.5">{children}</div>
      {error && <span role="alert" className="mt-1 block text-[12px] font-normal text-[var(--status-error)]">{error}</span>}
    </label>
  );
}

function ScopeChecks({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  return (
    <fieldset className="m-0 border-0 p-0">
      <legend className="text-[12px] font-medium text-[var(--text-secondary)]">Scopes</legend>
      <div className="mt-1.5 grid gap-1.5 sm:grid-cols-2">
        {OAUTH_SCOPES.map((s) => (
          <label key={s.value} className="flex items-start gap-2 text-[13px] text-[var(--text-primary)]">
            <input
              type="checkbox"
              className="mt-0.5"
              checked={value.includes(s.value)}
              onChange={(e) => onChange(e.target.checked ? [...value, s.value] : value.filter((x) => x !== s.value))}
            />
            <span>
              <code>{s.label}</code>
              <span className="block text-[12px] text-[var(--text-tertiary)]">{s.hint}</span>
            </span>
          </label>
        ))}
      </div>
    </fieldset>
  );
}

function AppFields({
  v,
  set,
  showType,
}: {
  v: { name: string; homepage: string; privacy: string; logo: string; type: OAuthClientType; scopes: string[] };
  set: (p: Partial<typeof v>) => void;
  showType: boolean;
}) {
  return (
    <div className="space-y-3">
      <Field label="Name">
        <input value={v.name} onChange={(e) => set({ name: e.target.value })} maxLength={80} placeholder="Notes Sync" className={INPUT} />
      </Field>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label="Homepage URL" error={validateHttpsUrl(v.homepage)}>
          <input value={v.homepage} onChange={(e) => set({ homepage: e.target.value })} inputMode="url" placeholder="https://example.com" className={INPUT} />
        </Field>
        <Field label="Privacy URL" error={validateHttpsUrl(v.privacy)}>
          <input value={v.privacy} onChange={(e) => set({ privacy: e.target.value })} inputMode="url" placeholder="https://example.com/privacy" className={INPUT} />
        </Field>
      </div>
      <Field label="Logo URL" error={validateHttpsUrl(v.logo)}>
        <input value={v.logo} onChange={(e) => set({ logo: e.target.value })} inputMode="url" placeholder="https://example.com/logo.png" className={INPUT} />
      </Field>
      {showType && (
        <fieldset className="m-0 border-0 p-0">
          <legend className="text-[12px] font-medium text-[var(--text-secondary)]">Client type</legend>
          <div className="mt-1.5 space-y-1.5 text-[13px] text-[var(--text-primary)]">
            {(
              [
                ["confidential", "Confidential: your server keeps a client secret."],
                ["public", "Public: a browser or mobile app, PKCE only, no secret."],
              ] as const
            ).map(([t, text]) => (
              <label key={t} className="flex items-center gap-2">
                <input type="radio" name="client-type" checked={v.type === t} onChange={() => set({ type: t })} />
                {text}
              </label>
            ))}
          </div>
        </fieldset>
      )}
      <ScopeChecks value={v.scopes} onChange={(scopes) => set({ scopes })} />
    </div>
  );
}

const orNull = (s: string) => (s.trim() ? s.trim() : null);

function CreateApp({ projectId }: { projectId: string }) {
  const navigate = useNavigate();
  const [v, setV] = useState({ name: "", homepage: "", privacy: "", logo: "", type: "confidential" as OAuthClientType, scopes: ["inference"] });
  const [uris, setUris] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<{ app: OAuthApp; secret: string | null } | null>(null);
  const urlsOk = !validateHttpsUrl(v.homepage) && !validateHttpsUrl(v.privacy) && !validateHttpsUrl(v.logo);
  const valid = v.name.trim() && v.scopes.length > 0 && uris.length > 0 && urlsOk;

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!valid) return;
    setBusy(true);
    setError(null);
    try {
      const res = await createOAuthApp(projectId, {
        name: v.name.trim(),
        homepage_url: orNull(v.homepage),
        privacy_url: orNull(v.privacy),
        logo_url: orNull(v.logo),
        client_type: v.type,
        scopes: v.scopes,
        redirect_uris: uris,
      });
      const { client_secret, ...app } = res;
      setCreated({ app, secret: client_secret ?? null });
    } catch (err) {
      setError(formatApiError(err, "Unable to create the app"));
    } finally {
      setBusy(false);
    }
  };

  if (created) {
    return (
      <>
        <PlatformHeader title="OAuth app created" subtitle="Keep the client ID. Link it from your app with the authorize URL." />
        <div className={`${CARD_CLASS} p-4`}>
          <CreatedNotice
            app={created.app}
            secret={created.secret}
            onClose={() => setCreated((c) => (c ? { ...c, secret: null } : c))}
          />
          <div className="mt-4">
            <button type="button" className={PRIMARY} onClick={() => navigate(`/platform/oauth-apps/${encodeURIComponent(created.app.id)}`)}>
              Open app
            </button>
          </div>
        </div>
      </>
    );
  }

  return (
    <>
      <BackLink />
      <PlatformHeader title="New OAuth app" subtitle="Describe your app and where Allternit may send people back." />
      <form onSubmit={submit} className={`${CARD_CLASS} space-y-4 p-4`}>
        {error && <ErrorBanner message={error} />}
        <AppFields v={v} set={(p) => setV((c) => ({ ...c, ...p }))} showType />
        <div>
          <div className="mb-1.5 text-[12px] font-medium text-[var(--text-secondary)]">Redirect URIs</div>
          <RedirectEditor
            uris={uris}
            onAdd={(u) => setUris((c) => (validateRedirectUri(u, c) ? c : [...c, u]))}
            onRemove={(u) => setUris((c) => c.filter((x) => x !== u))}
          />
        </div>
        <div className="flex flex-wrap gap-2">
          <button type="submit" className={PRIMARY} disabled={busy || !valid}>
            {busy ? "Creating…" : "Create app"}
          </button>
          <Link to="/platform/oauth-apps" className={QUIET_BUTTON_CLASS}>Cancel</Link>
        </div>
        {!uris.length && <p className="m-0 text-[12px] text-[var(--text-tertiary)]">Add at least one redirect URI (up to {MAX_REDIRECT_URIS}).</p>}
      </form>
    </>
  );
}

function BackLink() {
  return (
    <Link to="/platform/oauth-apps" className="mb-3 inline-flex items-center gap-1 text-[13px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]">
      <ArrowLeft size={14} aria-hidden /> OAuth apps
    </Link>
  );
}

function AppDetail({ projectId }: { projectId: string }) {
  const { app: appId = "" } = useParams();
  const navigate = useNavigate();
  const [app, setApp] = useState<OAuthApp | null>(null);
  const [testUsers, setTestUsers] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [secret, setSecret] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<"rotate" | "delete" | null>(null);
  const [form, setForm] = useState({ name: "", homepage: "", privacy: "", logo: "", type: "confidential" as OAuthClientType, scopes: [] as string[] });
  const [email, setEmail] = useState("");

  const adopt = (a: OAuthApp) => {
    setApp(a);
    setForm({ name: a.name, homepage: a.homepage_url ?? "", privacy: a.privacy_url ?? "", logo: a.logo_url ?? "", type: a.client_type, scopes: a.scopes });
  };

  useEffect(() => {
    let live = true;
    setLoading(true);
    Promise.all([getOAuthApp(projectId, appId), listTestUsers(projectId, appId).catch(() => [] as string[])])
      .then(([a, t]) => {
        if (!live) return;
        adopt(a);
        setTestUsers(t);
      })
      .catch((err) => live && setError(formatApiError(err, "Unable to load this app")))
      .finally(() => live && setLoading(false));
    return () => {
      live = false;
      setSecret(null); // never keep the secret past this view
    };
  }, [projectId, appId]);

  const run = async (fn: () => Promise<void>, fallback: string) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (err) {
      setError(formatApiError(err, fallback));
    } finally {
      setBusy(false);
    }
  };

  if (loading) {
    return (
      <div className={CARD_CLASS} role="status" aria-label="Loading app">
        <SkeletonRow />
        <SkeletonRow />
      </div>
    );
  }
  if (!app) {
    return (
      <>
        <BackLink />
        <ErrorBanner message={error ?? "App not found."} />
      </>
    );
  }

  const urlsOk = !validateHttpsUrl(form.homepage) && !validateHttpsUrl(form.privacy) && !validateHttpsUrl(form.logo);
  const emailError = email.trim() ? validateTestEmail(email, testUsers) : null;

  return (
    <>
      <BackLink />
      <PlatformHeader
        title={app.name}
        subtitle={`Client ID ${app.client_id}`}
        actions={<StateBadge state={app.state} />}
      />
      <div className="space-y-4">
        {error && <ErrorBanner message={error} />}
        {app.state === "testing" && (
          <div role="note" className="rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-3 text-[13px] text-[var(--text-primary)]">
            {TESTING_NOTICE}
          </div>
        )}
        {app.state === "suspended" && (
          <ErrorBanner message="This app is suspended. People cannot link it. Contact Allternit to resolve it." />
        )}
        {secret && <SecretBox secret={secret} onClose={() => setSecret(null)} />}

        <section className={`${CARD_CLASS} space-y-4 p-4`} aria-label="App details">
          <AppFields v={form} set={(p) => setForm((c) => ({ ...c, ...p }))} showType={false} />
          <div className="text-[12px] text-[var(--text-tertiary)]">
            {app.client_type === "confidential" ? `Confidential app, secret ${app.secret_prefix ?? ""}…` : "Public app, PKCE only"}
          </div>
          <button
            type="button"
            className={PRIMARY}
            disabled={busy || !form.name.trim() || !form.scopes.length || !urlsOk}
            onClick={() =>
              run(async () => {
                adopt(await updateOAuthApp(projectId, app.id, {
                  name: form.name.trim(),
                  homepage_url: orNull(form.homepage),
                  privacy_url: orNull(form.privacy),
                  logo_url: orNull(form.logo),
                  scopes: form.scopes,
                }));
              }, "Unable to save")
            }
          >
            Save changes
          </button>
        </section>

        <section className={`${CARD_CLASS} p-4`} aria-label="Redirect URIs">
          <h2 className="m-0 mb-2 text-[14px] font-semibold text-[var(--text-primary)]">Redirect URIs</h2>
          <RedirectEditor
            uris={app.redirect_uris}
            busy={busy}
            onAdd={(u) => run(async () => adopt(await addRedirectUri(projectId, app.id, u)), "Unable to add the redirect URI")}
            onRemove={(u) => run(async () => adopt(await removeRedirectUri(projectId, app.id, u)), "Unable to remove the redirect URI")}
          />
        </section>

        <section className={`${CARD_CLASS} p-4 space-y-2`} aria-label="Test users">
          <h2 className="m-0 text-[14px] font-semibold text-[var(--text-primary)]">Test users</h2>
          <p className="m-0 text-[12px] text-[var(--text-tertiary)]">Up to {MAX_TEST_USERS} emails ({testUsers.length} used). They can link the app while it is in testing.</p>
          {testUsers.length === 0 ? (
            <p className="m-0 text-[13px] text-[var(--text-secondary)]">No test users yet.</p>
          ) : (
            <ul className="m-0 list-none space-y-1.5 p-0">
              {testUsers.map((t) => (
                <li key={t} className="flex items-center gap-2 rounded-lg bg-[var(--bg-secondary)] px-3 py-1.5 text-[13px]">
                  <span className="min-w-0 flex-1 break-all">{t}</span>
                  <button
                    type="button"
                    className="text-[12px] text-[var(--text-secondary)] hover:text-[var(--status-error)]"
                    aria-label={`Remove ${t}`}
                    disabled={busy}
                    onClick={() => run(async () => { await removeTestUser(projectId, app.id, t); setTestUsers((c) => c.filter((x) => x !== t)); }, "Unable to remove the test user")}
                  >
                    Remove
                  </button>
                </li>
              ))}
            </ul>
          )}
          <form
            className="flex flex-wrap gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              if (!email.trim() || emailError) return;
              const next = email.trim().toLowerCase();
              void run(async () => { await addTestUser(projectId, app.id, next); setTestUsers((c) => [...c, next]); setEmail(""); }, "Unable to add the test user");
            }}
          >
            <input type="email" aria-label="Test user email" aria-invalid={emailError ? true : undefined} value={email} onChange={(e) => setEmail(e.target.value)} placeholder="person@example.com" className={`${INPUT} min-w-0 flex-1`} />
            <button type="submit" className={QUIET_BUTTON_CLASS} disabled={busy || !email.trim() || !!emailError}>Add</button>
          </form>
          {emailError && <p role="alert" className="m-0 text-[12px] text-[var(--status-error)]">{emailError}</p>}
        </section>

        <section className={`${CARD_CLASS} p-4 space-y-2`} aria-label="How to use it">
          <h2 className="m-0 text-[14px] font-semibold text-[var(--text-primary)]">How to use it</h2>
          <p className="m-0 text-[12px] text-[var(--text-secondary)]">Send the user to the authorize URL (PKCE, S256), then exchange the code at the token URL.</p>
          <div className="text-[12px] text-[var(--text-tertiary)]">Authorize URL</div>
          <code className="block break-all rounded-lg bg-[var(--bg-secondary)] px-3 py-2 text-[12px]">{sampleAuthorizeUrl(app)}</code>
          <div className="text-[12px] text-[var(--text-tertiary)]">Token URL</div>
          <code className="block break-all rounded-lg bg-[var(--bg-secondary)] px-3 py-2 text-[12px]">{OAUTH_TOKEN_URL}</code>
          <a href={OAUTH_DOCS_URL} target="_blank" rel="noreferrer" className="text-[13px] font-semibold text-[var(--accent-primary)] hover:underline">
            Read the OAuth apps guide
          </a>
        </section>

        <section className={`${CARD_CLASS} p-4 space-y-3`} aria-label="Actions">
          <StateActions
            state={app.state}
            busy={busy}
            showRotate={app.client_type === "confidential"}
            onSubmit={() => run(async () => adopt(await submitOAuthApp(projectId, app.id)), "Unable to submit for review")}
            onWithdraw={() => run(async () => adopt(await withdrawOAuthApp(projectId, app.id)), "Unable to withdraw")}
            onRotate={() => setConfirm("rotate")}
            onDelete={() => setConfirm("delete")}
          />
          {confirm && (
            <div role="alertdialog" aria-label="Confirm" className="rounded-lg border border-solid border-[var(--status-error)]/25 bg-[var(--status-error)]/[0.06] p-3 text-[13px] text-[var(--text-primary)] space-y-2">
              <div>
                {confirm === "rotate"
                  ? "Rotate the client secret? The old secret stops working right away."
                  : `Delete ${app.name}? People linked to it lose access. This cannot be undone.`}
              </div>
              <div className="flex gap-2">
                <button
                  type="button"
                  autoFocus
                  className={PRIMARY}
                  disabled={busy}
                  onClick={() =>
                    run(async () => {
                      if (confirm === "rotate") {
                        const { client_secret, ...rest } = await rotateOAuthSecret(projectId, app.id);
                        adopt(rest);
                        setSecret(client_secret ?? null);
                      } else {
                        await deleteOAuthApp(projectId, app.id);
                        navigate("/platform/oauth-apps");
                      }
                      setConfirm(null);
                    }, confirm === "rotate" ? "Unable to rotate the secret" : "Unable to delete the app")
                  }
                >
                  {confirm === "rotate" ? "Rotate secret" : "Delete app"}
                </button>
                <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setConfirm(null)}>Cancel</button>
              </div>
            </div>
          )}
        </section>
      </div>
    </>
  );
}
