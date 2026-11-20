/**
 * "Sign in with Allternit" developer apps: client for
 * /api/v1/platform/projects/:id/oauth-apps and the admin review routes, plus the
 * redirect-URI and email rules the server enforces (mirrored here for fast feedback).
 * The client secret only ever appears in a create or rotate response; callers keep
 * it in component state and never store it.
 */
import { api } from "@/lib/api-client";

export type OAuthAppState = "testing" | "in_review" | "live" | "suspended";
export type OAuthClientType = "confidential" | "public";

export interface OAuthApp {
  id: string;
  project_id: string;
  owner_user_id?: string;
  client_id: string;
  name: string;
  logo_url: string | null;
  homepage_url: string | null;
  privacy_url: string | null;
  client_type: OAuthClientType;
  secret_prefix?: string | null;
  state: OAuthAppState;
  scopes: string[];
  created_at: string;
  updated_at: string;
  approved_at: string | null;
  approved_by: string | null;
  redirect_uris: string[];
}

/** Create and rotate responses: `client_secret` is null for public apps. */
export interface OAuthAppWithSecret extends OAuthApp {
  client_secret: string | null;
}

export interface OAuthAppInput {
  name: string;
  homepage_url?: string | null;
  privacy_url?: string | null;
  logo_url?: string | null;
  client_type?: OAuthClientType;
  scopes: string[];
  redirect_uris?: string[];
}

export const OAUTH_SCOPES: { value: string; label: string; hint: string }[] = [
  { value: "inference", label: "inference", hint: "Run models on the user's credits" },
  { value: "usage", label: "usage", hint: "Read the user's usage for your app" },
  { value: "profile", label: "profile", hint: "Name and email of the user" },
  { value: "offline_access", label: "offline_access", hint: "Refresh tokens, so the link outlives a session" },
];

export const MAX_REDIRECT_URIS = 10;
export const MAX_TEST_USERS = 100;

export const OAUTH_AUTHORIZE_URL = "https://api.allternit.com/oauth2/authorize";
export const OAUTH_TOKEN_URL = "https://api.allternit.com/oauth2/token";
export const OAUTH_DOCS_URL = "https://docs.allternit.com/api/oauth-apps";

export const TESTING_NOTICE =
  "Only test users can link this app, with up to $50 a month, until Allternit approves it.";
export const SECRET_ONCE_LINE = "Copy it now. You won't see it again.";

export const STATE_LABEL: Record<OAuthAppState, string> = {
  testing: "Testing",
  in_review: "In review",
  live: "Live",
  suspended: "Suspended",
};

const LOCAL_HOSTS = new Set(["localhost", "127.0.0.1", "[::1]"]);

/** Returns an error message, or null when the redirect URI is acceptable. */
export function validateRedirectUri(raw: string, existing: string[] = []): string | null {
  const uri = raw.trim();
  if (!uri) return "Enter a redirect URI.";
  if (uri.includes("*")) return "Wildcards are not allowed. Use the exact URI.";
  if (uri.includes("#")) return "Redirect URIs cannot have a fragment (#).";
  let url: URL;
  try {
    url = new URL(uri);
  } catch {
    return "Enter a full URI, for example https://example.com/callback.";
  }
  if (url.protocol === "https:") {
    // ok
  } else if (url.protocol === "http:") {
    if (!LOCAL_HOSTS.has(url.hostname)) return "http is only allowed on localhost, 127.0.0.1 or [::1]. Use https.";
  } else {
    return "Custom schemes are not allowed. Use https, or http on localhost.";
  }
  if (existing.includes(uri)) return "That URI is already on the list.";
  if (existing.length >= MAX_REDIRECT_URIS) return `At most ${MAX_REDIRECT_URIS} redirect URIs.`;
  return null;
}

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export function validateTestEmail(raw: string, existing: string[] = []): string | null {
  const email = raw.trim().toLowerCase();
  if (!EMAIL_RE.test(email)) return "Enter a valid email address.";
  if (existing.map((e) => e.toLowerCase()).includes(email)) return "That email is already a test user.";
  if (existing.length >= MAX_TEST_USERS) return `At most ${MAX_TEST_USERS} test users.`;
  return null;
}

export function validateHttpsUrl(raw: string): string | null {
  const v = raw.trim();
  if (!v) return null;
  try {
    const u = new URL(v);
    return u.protocol === "https:" || u.protocol === "http:" ? null : "Use an https URL.";
  } catch {
    return "Enter a full URL.";
  }
}

/** The authorize URL shown in the "How to use it" box. */
export function sampleAuthorizeUrl(app: Pick<OAuthApp, "client_id" | "redirect_uris" | "scopes">): string {
  const q = [
    `client_id=${app.client_id}`,
    `redirect_uri=${encodeURIComponent(app.redirect_uris[0] ?? "https://example.com/callback")}`,
    "response_type=code",
    "code_challenge=…",
    "code_challenge_method=S256",
    `scope=${app.scopes.join("%20")}`,
    "state=…",
  ];
  return `${OAUTH_AUTHORIZE_URL}?${q.join("&")}`;
}

const base = (projectId: string) => `/api/v1/platform/projects/${encodeURIComponent(projectId)}/oauth-apps`;
const appPath = (projectId: string, appId: string) => `${base(projectId)}/${encodeURIComponent(appId)}`;

interface List<T> {
  data: T[];
}

export async function listOAuthApps(projectId: string): Promise<OAuthApp[]> {
  const res = await api.get<List<OAuthApp> | OAuthApp[]>(base(projectId));
  return Array.isArray(res) ? res : res.data;
}
export const getOAuthApp = (p: string, a: string) => api.get<OAuthApp>(appPath(p, a));
export const createOAuthApp = (p: string, input: OAuthAppInput) => api.post<OAuthAppWithSecret>(base(p), input);
export const updateOAuthApp = (p: string, a: string, input: Partial<OAuthAppInput>) =>
  api.patch<OAuthApp>(appPath(p, a), input);
export const deleteOAuthApp = (p: string, a: string) => api.delete<void>(appPath(p, a));
export const rotateOAuthSecret = (p: string, a: string) =>
  api.post<OAuthAppWithSecret>(`${appPath(p, a)}/secret/rotate`, {});
export const submitOAuthApp = (p: string, a: string) => api.post<OAuthApp>(`${appPath(p, a)}/submit`, {});
export const withdrawOAuthApp = (p: string, a: string) => api.post<OAuthApp>(`${appPath(p, a)}/withdraw`, {});
export const addRedirectUri = (p: string, a: string, uri: string) =>
  api.post<OAuthApp>(`${appPath(p, a)}/redirect-uris`, { uri });
export const removeRedirectUri = (p: string, a: string, uri: string) =>
  api.delete<OAuthApp>(`${appPath(p, a)}/redirect-uris?uri=${encodeURIComponent(uri)}`);

export async function listTestUsers(p: string, a: string): Promise<string[]> {
  const res = await api.get<{ data?: (string | { email: string })[]; emails?: string[] } | string[]>(
    `${appPath(p, a)}/test-users`,
  );
  const raw = Array.isArray(res) ? res : res.emails ?? res.data ?? [];
  return raw.map((r) => (typeof r === "string" ? r : r.email));
}
export const addTestUser = (p: string, a: string, email: string) =>
  api.post<unknown>(`${appPath(p, a)}/test-users`, { email });
export const removeTestUser = (p: string, a: string, email: string) =>
  api.delete<unknown>(`${appPath(p, a)}/test-users/${encodeURIComponent(email)}`);

export async function adminListOAuthApps(state: OAuthAppState = "in_review"): Promise<OAuthApp[]> {
  const res = await api.get<List<OAuthApp> | OAuthApp[]>(`/api/v1/admin/oauth-apps?state=${state}`);
  return Array.isArray(res) ? res : res.data;
}
export const adminDecideOAuthApp = (id: string, action: "approve" | "suspend") =>
  api.post<OAuthApp>(`/api/v1/admin/oauth-apps/${encodeURIComponent(id)}/${action}`, {});
