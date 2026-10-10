/**
 * Platform API console client: a project's payment method and plan.
 *
 * Every project, sandbox included, needs a card on file before it can do
 * billable work; until then the API answers 402 `payment_method_required`
 * with this page's link. A plan is chosen through Stripe Checkout
 * (subscription mode); the Stripe webhook puts the card on file.
 */

import { api } from "@/lib/api-client";

export type PlatformPlanId = "payg" | "growth";

export interface PlatformPlanOption {
  id: PlatformPlanId;
  name: string;
  monthly_cents: number;
  usage_credit_cents: number;
  available: boolean;
  summary: string;
}

export interface ProjectBilling {
  project_id: string;
  env: "sandbox" | "live";
  plan: string;
  payment_method: boolean;
  status: "active" | "past_due" | "unpaid" | "canceled" | null;
  subscription_id: string | null;
  payment_method_added_at: string | null;
  portal_available: boolean;
  checkout_available: boolean;
  spend_cap_cents: number;
  plans: PlatformPlanOption[];
  terms_url: string;
  acceptable_use_url: string;
}

export const DEVELOPER_TERMS_URL = "https://docs.allternit.com/legal/developer-terms";
export const ACCEPTABLE_USE_URL = "https://docs.allternit.com/legal/acceptable-use-policy";

const base = (projectId: string) => `/api/v1/platform/projects/${encodeURIComponent(projectId)}/billing`;

export async function getProjectBilling(projectId: string): Promise<ProjectBilling> {
  return api.get<ProjectBilling>(base(projectId));
}

/** Starts Stripe Checkout for the plan; returns the hosted Checkout URL. */
export async function startPlanCheckout(projectId: string, plan: PlatformPlanId): Promise<string> {
  const res = await api.post<{ checkout_url: string }>(`${base(projectId)}/checkout`, { plan });
  return res.checkout_url;
}

/** A Stripe customer portal link (change card, invoices, cancel). */
export async function openBillingPortal(projectId: string): Promise<string> {
  const res = await api.post<{ portal_url: string }>(`${base(projectId)}/portal`, {});
  return res.portal_url;
}
