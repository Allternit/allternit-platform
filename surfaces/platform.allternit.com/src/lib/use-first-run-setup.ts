import { useEffect, useState } from "react";
import { listGatewayKeys } from "@/lib/gateway-keys";
import { listApiKeys } from "@/lib/api-keys";
import { useClerk, usePlatformUser } from "@/lib/platform-auth-client";
import {
  readLocalSetupRecord,
  readMetadataRecord,
  shouldOfferSetup,
} from "@/lib/platform-setup";

/** How long the dashboard may wait on the key lists before it just renders. */
const CHECK_BUDGET_MS = 2500;
const SESSION_FLAG = "allternit:platform-setup:offered";

/**
 * Decides whether the signed-in account should see first-run setup.
 *
 * Fast paths (no network): a setup record on the account or in this browser,
 * or any organization membership → "console". Otherwise the gateway-key and
 * scoped-key lists are read; only two successful empty lists mean "setup".
 * Errors and the time budget fall back to "console", and setup is offered at
 * most once per browser session, so it can never trap an existing user.
 */
export function useFirstRunSetup(enabled: boolean): "checking" | "setup" | "console" {
  const clerk = useClerk();
  const { user } = usePlatformUser();
  const [result, setResult] = useState<"checking" | "setup" | "console">("checking");

  const userId = user?.id ?? null;
  const clerkUser = clerk?.user ?? null;

  useEffect(() => {
    if (!enabled) {
      setResult("console");
      return;
    }
    let offered: boolean;
    try {
      offered = window.sessionStorage.getItem(SESSION_FLAG) === "1";
    } catch {
      offered = false;
    }
    const record = readMetadataRecord(clerkUser?.unsafeMetadata) ?? readLocalSetupRecord(userId);
    const memberships = clerkUser?.organizationMemberships;
    const orgCount = Array.isArray(memberships) ? memberships.length : null;

    if (offered || record || orgCount === null || orgCount > 0) {
      setResult("console");
      return;
    }

    let active = true;
    const timer = window.setTimeout(() => active && setResult("console"), CHECK_BUDGET_MS);
    const count = (p: Promise<unknown[]>) => p.then((l) => l.length).catch(() => null);
    Promise.all([count(listGatewayKeys()), count(listApiKeys())]).then(([gatewayKeys, scopedKeys]) => {
      if (!active) return;
      window.clearTimeout(timer);
      const offer = shouldOfferSetup({ record, orgCount, gatewayKeys, scopedKeys });
      if (offer) {
        try {
          window.sessionStorage.setItem(SESSION_FLAG, "1");
        } catch {
          // ignore
        }
      }
      setResult(offer ? "setup" : "console");
    });
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [enabled, clerkUser, userId]);

  return result;
}
