import React from "react";
import { Routes, Route, Navigate, useLocation } from "react-router-dom";
import { ConsoleLayout } from "@/components/ConsoleLayout";
import { PublicPageShell } from "@/components/PublicPageShell";
import { DashboardPage } from "@/pages/DashboardPage";
import { OrganizationsPage } from "@/pages/OrganizationsPage";
import { ComputePage } from "@/pages/ComputePage";
import { DevicesPage } from "@/pages/DevicesPage";
import { BillingPage } from "@/pages/BillingPage";
import { ApiKeysPage } from "@/pages/ApiKeysPage";
import { DocsPage } from "@/pages/DocsPage";
import { SettingsPage } from "@/pages/SettingsPage";
import { CloudAccountsPage } from "@/pages/CloudAccountsPage";
import { SignInPage } from "@/pages/SignInPage";
import { SignUpPage } from "@/pages/SignUpPage";
import { RunsPage } from "@/pages/RunsPage";
import { SchedulesPage } from "@/pages/SchedulesPage";
import { ApprovalsPage } from "@/pages/ApprovalsPage";
import { FabricPage } from "@/pages/FabricPage";
import { PortalLandingPage } from "@/pages/PortalLandingPage";
import { ModelsPage } from "@/pages/ModelsPage";
import { PlansPage } from "@/pages/PlansPage";
import { AdminCustomersPage } from "@/pages/AdminCustomersPage";
import { PlaygroundPage } from "@/pages/console/PlaygroundPage";
import { FilesPage } from "@/pages/console/FilesPage";
import { BatchesPage } from "@/pages/console/BatchesPage";
import { SkillsPage } from "@/pages/console/SkillsPage";
import { BuilderPage } from "@/pages/console/BuilderPage";
import { AgentsPage } from "@/pages/console/agents/AgentsPage";
import { AgentDetailPage } from "@/pages/console/agents/AgentDetailPage";
import { AgentFormPage } from "@/pages/console/agents/AgentFormPage";
import { SessionsPage } from "@/pages/console/sessions/SessionsPage";
import { SessionNewPage } from "@/pages/console/sessions/SessionNewPage";
import { SessionDetailPage } from "@/pages/console/sessions/SessionDetailPage";
import { DeploymentsPage } from "@/pages/console/deployments/DeploymentsPage";
import { ComputersPage } from "@/pages/console/computers/ComputersPage";
import { VaultsPage, VaultDetailPage } from "@/pages/console/vaults/VaultsPage";
import { MemoryPage, MemoryStorePage } from "@/pages/console/memory/MemoryPage";
import { UsagePage } from "@/pages/console/analytics/UsagePage";
import { LogsPage } from "@/pages/console/analytics/LogsPage";
import { CachingPage } from "@/pages/console/analytics/CachingPage";
import { RateLimitsPage } from "@/pages/console/analytics/RateLimitsPage";
import { CostPage } from "@/pages/console/analytics/CostPage";
import { GizziUsagePage } from "@/pages/console/gizzi/GizziUsagePage";
import { RoutingPolicyPage } from "@/pages/console/gateway/RoutingPolicyPage";
import { RouteCredentialsPage } from "@/pages/console/gateway/RouteCredentialsPage";
import { MembersPage } from "@/pages/console/manage/MembersPage";
import { ServiceAccountsPage } from "@/pages/console/manage/ServiceAccountsPage";
import { SpendLimitsPage } from "@/pages/console/manage/SpendLimitsPage";
import { SecurityPage } from "@/pages/console/manage/SecurityPage";
import { WebhooksPage } from "@/pages/console/manage/WebhooksPage";
import { TagsPage } from "@/pages/console/manage/TagsPage";
import { PlatformProjectsPage } from "@/pages/console/platform/ProjectsPage";
import { PlatformAgentsPage } from "@/pages/console/platform/AgentsPage";
import { PlatformConversationsPage } from "@/pages/console/platform/ConversationsPage";
import { PlatformUsagePage } from "@/pages/console/platform/UsagePage";
import { PlatformComputersPage } from "@/pages/console/platform/ComputersPage";
import { PlatformWebhooksPage } from "@/pages/console/platform/WebhooksPage";
import { PlatformBillingPage } from "@/pages/console/platform/BillingPage";
import { usePlatformAuth } from "@/lib/platform-auth-client";

function RequireAuth({ children }: { children: React.ReactNode }) {
  const auth = usePlatformAuth();
  const location = useLocation();

  if (!auth.isLoaded) {
    return (
      <div className="flex h-screen w-full items-center justify-center bg-[var(--bg-primary)]">
        <div className="size-8 animate-spin rounded-full border-2 border-[var(--accent-primary)] border-t-transparent" />
      </div>
    );
  }

  if (!auth.isSignedIn) {
    const returnUrl = encodeURIComponent(location.pathname + location.search);
    return <Navigate to={`/sign-in?redirect_url=${returnUrl}`} replace />;
  }

  return <>{children}</>;
}

function ConsoleRoute({ children }: { children: React.ReactNode }) {
  return (
    <RequireAuth>
      <ConsoleLayout>{children}</ConsoleLayout>
    </RequireAuth>
  );
}

/** Phase 1 designed stubs — one per future console page. */
const consoleStubRoutes: Array<{ path: string; element: React.ReactNode }> = [
  // /manage/rate-limits content (caller snapshot + admin org limits) shipped
  // with the Analytics surface at /analytics/rate-limits (Phase 4).
  { path: "/manage/rate-limits/*", element: <Navigate to="/analytics/rate-limits" replace /> },
];

function HomeRoute() {
  const auth = usePlatformAuth();

  if (!auth.isLoaded) {
    return (
      <div className="flex h-screen w-full items-center justify-center bg-[var(--bg-primary)]">
        <div className="size-8 animate-spin rounded-full border-2 border-[var(--accent-primary)] border-t-transparent" />
      </div>
    );
  }

  if (auth.isSignedIn) {
    return (
      <ConsoleRoute>
        <DashboardPage />
      </ConsoleRoute>
    );
  }

  return <PortalLandingPage />;
}

function BillingRoute() {
  const auth = usePlatformAuth();

  if (!auth.isLoaded) {
    return (
      <div className="flex h-screen w-full items-center justify-center bg-[var(--bg-primary)]">
        <div className="size-8 animate-spin rounded-full border-2 border-[var(--accent-primary)] border-t-transparent" />
      </div>
    );
  }

  if (auth.isSignedIn) {
    return (
      <ConsoleLayout>
        <BillingPage />
      </ConsoleLayout>
    );
  }

  return (
    <PublicPageShell>
      <BillingPage />
    </PublicPageShell>
  );
}

export default function App() {
  return (
    <Routes>
      <Route path="/" element={<HomeRoute />} />
      <Route path="/models/*" element={<ModelsPage />} />
      <Route path="/plans/*" element={<PlansPage />} />
      <Route path="/billing/*" element={<BillingRoute />} />
      <Route
        path="/admin/customers"
        element={
          <ConsoleRoute>
            <AdminCustomersPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/organizations/*"
        element={
          <ConsoleRoute>
            <OrganizationsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/compute/*"
        element={
          <ConsoleRoute>
            <ComputePage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/agents"
        element={
          <ConsoleRoute>
            <AgentsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/agents/new"
        element={
          <ConsoleRoute>
            <AgentFormPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/agents/:id"
        element={
          <ConsoleRoute>
            <AgentDetailPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/agents/:id/edit"
        element={
          <ConsoleRoute>
            <AgentFormPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/sessions"
        element={
          <ConsoleRoute>
            <SessionsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/sessions/new"
        element={
          <ConsoleRoute>
            <SessionNewPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/sessions/:id"
        element={
          <ConsoleRoute>
            <SessionDetailPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/deployments"
        element={
          <ConsoleRoute>
            <DeploymentsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/computers"
        element={
          <ConsoleRoute>
            <ComputersPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/vaults"
        element={
          <ConsoleRoute>
            <VaultsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/vaults/:id"
        element={
          <ConsoleRoute>
            <VaultDetailPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/memory"
        element={
          <ConsoleRoute>
            <MemoryPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/memory/:id"
        element={
          <ConsoleRoute>
            <MemoryStorePage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/devices/*"
        element={
          <ConsoleRoute>
            <DevicesPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/runs/*"
        element={
          <ConsoleRoute>
            <RunsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/schedules/*"
        element={
          <ConsoleRoute>
            <SchedulesPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/approvals/*"
        element={
          <ConsoleRoute>
            <ApprovalsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/cloud-accounts/*"
        element={
          <ConsoleRoute>
            <CloudAccountsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/api-keys/*"
        element={
          <ConsoleRoute>
            <ApiKeysPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/docs/*"
        element={
          <ConsoleRoute>
            <DocsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/settings/*"
        element={
          <ConsoleRoute>
            <SettingsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/fabric/*"
        element={
          <ConsoleRoute>
            <FabricPage />
          </ConsoleRoute>
        }
      />
      {/* Phase 2 Build group — real pages on the console-ui kit. */}
      <Route
        path="/playground/*"
        element={
          <ConsoleRoute>
            <PlaygroundPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/files/*"
        element={
          <ConsoleRoute>
            <FilesPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/skills/*"
        element={
          <ConsoleRoute>
            <SkillsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/batches/*"
        element={
          <ConsoleRoute>
            <BatchesPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/builder/*"
        element={
          <ConsoleRoute>
            <BuilderPage />
          </ConsoleRoute>
        }
      />
      {/* Phase 4 Analytics group — real pages over the live gateway routes. */}
      <Route
        path="/analytics/usage/*"
        element={
          <ConsoleRoute>
            <UsagePage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/analytics/logs/*"
        element={
          <ConsoleRoute>
            <LogsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/analytics/caching/*"
        element={
          <ConsoleRoute>
            <CachingPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/analytics/rate-limits/*"
        element={
          <ConsoleRoute>
            <RateLimitsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/analytics/cost/*"
        element={
          <ConsoleRoute>
            <CostPage />
          </ConsoleRoute>
        }
      />
      {/* Phase 5 Manage group — real pages over the admin gateway routes. */}
      <Route
        path="/manage/members/*"
        element={
          <ConsoleRoute>
            <MembersPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/manage/service-accounts/*"
        element={
          <ConsoleRoute>
            <ServiceAccountsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/manage/spend-limits/*"
        element={
          <ConsoleRoute>
            <SpendLimitsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/manage/security/*"
        element={
          <ConsoleRoute>
            <SecurityPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/manage/webhooks/*"
        element={
          <ConsoleRoute>
            <WebhooksPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/manage/tags/*"
        element={
          <ConsoleRoute>
            <TagsPage />
          </ConsoleRoute>
        }
      />
      {/* Phase 6 — Gizzi Code usage over the admin analytics route. */}
      <Route
        path="/gizzi/usage/*"
        element={
          <ConsoleRoute>
            <GizziUsagePage />
          </ConsoleRoute>
        }
      />
      {/* Gateway control plane — provider routing policy + BYO route credentials. */}
      <Route
        path="/gateway/routing-policy/*"
        element={
          <ConsoleRoute>
            <RoutingPolicyPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/gateway/credentials/*"
        element={
          <ConsoleRoute>
            <RouteCredentialsPage />
          </ConsoleRoute>
        }
      />
      {/* Platform API (developer /v1): projects, agents, conversations, computers, usage, webhooks. */}
      <Route
        path="/platform/projects"
        element={
          <ConsoleRoute>
            <PlatformProjectsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/platform/agents"
        element={
          <ConsoleRoute>
            <PlatformAgentsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/platform/conversations"
        element={
          <ConsoleRoute>
            <PlatformConversationsPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/platform/computers"
        element={
          <ConsoleRoute>
            <PlatformComputersPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/platform/usage"
        element={
          <ConsoleRoute>
            <PlatformUsagePage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/platform/webhooks"
        element={
          <ConsoleRoute>
            <PlatformWebhooksPage />
          </ConsoleRoute>
        }
      />
      <Route
        path="/platform/billing"
        element={
          <ConsoleRoute>
            <PlatformBillingPage />
          </ConsoleRoute>
        }
      />
      <Route path="/platform" element={<Navigate to="/platform/projects" replace />} />
      {consoleStubRoutes.map((route) => (
        <Route
          key={route.path}
          path={route.path}
          element={<ConsoleRoute>{route.element}</ConsoleRoute>}
        />
      ))}
      {/* Fallback Clerk path-routed pages. Public marketing pages open the auth modal
          inline, but /sign-in and /sign-up remain available for direct navigation. */}
      <Route path="/sign-in/*" element={<SignInPage />} />
      <Route path="/sign-up/*" element={<SignUpPage />} />
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  );
}
