import React from "react";
import { Link } from "react-router-dom";
import {
  ArrowRight,
  Cloud,
  Code,
  Cpu,
  CreditCard,
  Key,
  Lightning,
  LockKey,
  PuzzlePiece,
  Rocket,
  ShieldCheck,
  Terminal,
  Users,
} from "@phosphor-icons/react";
import { PlanPicker } from "@/components/PlanPicker";
import { PublicPageShell } from "@/components/PublicPageShell";
import { ModelCapabilitiesSection } from "@/components/landing/ModelCapabilitiesSection";
import { ResourcesSection } from "@/components/landing/ResourcesSection";
import { SecuritySection } from "@/components/landing/SecuritySection";
import { UseCasesSection } from "@/components/landing/UseCasesSection";
import { FaqSection } from "@/components/landing/FaqSection";
import { usePlatformAuth, useClerk } from "@/lib/platform-auth-client";

const BUILD_CARDS = [
  {
    icon: <Code size={24} weight="duotone" />,
    title: "Direct API",
    description:
      "One OpenAI-compatible endpoint. Allternit routes each request to the optimal supplier for cost, speed, or capacity — you never manage keys per vendor.",
  },
  {
    icon: <Cpu size={24} weight="duotone" />,
    title: "Managed compute",
    description:
      "Provision hosted runtimes, pair your own devices, or run models locally. Choose the compute that fits the job.",
  },
  {
    icon: <PuzzlePiece size={24} weight="duotone" />,
    title: "Built-in tools",
    description:
      "Give models access to search, code execution, and custom capabilities through the same API and credit balance.",
  },
];

const JOURNEY_STEPS = [
  {
    step: "01",
    icon: <Key size={18} />,
    title: "Get an API key",
    description: "Create an organization and generate a key from the console.",
  },
  {
    step: "02",
    icon: <Terminal size={18} />,
    title: "Pair a runtime",
    description: "Connect a local device or provision a hosted runtime in one command.",
  },
  {
    step: "03",
    icon: <Lightning size={18} />,
    title: "Run jobs",
    description: "Schedule runs, route them to local or cloud compute, and monitor usage.",
  },
  {
    step: "04",
    icon: <Users size={18} />,
    title: "Scale the team",
    description: "Add members, set roles, and centralize billing as you grow.",
  },
];

const FEATURES = [
  {
    icon: <Code size={22} weight="duotone" />,
    title: "Unified API",
    description: "One endpoint for local and cloud inference, tools, and job orchestration.",
  },
  {
    icon: <Cpu size={22} weight="duotone" />,
    title: "Hosted runtimes",
    description: "Provision managed compute for long-running or cloud-only workloads.",
  },
  {
    icon: <Cloud size={22} weight="duotone" />,
    title: "Local-first option",
    description: "Run open-weights models on your own hardware with no compute cost.",
  },
  {
    icon: <CreditCard size={22} weight="duotone" />,
    title: "Credits, not invoices",
    description: "Pre-paid credits roll over monthly. Upgrade, downgrade, or cancel anytime.",
  },
  {
    icon: <Users size={22} weight="duotone" />,
    title: "Organizations",
    description: "Shared billing, member roles, and resource access for teams.",
  },
  {
    icon: <ShieldCheck size={22} weight="duotone" />,
    title: "Clerk auth",
    description: "Production-grade sign-in, session management, and organization switching.",
  },
];

const CODE_SNIPPET = `curl https://api.allternit.com/v1/chat/completions \\
  -H "Authorization: Bearer $ALLTERNIT_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "llama-3.3-70b-turbo",
    "messages": [{"role": "user", "content": "Hello"}]
  }'`;

export function PortalLandingPage() {
  const auth = usePlatformAuth();
  const clerk = useClerk();
  const isSignedIn = auth.isLoaded && auth.isSignedIn;

  const handleSubscribe = () => {
    if (isSignedIn) return;
    if (clerk?.openSignIn) {
      clerk.openSignIn({ redirectUrl: "/billing" });
    } else {
      window.location.href = `/sign-in?redirect_url=${encodeURIComponent("/billing")}`;
    }
  };

  return (
    <PublicPageShell>
      {/* Hero */}
      <section className="relative overflow-hidden border-b border-[var(--border-subtle)] bg-[var(--surface-canvas)] px-6 pb-16 pt-12 md:pb-24 md:pt-20">
        <div
          className="pointer-events-none absolute inset-0 -z-10"
          style={{
            backgroundImage:
              "radial-gradient(circle at 70% 20%, rgba(217,119,87,0.14), transparent 35%), radial-gradient(circle at 30% 80%, rgba(217,119,87,0.06), transparent 30%)",
          }}
        />
        <div
          className="pointer-events-none absolute inset-0 -z-10 text-[var(--text-primary)]"
          style={{
            backgroundImage: "radial-gradient(circle, currentColor 1px, transparent 1px)",
            backgroundSize: "32px 32px",
            opacity: 0.04,
            maskImage: "linear-gradient(to bottom, black 40%, transparent 100%)",
            WebkitMaskImage: "linear-gradient(to bottom, black 40%, transparent 100%)",
          }}
        />
        <div className="mx-auto grid max-w-6xl items-center gap-10 lg:grid-cols-2 lg:gap-14">
          <div>
            <div className="mb-5 inline-flex items-center gap-2 rounded-full border border-[#D97757]/30 bg-[#D97757]/10 px-3 py-1 text-[11px] font-semibold tracking-[0.08em] text-[#D97757]">
              <Rocket size={12} weight="fill" /> Allternit Cloud is in beta
            </div>
            <h1 className="text-[40px] font-bold leading-[1.05] tracking-tight text-[var(--text-primary)] md:text-[56px] lg:text-[64px]">
              Build with local and cloud models on one platform
            </h1>
            <p className="mt-5 max-w-xl text-[16px] leading-relaxed text-[var(--text-secondary)] md:text-[18px]">
              A single account that routes every workload to the best place to run it — your local
              hardware, Allternit-managed compute, or a cloud model supplier — all on one set of
              credits with one OpenAI-compatible API.
            </p>
            <div className="mt-8 flex flex-col items-start gap-3 sm:flex-row">
              {isSignedIn ? (
                <Link
                  to="/"
                  className="inline-flex items-center gap-2 rounded-lg bg-[#D97757] px-5 py-2.5 text-[14px] font-semibold text-[#FDF8F3] transition-all hover:brightness-110"
                >
                  Open console <ArrowRight size={16} />
                </Link>
              ) : (
                <>
                  <Link
                    to="/sign-up"
                    className="inline-flex items-center gap-2 rounded-lg bg-[#D97757] px-5 py-2.5 text-[14px] font-semibold text-[#FDF8F3] transition-all hover:brightness-110"
                  >
                    Start building free <ArrowRight size={16} />
                  </Link>
                  <button
                    onClick={() => {
                      if (clerk?.openSignIn) clerk.openSignIn({ redirectUrl: "/" });
                      else window.location.href = `/sign-in?redirect_url=${encodeURIComponent("/")}`;
                    }}
                    className="inline-flex items-center gap-2 rounded-lg border border-[var(--border-default)] px-5 py-2.5 text-[14px] font-semibold text-[var(--text-primary)] transition-colors hover:bg-[var(--surface-hover)]"
                  >
                    Sign in
                  </button>
                </>
              )}
            </div>
            <p className="mt-4 text-[12px] text-[var(--text-tertiary)]">
              Free tier includes local models and standard rate limits.
            </p>
            <Link
              to="/cloud"
              className="mt-2 inline-flex items-center gap-1 text-[13px] font-medium text-[#D97757] hover:underline"
            >
              Calling models from code? See the Cloud API models and prices <ArrowRight size={13} />
            </Link>
          </div>

          <div className="relative">
            <div className="absolute -inset-1 rounded-2xl bg-gradient-to-br from-[#D97757]/20 via-transparent to-[#D97757]/10 blur-xl" />
            <div className="relative overflow-hidden rounded-2xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)] shadow-2xl">
              <div className="flex items-center gap-2 border-b border-[var(--border-subtle)] px-4 py-3">
                <div className="size-2.5 rounded-full bg-[#D97757]" />
                <div className="size-2.5 rounded-full bg-[var(--border-default)]" />
                <div className="size-2.5 rounded-full bg-[var(--border-default)]" />
                <span className="ml-2 text-[11px] text-[var(--text-tertiary)]">allternit-cloud.sh</span>
              </div>
              <pre className="overflow-x-auto p-5 text-[12px] leading-relaxed text-[var(--text-primary)] md:text-[13px]">
                <code>{CODE_SNIPPET}</code>
              </pre>
            </div>
          </div>
        </div>
      </section>

      {/* Build your way */}
      <section className="px-6 py-16 md:py-24">
        <div className="mx-auto max-w-6xl">
          <div className="max-w-2xl">
            <h2 className="text-[28px] font-bold tracking-tight text-[var(--text-primary)] md:text-[36px]">
              Build your way
            </h2>
            <p className="mt-3 text-[15px] leading-relaxed text-[var(--text-secondary)] md:text-[16px]">
              Pick the surface that matches your stack. Everything runs on the same account and credit balance.
            </p>
          </div>
          <div className="mt-10 grid grid-cols-1 gap-5 md:grid-cols-3">
            {BUILD_CARDS.map((card) => (
              <div
                key={card.title}
                className="group rounded-2xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-6 transition-all hover:border-[#D97757]/30 hover:bg-[var(--bg-secondary)]"
              >
                <div className="mb-4 inline-flex rounded-xl bg-[#D97757]/10 p-3 text-[#D97757] transition-transform group-hover:scale-105">
                  {card.icon}
                </div>
                <h3 className="text-[17px] font-semibold text-[var(--text-primary)]">{card.title}</h3>
                <p className="mt-2 text-[14px] leading-relaxed text-[var(--text-secondary)]">{card.description}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* Developer journey */}
      <section className="border-y border-[var(--border-subtle)] bg-[var(--bg-secondary)]/30 px-6 py-16 md:py-24">
        <div className="mx-auto max-w-6xl">
          <div className="max-w-2xl">
            <h2 className="text-[28px] font-bold tracking-tight text-[var(--text-primary)] md:text-[36px]">
              From idea to production
            </h2>
            <p className="mt-3 text-[15px] leading-relaxed text-[var(--text-secondary)] md:text-[16px]">
              A simple path from first API call to a team-managed deployment.
            </p>
          </div>
          <div className="mt-10 grid grid-cols-1 gap-6 sm:grid-cols-2 lg:grid-cols-4">
            {JOURNEY_STEPS.map((item) => (
              <div key={item.step} className="relative">
                <span className="text-[40px] font-bold leading-none text-[var(--border-subtle)]">{item.step}</span>
                <div className="mt-4 flex items-center gap-2 text-[13px] font-semibold uppercase tracking-[0.1em] text-[#D97757]">
                  <span className="inline-flex rounded-md bg-[#D97757]/10 p-1">{item.icon}</span>
                  {item.title}
                </div>
                <p className="mt-2 text-[14px] leading-relaxed text-[var(--text-secondary)]">{item.description}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* Model capabilities */}
      <ModelCapabilitiesSection />

      {/* Resources */}
      <ResourcesSection />

      {/* Security */}
      <SecuritySection />

      {/* Use cases */}
      <UseCasesSection />

      {/* FAQ */}
      <FaqSection />

      {/* What you get */}
      <section className="px-6 py-16 md:py-24">
        <div className="mx-auto max-w-6xl">
          <div className="max-w-2xl">
            <h2 className="text-[28px] font-bold tracking-tight text-[var(--text-primary)] md:text-[36px]">
              What you get
            </h2>
            <p className="mt-3 text-[15px] leading-relaxed text-[var(--text-secondary)] md:text-[16px]">
              The console gives you control over models, compute, teams, and spend — not dashboards you cannot act on.
            </p>
          </div>
          <div className="mt-10 grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
            {FEATURES.map((feature) => (
              <div
                key={feature.title}
                className="flex gap-4 rounded-2xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)]/50 p-5"
              >
                <div className="shrink-0 text-[#D97757]">{feature.icon}</div>
                <div>
                  <h3 className="text-[15px] font-semibold text-[var(--text-primary)]">{feature.title}</h3>
                  <p className="mt-1 text-[13px] leading-relaxed text-[var(--text-secondary)]">
                    {feature.description}
                  </p>
                </div>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* Plans */}
      <section className="border-y border-[var(--border-subtle)] bg-[var(--surface-canvas)] px-6 py-16 md:py-24">
        <div className="mx-auto max-w-6xl">
          <div className="max-w-2xl">
            <div className="mb-4 inline-flex items-center gap-2 rounded-full border border-[#D97757]/30 bg-[#D97757]/10 px-3 py-1 text-[11px] font-semibold tracking-[0.08em] text-[#D97757]">
              <LockKey size={12} /> Beta pricing
            </div>
            <h2 className="text-[28px] font-bold tracking-tight text-[var(--text-primary)] md:text-[36px]">
              Simple, credit-based plans
            </h2>
            <p className="mt-3 text-[15px] leading-relaxed text-[var(--text-secondary)] md:text-[16px]">
              Start free. Upgrade when you need cloud models, hosted tools, or higher rate limits.
              BETA subscriptions are UI-only; cloud models are provided by upstream partners.
            </p>
          </div>
          <div className="mt-10">
            <PlanPicker
              currentPlanName={null}
              title="Choose a plan"
              onSubscribe={isSignedIn ? undefined : handleSubscribe}
            />
          </div>
        </div>
      </section>

      {/* Bottom CTA */}
      <section className="px-6 py-16 md:py-24">
        <div className="mx-auto max-w-4xl rounded-3xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-8 text-center md:p-14">
          <h2 className="text-[26px] font-bold tracking-tight text-[var(--text-primary)] md:text-[36px]">
            Start building with Allternit Cloud
          </h2>
          <p className="mx-auto mt-3 max-w-xl text-[15px] leading-relaxed text-[var(--text-secondary)] md:text-[16px]">
            Create a free account, generate an API key, and run your first local or cloud model in minutes.
          </p>
          <div className="mt-8 flex flex-col items-center justify-center gap-3 sm:flex-row">
            {isSignedIn ? (
              <Link
                to="/"
                className="inline-flex items-center gap-2 rounded-lg bg-[#D97757] px-6 py-3 text-[14px] font-semibold text-[#FDF8F3] transition-all hover:brightness-110"
              >
                Open console <ArrowRight size={16} />
              </Link>
            ) : (
              <>
                <Link
                  to="/sign-up"
                  className="inline-flex items-center gap-2 rounded-lg bg-[#D97757] px-6 py-3 text-[14px] font-semibold text-[#FDF8F3] transition-all hover:brightness-110"
                >
                  Create free account <ArrowRight size={16} />
                </Link>
                <button
                  onClick={() => {
                    if (clerk?.openSignIn) clerk.openSignIn({ redirectUrl: "/" });
                    else window.location.href = `/sign-in?redirect_url=${encodeURIComponent("/")}`;
                  }}
                  className="inline-flex items-center gap-2 rounded-lg border border-[var(--border-subtle)] px-6 py-3 text-[14px] font-semibold text-[var(--text-primary)] transition-colors hover:bg-[var(--bg-primary)]"
                >
                  Sign in
                </button>
              </>
            )}
          </div>
        </div>
      </section>
    </PublicPageShell>
  );
}
