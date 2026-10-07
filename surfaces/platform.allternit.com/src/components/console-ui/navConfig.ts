/**
 * Shared console navigation configuration.
 *
 * This is the single source of truth for the sidebar information architecture.
 * ConsoleLayout renders it, CommandPalette searches it — keeping both pointed
 * at this module prevents the palette index and the sidebar from drifting.
 */
import {
  LayoutDashboardIcon,
  Key01Icon,
  SourceCodeIcon,
  PlayIcon,
  File01Icon,
  SparklesIcon,
  Briefcase01Icon,
  BotIcon,
  ComputerIcon,
  DatabaseIcon,
  LockIcon,
  AiBrain01Icon,
  MonitorIcon,
  PieChartIcon,
  Analytics01Icon,
  DocumentValidationIcon,
  GaugeIcon,
  Timer01Icon,
  Dollar01Icon,
  CoinsDollarIcon,
  Setting07Icon,
  UserGroupIcon,
  ShieldUserIcon,
  LockPasswordIcon,
  WebhookIcon,
  Tag01Icon,
  CloudIcon,
  TeamWorkIcon,
  CpuIcon,
  DeviceAccessIcon,
  LayersIcon,
  RocketIcon,
  Calendar02Icon,
  ShieldCheckIcon,
  Wallet01Icon,
  BookOpen02Icon,
  MagicWandIcon,
  ApiGatewayIcon,
  Route01Icon,
  Key02Icon,
  CubeIcon,
  BubbleChatIcon,
} from "@hugeicons/core-free-icons";

export type ConsoleNavIcon = typeof LayoutDashboardIcon;

export interface ConsoleNavItem {
  to: string;
  label: string;
  icon: ConsoleNavIcon;
}

export interface ConsoleNavGroup {
  label: string;
  icon: ConsoleNavIcon;
  defaultOpen?: boolean;
  /** Optional pill rendered next to the group label (e.g. an amber "soon"). */
  badge?: string;
  items: ConsoleNavItem[];
}

export interface ConsoleNavConfig {
  top: ConsoleNavItem[];
  groups: ConsoleNavGroup[];
}

export const consoleNav: ConsoleNavConfig = {
  top: [
    { to: "/", label: "Dashboard", icon: LayoutDashboardIcon },
    { to: "/api-keys", label: "API keys", icon: Key01Icon },
  ],
  groups: [
    {
      label: "Build",
      icon: SourceCodeIcon,
      defaultOpen: true,
      items: [
        { to: "/playground", label: "Playground", icon: PlayIcon },
        { to: "/files", label: "Files", icon: File01Icon },
        { to: "/skills", label: "Skills", icon: SparklesIcon },
        { to: "/batches", label: "Batches", icon: Briefcase01Icon },
      ],
    },
    {
      label: "Platform API",
      icon: CubeIcon,
      defaultOpen: true,
      items: [
        { to: "/platform/projects", label: "Projects", icon: CubeIcon },
        { to: "/platform/agents", label: "Agents", icon: BotIcon },
        { to: "/platform/conversations", label: "Conversations", icon: BubbleChatIcon },
        { to: "/platform/usage", label: "Usage", icon: Analytics01Icon },
        { to: "/platform/webhooks", label: "Webhooks", icon: WebhookIcon },
      ],
    },
    {
      label: "Managed Agents",
      icon: BotIcon,
      defaultOpen: true,
      items: [
        { to: "/agents", label: "Overview", icon: BotIcon },
        { to: "/builder", label: "Builder", icon: MagicWandIcon },
        { to: "/sessions", label: "Sessions", icon: ComputerIcon },
        { to: "/deployments", label: "Deployments", icon: RocketIcon },
        { to: "/computers", label: "Computers", icon: MonitorIcon },
        { to: "/vaults", label: "Vaults", icon: DatabaseIcon },
        { to: "/memory", label: "Memory", icon: AiBrain01Icon },
      ],
    },
    {
      label: "Analytics",
      icon: PieChartIcon,
      items: [
        { to: "/analytics/usage", label: "Usage", icon: Analytics01Icon },
        { to: "/analytics/logs", label: "Logs", icon: DocumentValidationIcon },
        { to: "/analytics/caching", label: "Caching", icon: GaugeIcon },
        { to: "/analytics/rate-limits", label: "Rate limits", icon: Timer01Icon },
        { to: "/analytics/cost", label: "Cost", icon: Dollar01Icon },
      ],
    },
    {
      label: "Gizzi Code",
      icon: AiBrain01Icon,
      badge: "soon",
      items: [{ to: "/gizzi/usage", label: "Usage", icon: CoinsDollarIcon }],
    },
    {
      label: "Gateway",
      icon: ApiGatewayIcon,
      items: [
        { to: "/gateway/routing-policy", label: "Routing policy", icon: Route01Icon },
        { to: "/gateway/credentials", label: "Route credentials", icon: Key02Icon },
      ],
    },
    {
      label: "Manage",
      icon: Setting07Icon,
      items: [
        { to: "/manage/rate-limits", label: "Rate limits", icon: Timer01Icon },
        { to: "/manage/spend-limits", label: "Spend limits", icon: CoinsDollarIcon },
        { to: "/manage/members", label: "Members", icon: UserGroupIcon },
        { to: "/manage/service-accounts", label: "Service accounts", icon: ShieldUserIcon },
        { to: "/manage/security", label: "Security", icon: LockPasswordIcon },
        { to: "/manage/webhooks", label: "Webhooks", icon: WebhookIcon },
        { to: "/manage/tags", label: "Tags", icon: Tag01Icon },
      ],
    },
    {
      label: "Allternit Cloud",
      icon: CloudIcon,
      items: [
        { to: "/organizations", label: "Organizations", icon: TeamWorkIcon },
        { to: "/compute", label: "Compute", icon: CpuIcon },
        { to: "/devices", label: "Devices", icon: DeviceAccessIcon },
        { to: "/fabric", label: "Fabric", icon: LayersIcon },
        { to: "/runs", label: "Runs", icon: RocketIcon },
        { to: "/schedules", label: "Schedules", icon: Calendar02Icon },
        { to: "/approvals", label: "Approvals", icon: ShieldCheckIcon },
        { to: "/cloud-accounts", label: "Cloud accounts", icon: CloudIcon },
        { to: "/billing", label: "Billing", icon: Wallet01Icon },
        { to: "/models", label: "Models", icon: SparklesIcon },
        { to: "/plans", label: "Plans", icon: LayersIcon },
      ],
    },
    {
      label: "Resources",
      icon: BookOpen02Icon,
      items: [
        { to: "/docs", label: "Docs", icon: BookOpen02Icon },
        { to: "/settings", label: "Settings", icon: Setting07Icon },
      ],
    },
  ],
};

/** Every navigable item, flattened with its group label for palette display. */
export interface ConsoleNavEntry {
  to: string;
  label: string;
  group: string | null;
  icon: ConsoleNavIcon;
}

export const consoleNavEntries: ConsoleNavEntry[] = [
  ...consoleNav.top.map((item) => ({ ...item, group: null })),
  ...consoleNav.groups.flatMap((group) =>
    group.items.map((item) => ({ ...item, group: group.label }))
  ),
];

export function consoleNavLabelForPath(pathname: string): string {
  if (pathname === "/") return "Dashboard";
  const match = consoleNavEntries.find(
    (item) => pathname === item.to || pathname.startsWith(`${item.to}/`)
  );
  return match?.label ?? "Console";
}
