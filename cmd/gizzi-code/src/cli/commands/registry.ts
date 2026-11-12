
import { ExecCommand, RunCommand } from "@/cli/commands/run"
import { GenerateCommand } from "@/cli/commands/generate"
import { ApiKeysCommand } from "@/cli/commands/api-keys"
import { ConnectCommand } from "@/cli/commands/connect"
import { SkillsCommand } from "@/cli/commands/skills"
import { UpgradeCommand } from "@/cli/commands/upgrade"
import { UninstallCommand } from "@/cli/commands/uninstall"
import { ModelsCommand } from "@/cli/commands/models"
import { HardwareCommand } from "@/cli/commands/hardware"
import { ServeCommand } from "@/cli/commands/serve"
import { PairCommand, LoginCommand } from "@/cli/commands/pair"
import { ComputersCommand } from "@/cli/commands/computers"
import { PhoneCommand } from "@/cli/commands/phone"
import { TeleportCommand } from "@/cli/commands/teleport"
import { DebugCommand } from "@/cli/commands/debug"
import { StatsCommand } from "@/cli/commands/stats"
import { McpCommand } from "@/cli/commands/mcp"
import { GithubCommand } from "@/cli/commands/github"
import { ExportCommand } from "@/cli/commands/export"
import { ImportCommand } from "@/cli/commands/import"
import { AttachCommand } from "@/cli/ui/ink-app/attach"
import { TuiThreadCommand } from "@/cli/ui/ink-app/thread"
import { AcpCommand } from "@/cli/commands/acp"
import { WebCommand } from "@/cli/commands/web"
import { PrCommand } from "@/cli/commands/pr"
import { SessionCommand } from "@/cli/commands/session"
import { DbCommand } from "@/cli/commands/db"
import { PluginCommand } from "@/cli/commands/plugin"
import { InitCommand } from "@/cli/commands/init"
import { DoctorCommand } from "@/cli/commands/doctor"
import { VerificationCommand } from "@/cli/commands/verification"
import { FabricWorkerCommand } from "@/cli/commands/fabric-worker"
import { ProviderCommand } from "@/cli/commands/provider"
import { AllternitCommand } from "@/cli/commands/allternit"
import { BrainCommand } from "@/cli/commands/brain"
import { MemoryCommand } from "@/cli/commands/memory"
import { ProductsCommand } from "@/cli/commands/products"
import { HtmlArtifactCommand } from "@/cli/commands/html-artifact"
import { ArtifactCommand } from "@/cli/commands/artifact"
import { PdfCommand } from "@/cli/commands/pdf"
import { ProgramsCommand } from "@/cli/commands/programs"
import { OrgCommand } from "@/cli/commands/org"
import { UsageCommand } from "@/cli/commands/usage"
import { LabsCommand } from "@/cli/commands/labs"
import { HookCommand } from "@/cli/commands/hook"
import { TeamMcpCommand } from "@/cli/commands/team-mcp"
import { UdemyCommand } from "@/cli/commands/udemy"
import { VaultCommand } from "@/cli/commands/vault"
import { CodemapCommand } from "@/cli/commands/codemap"
import { StatusCommand } from "@/cli/commands/status"
import { AuthCommand } from "@/cli/commands/auth"
import { ConfigCommand } from "@/cli/commands/config"
import { ProfileCommand } from "@/cli/commands/profile"
import { PermissionProfileCommand } from "@/cli/commands/permission-profile"
import { RemoteCommand } from "@/cli/commands/remote"
import { CronLegacyStartCommand } from "@/cli/commands/cron"
import { BotPauseAliasCommand } from "@/cli/commands/bot"
import { AgentsCommand } from "@/cli/commands/factory/agents"
import { TeamCommand } from "@/cli/commands/team"
import { OrchestrationCommand } from "@/cli/commands/factory/orchestration"
import { WorkflowsCommand } from "@/cli/commands/factory/workflows"
import { WorkspaceCommand } from "@/cli/commands/factory/workspace"
import { BoardCommand, SwarmCommand } from "@/cli/commands/factory/swarm"
import { MissionCommand } from "@/cli/commands/mission"

/**
 * Structural shape shared by every module in the registry. Deliberately
 * narrower than yargs CommandModule (whose generic handler typing rejects
 * modules with strongly-typed handlers under strict contravariance) — main.ts
 * is the only consumer that hands these to yargs, and it validates at runtime.
 */
export interface RegisteredCommand {
  command?: string | readonly string[]
  describe?: string | false
}

/**
 * The single source of truth for the gizzi command tree. `main.ts` spreads
 * this into the root yargs instance and the completions generator derives
 * its output from it, so `gizzi --help` and `gizzi completions <shell>` can
 * never drift apart.
 *
 * The `completions` command itself is registered separately by main.ts so
 * this module stays dependency-cycle-free.
 */
export const COMMANDS: RegisteredCommand[] = [
  // The four Allternit Factory parts (SPEC §7). Gizzi's older agent-control
  // commands (agent, agent-hub, runtime, bot, thread, mail, ac, cron,
  // cowork, cowork-team) are folded in under these.
  AgentsCommand,
  TeamCommand,
  OrchestrationCommand,
  WorkflowsCommand,
  WorkspaceCommand,
  // Swarms over the Factory: start, watch, wait, land.
  SwarmCommand,
  BoardCommand,
  // Missions: a long goal planned into milestones, each run as a swarm, validated and landed.
  MissionCommand,
  AcpCommand,
  McpCommand,
  TuiThreadCommand,
  AttachCommand,
  RunCommand,
  ExecCommand,
  GenerateCommand,
  ApiKeysCommand,
  DebugCommand,
  ConnectCommand,
  SkillsCommand,
  UpgradeCommand,
  UninstallCommand,
  ServeCommand,
  PairCommand,
  ComputersCommand,
  PhoneCommand,
  TeleportCommand,
  LoginCommand,
  WebCommand,
  ModelsCommand,
  HardwareCommand,
  StatsCommand,
  StatusCommand,
  ExportCommand,
  ImportCommand,
  GithubCommand,
  PrCommand,
  SessionCommand,
  DbCommand,
  PluginCommand,
  InitCommand,
  DoctorCommand,
  VerificationCommand,
  FabricWorkerCommand,
  ProviderCommand,
  AllternitCommand,
  BrainCommand,
  MemoryCommand,
  HtmlArtifactCommand,
  ArtifactCommand,
  PdfCommand,
  ProgramsCommand,
  OrgCommand,
  UsageCommand,
  ProductsCommand,
  LabsCommand,
  UdemyCommand,
  VaultCommand,
  CodemapCommand,
  AuthCommand,
  ConfigCommand,
  ProfileCommand,
  PermissionProfileCommand,
  RemoteCommand,
  HookCommand,
  TeamMcpCommand,
  // Hidden: installed cron supervisors still exec `gizzi cron start`.
  CronLegacyStartCommand,
  // Hidden short form: `gizzi bot pause|resume`.
  BotPauseAliasCommand,
]
