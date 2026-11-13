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
  MagicWandIcon,
  ApiGatewayIcon,
  Route01Icon,
  Key02Icon,
  CubeIcon,
  BubbleChatIcon,
  SmartPhone01Icon,
  Audit01Icon,
  Building03Icon,
  CheckListIcon,
} from "@hugeicons/core-free-icons";
import { fabricEnabled, isCloudGateway, isHiddenOnCloud, type GatedFeature } from "@/lib/availability";

export type ConsoleNavIcon = typeof LayoutDashboardIcon;

export interface ConsoleNavItem {
  to: string;
  label: string;
  icon: ConsoleNavIcon;
  /** Optional pill rendered next to the item label (e.g. an amber "soon"). */
  badge?: string;
  /** Hidden on Allternit Cloud (see src/lib/availability.ts). */
  cloudHidden?: GatedFeature;
  /** Only shown when the Fabric backend (VITE_ALLTERNITOS_URL) is configured. */
  requiresFabric?: boolean;
}

export interface ConsoleNavGroup {
  label: string;
  icon: ConsoleNavIcon;
  /** Optional pill rendered next to the group label (e.g. an amber "soon"). */
  badge?: string;
  items: ConsoleNavItem[];
}

export interface ConsoleNavConfig {
  top: ConsoleNavItem[];
  groups: ConsoleNavGroup[];
}

/**
 * Rail layout (approved 2026-11-13): four top items, then groups that open one
 * at a time. Labels can repeat across groups (Computers, Usage) because the
 * rail, palette and breadcrumb always show them with their group.
 * `/docs` is not here: it lives in the rail footer's link line.
 */
const baseConsoleNav: ConsoleNavConfig = {
  top: [
    { to: "/", label: "Dashboard", icon: LayoutDashboardIcon },
    { to: "/playground", label: "Playground", icon: PlayIcon },
    { to: "/api-keys", label: "API keys", icon: Key01Icon },
    { to: "/models", label: "Models", icon: SparklesIcon },
  ],
  groups: [
    {
      label: "Platform API",
      icon: CubeIcon,
      items: [
        { to: "/platform/projects", label: "Projects", icon: CubeIcon },
        { to: "/platform/agents", label: "Agents", icon: BotIcon },
        { to: "/platform/conversations", label: "Conversations", icon: BubbleChatIcon },
        { to: "/platform/decisions", label: "Decisions", icon: CheckListIcon },
        { to: "/approvals", label: "Approvals", icon: ShieldCheckIcon },
        { to: "/platform/computers", label: "Computers", icon: ComputerIcon },
        { to: "/platform/phones", label: "Phones", icon: SmartPhone01Icon },
        { to: "/platform/usage", label: "Usage", icon: Analytics01Icon },
        { to: "/platform/billing", label: "Billing", icon: Dollar01Icon },
        { to: "/platform/webhooks", label: "Webhooks", icon: WebhookIcon },
      ],
    },
    {
      label: "Managed Agents",
      icon: BotIcon,
      items: [
        { to: "/agents", label: "Overview", icon: BotIcon },
        { to: "/builder", label: "Builder", icon: MagicWandIcon },
        { to: "/sessions", label: "Sessions", icon: SourceCodeIcon },
        { to: "/deployments", label: "Deployments", icon: RocketIcon },
        { to: "/computers", label: "Computers", icon: MonitorIcon },
        { to: "/vaults", label: "Vaults", icon: DatabaseIcon },
        { to: "/memory", label: "Memory", icon: AiBrain01Icon },
        { to: "/skills", label: "Skills", icon: SparklesIcon },
        { to: "/files", label: "Files", icon: File01Icon, cloudHidden: "files" },
        { to: "/batches", label: "Batches", icon: Briefcase01Icon, cloudHidden: "batches" },
      ],
    },
    {
      label: "Analytics",
      icon: PieChartIcon,
      items: [
        { to: "/analytics/usage", label: "Usage", icon: Analytics01Icon },
        { to: "/analytics/logs", label: "Logs", icon: DocumentValidationIcon },
        { to: "/analytics/cost", label: "Cost", icon: Dollar01Icon },
        { to: "/analytics/rate-limits", label: "Rate limits", icon: Timer01Icon },
        { to: "/analytics/caching", label: "Caching", icon: GaugeIcon, cloudHidden: "caching" },
        { to: "/gizzi/usage", label: "Gizzi Code", icon: CoinsDollarIcon, badge: "soon" },
      ],
    },
    {
      label: "Organization",
      icon: Building03Icon,
      items: [
        { to: "/billing", label: "Credits & billing", icon: Wallet01Icon },
        { to: "/plans", label: "Plans", icon: LayersIcon },
        { to: "/manage/members", label: "Members", icon: UserGroupIcon },
        { to: "/manage/audit-log", label: "Audit log", icon: Audit01Icon },
        { to: "/manage/spend-limits", label: "Spend limits", icon: CoinsDollarIcon, cloudHidden: "spendLimits" },
        { to: "/manage/service-accounts", label: "Service accounts", icon: ShieldUserIcon, cloudHidden: "serviceAccounts" },
        { to: "/manage/security", label: "Security", icon: LockPasswordIcon, cloudHidden: "security" },
        { to: "/manage/webhooks", label: "Organization webhooks", icon: WebhookIcon, cloudHidden: "orgWebhooks" },
        { to: "/manage/tags", label: "Tags", icon: Tag01Icon, cloudHidden: "tags" },
        { to: "/settings", label: "Settings", icon: Setting07Icon },
      ],
    },
    {
      label: "Cloud",
      icon: CloudIcon,
      items: [
        { to: "/compute", label: "Compute", icon: CpuIcon },
        { to: "/devices", label: "Devices", icon: DeviceAccessIcon },
        { to: "/fabric", label: "Fabric", icon: LayersIcon, requiresFabric: true },
        { to: "/runs", label: "Runs", icon: RocketIcon },
        { to: "/schedules", label: "Schedules", icon: Calendar02Icon },
        { to: "/cloud-accounts", label: "Cloud accounts", icon: CloudIcon },
        { to: "/organizations", label: "Organizations", icon: TeamWorkIcon },
      ],
    },
    {
      label: "Gateway",
      icon: ApiGatewayIcon,
      items: [
        { to: "/gateway/routing-policy", label: "Routing policy", icon: Route01Icon, cloudHidden: "routingPolicy" },
        { to: "/gateway/credentials", label: "Route credentials", icon: Key02Icon, cloudHidden: "routeCredentials" },
      ],
    },
  ],
};

export interface ConsoleNavOptions {
  cloud: boolean;
  fabric: boolean;
}

/** Build the nav for a given environment; empty groups are dropped. */
export function buildConsoleNav(opts: ConsoleNavOptions): ConsoleNavConfig {
  const keep = (item: ConsoleNavItem) =>
    !(item.cloudHidden && isHiddenOnCloud(item.cloudHidden, opts.cloud)) &&
    !(item.requiresFabric && !opts.fabric);
  return {
    top: baseConsoleNav.top.filter(keep),
    groups: baseConsoleNav.groups
      .map((group) => ({ ...group, items: group.items.filter(keep) }))
      .filter((group) => group.items.length > 0),
  };
}

export const consoleNav: ConsoleNavConfig = buildConsoleNav({
  cloud: isCloudGateway(),
  fabric: fabricEnabled(),
});

/** Every navigable item, flattened with its group label for palette display. */
export interface ConsoleNavEntry {
  to: string;
  label: string;
  group: string | null;
  icon: ConsoleNavIcon;
}

export function buildConsoleNavEntries(nav: ConsoleNavConfig): ConsoleNavEntry[] {
  return [
    ...nav.top.map((item) => ({ ...item, group: null })),
    ...nav.groups.flatMap((group) =>
      group.items.map((item) => ({ ...item, group: group.label }))
    ),
  ];
}

export const consoleNavEntries: ConsoleNavEntry[] = buildConsoleNavEntries(consoleNav);

/**
 * The nav entry a path belongs to. The longest matching URL wins, so nested
 * pages (e.g. /agents/new) resolve to their own item and never to a shorter
 * sibling. "/" only matches itself.
 */
export function consoleNavEntryForPath(
  pathname: string,
  entries: ConsoleNavEntry[] = consoleNavEntries
): ConsoleNavEntry | undefined {
  let best: ConsoleNavEntry | undefined;
  for (const entry of entries) {
    const hit =
      entry.to === "/"
        ? pathname === "/"
        : pathname === entry.to || pathname.startsWith(`${entry.to}/`);
    if (hit && (!best || entry.to.length > best.to.length)) best = entry;
  }
  return best;
}

export function consoleNavLabelForPath(pathname: string): string {
  return consoleNavEntryForPath(pathname)?.label ?? "Console";
}
