import { afterEach, describe, expect, it, vi } from "vitest";
import {
  AllternitApiClient,
  AllternitApiError,
  NO_RUNTIME_MESSAGE,
  formatApiError,
  isNoRuntime,
} from "../api-client";

/** Fake gateway: answers 200 only for the bearer it currently accepts. */
function fakeGateway(accepts: () => string | null) {
  const seen: Array<string | null> = [];
  const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
    const headers = (init?.headers ?? {}) as Record<string, string>;
    const auth = headers.Authorization ?? null;
    seen.push(auth);
    if (auth !== `Bearer ${accepts()}`) {
      return new Response(
        JSON.stringify({ error: "The Clerk session is invalid or expired.", code: "UNAUTHORIZED" }),
        { status: 401, headers: { "Content-Type": "application/json" } }
      );
    }
    return new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  });
  vi.stubGlobal("fetch", fetchMock);
  return { seen };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("session token", () => {
  it("sends the token Clerk has now, not one cached earlier", async () => {
    // Clerk's getToken() rotates the JWT; the old one is expired server-side.
    let clerkToken = "jwt-1";
    const gateway = fakeGateway(() => clerkToken);
    const client = new AllternitApiClient();
    client.setTokenProvider(async () => clerkToken);

    await expect(client.get("/api/v1/agents")).resolves.toEqual({ ok: true });

    clerkToken = "jwt-2"; // jwt-1 expired while the tab sat idle
    await expect(client.get("/api/v1/agents")).resolves.toEqual({ ok: true });
    await expect(client.get("/api/v1/vaults")).resolves.toEqual({ ok: true });

    expect(gateway.seen).toEqual(["Bearer jwt-1", "Bearer jwt-2", "Bearer jwt-2"]);
  });

  it("retries a 401 once with a freshly minted token", async () => {
    // Clerk's cache still holds an expired JWT; skipCache mints a new one.
    const gateway = fakeGateway(() => "jwt-new");
    const client = new AllternitApiClient();
    client.setTokenProvider(async (options) => (options?.skipCache ? "jwt-new" : "jwt-stale"));

    await expect(client.get("/api/v1/agents")).resolves.toEqual({ ok: true });
    expect(gateway.seen).toEqual(["Bearer jwt-stale", "Bearer jwt-new"]);
  });

  it("surfaces the 401 after one retry when the fresh token is rejected too", async () => {
    const gateway = fakeGateway(() => "never");
    const client = new AllternitApiClient();
    client.setTokenProvider(async (options) => (options?.skipCache ? "jwt-b" : "jwt-a"));

    const err = await client.get("/api/v1/agents").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AllternitApiError);
    expect((err as AllternitApiError).statusCode).toBe(401);
    expect(gateway.seen).toEqual(["Bearer jwt-a", "Bearer jwt-b"]);
  });

  it("keeps a caller's own Authorization header and never retries it", async () => {
    const gateway = fakeGateway(() => "jwt");
    const client = new AllternitApiClient();
    client.setTokenProvider(async () => "jwt");

    const res = await client.raw("/v1/chat/completions", {
      method: "POST",
      headers: { Authorization: "Bearer ak-virtual-key" },
    });
    expect(res.status).toBe(401);
    expect(gateway.seen).toEqual(["Bearer ak-virtual-key"]);
  });

  it("still sends a static token set without a provider", async () => {
    const gateway = fakeGateway(() => "static-key");
    const client = new AllternitApiClient();
    client.setToken("static-key");

    await expect(client.get("/api/v1/agents")).resolves.toEqual({ ok: true });
    expect(gateway.seen).toEqual(["Bearer static-key"]);
  });
});

describe("no data-plane node (428)", () => {
  it("maps 428 to a no-runtime error with a readable message", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(
          JSON.stringify({
            error: {
              code: "PRECONDITION_REQUIRED",
              message:
                "No data-plane node registered for this account — pair a device (or start a hosted runtime) and try again",
            },
          }),
          { status: 428, headers: { "Content-Type": "application/json" } }
        )
      )
    );
    const client = new AllternitApiClient();
    client.setToken("t");

    const err = await client.get("/api/v1/agents").catch((e: unknown) => e);
    expect(isNoRuntime(err)).toBe(true);
    const shown = formatApiError(err, "Unable to load agents");
    expect(shown).toBe(NO_RUNTIME_MESSAGE);
    // Pages keep the formatted string in state; it still identifies the case.
    expect(isNoRuntime(shown)).toBe(true);
  });

  it("does not treat other errors as no-runtime", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(JSON.stringify({ error: "boom" }), {
          status: 500,
          headers: { "Content-Type": "application/json" },
        })
      )
    );
    const client = new AllternitApiClient();
    client.setToken("t");

    const err = await client.get("/api/v1/agents").catch((e: unknown) => e);
    expect(isNoRuntime(err)).toBe(false);
    expect(formatApiError(err, "x")).toBe("boom");
  });
});
