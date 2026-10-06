# Integration Implementation Guide

**Claude Code + Allternit/Gizzi Integration**

---

## 1. Module Integration Map

### 1.1 High-Priority Integrations

```
┌────────────────────────────────────────────────────────────────────────┐
│                    INTEGRATION PRIORITY MATRIX                          │
├────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  CRITICAL (Must Have)                                                   │
│  ├── Blueprint Execution Engine → Use Claude Code tools as actions     │
│  ├── Connector Abstraction → Wrap MCP + custom connectors              │
│  ├── Circuit Breakers → Integrate into tool permission layer           │
│  └── Heartbeat Context → Extend session context system                 │
│                                                                         │
│  HIGH (Should Have)                                                     │
│  ├── Routine Scheduler → Add to Claude Code's task system              │
│  ├── Multi-Environment → Extend config system                          │
│  ├── Blueprint Visualization → New Ink components                      │
│  └── Connector Management UI → Settings extension                      │
│                                                                         │
│  MEDIUM (Nice to Have)                                                  │
│  ├── Blueprint Marketplace → Plugin system extension                   │
│  ├── Advanced Workflow Patterns → Conditional, parallel execution      │
│  └── Cross-Session Persistence → State management enhancement          │
│                                                                         │
└────────────────────────────────────────────────────────────────────────┘
```

### 1.2 Code Integration Points

| Allternit Feature | Claude Code Integration Target | Implementation Strategy |
|-------------------|-------------------------------|------------------------|
| `blueprint.yaml` | New `/blueprint` command | Add command + execution engine |
| `connectors/` | Extend MCP system | Create Connector → MCP adapter |
| `heartbeat` | Modify `context.ts` | Add persistent agent context |
| `routines` | Extend `tasks/` | Add scheduled task support |
| `circuit_breakers` | Modify `hooks/toolPermission/` | Add execution limits |
| `environments` | Extend `services/` | Add env-specific configs |

---

## 2. Blueprint Engine Implementation

### 2.1 Blueprint Schema (Unified)

```typescript
// src/blueprint/types.ts
export interface Blueprint {
  apiVersion: 'allternit.io/v1';
  kind: 'WorkflowBlueprint';
  metadata: {
    name: string;
    version: string;
    displayName: string;
    description: string;
  };
  agents: Record<string, BlueprintAgent>;
  connectors: Record<string, BlueprintConnector>;
  routines: Record<string, BlueprintRoutine>;
  reliability?: ReliabilityConfig;
}

export interface BlueprintAgent {
  id: string;
  name: string;
  role: string;
  model: {
    provider: string;
    model: string;
    temperature?: number;
    maxTokens?: number;
  };
  tools: string[];  // Claude Code tool names
  heartbeat?: HeartbeatConfig;
}

export interface BlueprintConnector {
  id: string;
  type: 'mcp' | 'oauth' | 'api_key';
  config: Record<string, unknown>;
  // Maps to Claude Code MCP tools or creates new tools
}

export interface BlueprintRoutine {
  id: string;
  schedule: {
    cron: string;
    timezone?: string;
  };
  agent: string;
  steps: WorkflowStep[];
}

export interface WorkflowStep {
  id: string;
  action: string;  // Claude Code tool name or connector action
  params: Record<string, unknown>;
  output?: string;
  condition?: string;  // Conditional execution
}
```

### 2.2 Blueprint Execution Engine

```typescript
// src/blueprint/BlueprintEngine.ts
import type { Blueprint, BlueprintRoutine } from './types.js';
import type { Tools } from '../Tool.js';

export class BlueprintEngine {
  private tools: Tools;
  private connectors: Map<string, Connector>;
  
  constructor(tools: Tools) {
    this.tools = tools;
    this.connectors = new Map();
  }
  
  async loadBlueprint(path: string): Promise<Blueprint> {
    // Load and validate YAML
    const content = await fs.readFile(path, 'utf8');
    return this.parseBlueprint(content);
  }
  
  async executeRoutine(
    blueprint: Blueprint, 
    routineId: string,
    context: ExecutionContext
  ): Promise<RoutineResult> {
    const routine = blueprint.routines[routineId];
    const agent = blueprint.agents[routine.agent];
    
    // Set up heartbeat context
    const heartbeatContext = await this.loadHeartbeat(agent);
    
    // Execute with circuit breakers
    return this.executeWithCircuitBreakers(
      () => this.runSteps(routine.steps, context),
      blueprint.reliability
    );
  }
  
  private async runSteps(
    steps: WorkflowStep[], 
    context: ExecutionContext
  ): Promise<StepResult[]> {
    const results: StepResult[] = [];
    
    for (const step of steps) {
      // Check if tool exists in Claude Code
      const tool = this.tools[step.action];
      if (tool) {
        const result = await this.executeTool(tool, step.params, context);
        results.push({ step: step.id, result });
      } else {
        // Try connector
        const connector = this.connectors.get(step.action);
        if (connector) {
          const result = await connector.execute(step.params);
          results.push({ step: step.id, result });
        }
      }
    }
    
    return results;
  }
}
```

### 2.3 New Commands to Add

```typescript
// src/commands/blueprint/blueprint.tsx
export default function BlueprintCommand() {
  // /blueprint list - List available blueprints
  // /blueprint load <name> - Load a blueprint
  // /blueprint run <routine> - Execute a routine
  // /blueprint status - Show running routines
  // /blueprint connectors - Manage connectors
}

// src/commands/routine/routine.tsx  
export default function RoutineCommand() {
  // /routine list - List scheduled routines
  // /routine schedule <blueprint>/<routine> - Schedule a routine
  // /routine cancel <id> - Cancel a scheduled routine
  // /routine logs <id> - View routine execution logs
}
```

---

## 3. Connector System Integration

### 3.1 Connector Abstraction Layer

```typescript
// src/connectors/types.ts
export interface Connector {
  id: string;
  name: string;
  authType: 'oauth' | 'api_key' | 'token' | 'none';
  
  authenticate(config: AuthConfig): Promise<void>;
  execute(action: string, params: unknown): Promise<unknown>;
  listActions(): string[];
}

// Unified interface that can wrap MCP or Allternit-style connectors
export interface ConnectorAdapter {
  // Convert connector actions to Claude Code tools
  toTool(connector: Connector, action: string): Tool;
  
  // Register all connector actions as tools
  registerTools(connector: Connector): Tools;
}
```

### 3.2 MCP-to-Connector Bridge

```typescript
// src/connectors/McpConnectorAdapter.ts
import { Client } from '@modelcontextprotocol/client'; // SDK v2

export class McpConnectorAdapter implements Connector {
  private mcpClient: Client;
  private tools: Map<string, McpTool>;
  
  constructor(mcpClient: Client) {
    this.mcpClient = mcpClient;
    this.tools = new Map();
  }
  
  async initialize(): Promise<void> {
    // List available MCP tools
    const response = await this.mcpClient.listTools();
    
    // Convert to connector actions
    for (const tool of response.tools) {
      this.tools.set(tool.name, {
        name: tool.name,
        description: tool.description,
        schema: tool.inputSchema,
        execute: async (params) => {
          return this.mcpClient.callTool({
            name: tool.name,
            arguments: params
          });
        }
      });
    }
  }
  
  async execute(action: string, params: unknown): Promise<unknown> {
    const tool = this.tools.get(action);
    if (!tool) {
      throw new Error(`Unknown action: ${action}`);
    }
    return tool.execute(params);
  }
  
  // Convert to Claude Code Tool format
  toClaudeTool(action: string): Tool {
    const tool = this.tools.get(action);
    return {
      name: `${this.id}_${action}`,
      description: tool.description,
      parameters: tool.schema,
      execute: tool.execute,
      // ... other Tool properties
    };
  }
}
```

### 3.3 GitHub Connector Example

```typescript
// src/connectors/github/GitHubConnector.ts
export class GitHubConnector implements Connector {
  id = 'github';
  name = 'GitHub';
  authType = 'token';
  
  private octokit: Octokit;
  
  async authenticate(config: { token: string }): Promise<void> {
    this.octokit = new Octokit({ auth: config.token });
  }
  
  listActions(): string[] {
    return [
      'repos.listForUser',
      'repos.get',
      'issues.list',
      'issues.create',
      'pulls.list',
      'pulls.get',
      'pulls.createReview'
    ];
  }
  
  async execute(action: string, params: unknown): Promise<unknown> {
    // Map connector actions to GitHub API calls
    switch (action) {
      case 'repos.listForUser':
        return this.octokit.repos.listForUser(params);
      case 'issues.list':
        return this.octokit.issues.list(params);
      case 'pulls.list':
        return this.octokit.pulls.list(params);
      // ... etc
    }
  }
  
  // Convert to Claude Code Tool format for use in blueprints
  toClaudeTools(): Tool[] {
    return [
      {
        name: 'github_list_prs',
        description: 'List pull requests in a repository',
        parameters: z.object({
          owner: z.string(),
          repo: z.string(),
          state: z.enum(['open', 'closed', 'all']).optional()
        }),
        execute: async (params) => this.execute('pulls.list', {
          owner: params.owner,
          repo: params.repo,
          state: params.state
        })
      },
      // ... more tools
    ];
  }
}
```

---

## 4. Heartbeat Context System

### 4.1 Extending Session Context

```typescript
// src/heartbeat/types.ts
export interface HeartbeatContext {
  agent: {
    id: string;
    name: string;
    role: string;
    values: string[];
  };
  organization: {
    name: string;
    values: string;
  };
  currentFocus: string;
  dynamicContext: {
    // Pulled from external sources
    open_prs_count?: number;
    active_blockers?: string[];
    sprint_name?: string;
    sprint_end?: string;
  };
  lastUpdated: Date;
}

export interface ContextSource {
  type: 'github' | 'database' | 'api' | 'connector';
  query: string;
  as: string;  // Variable name in context
  refreshInterval?: number;  // Seconds
}
```

### 4.2 Heartbeat Manager

```typescript
// src/heartbeat/HeartbeatManager.ts
export class HeartbeatManager {
  private context: HeartbeatContext;
  private sources: ContextSource[];
  private refreshTimer?: NodeJS.Timeout;
  
  constructor(context: HeartbeatContext, sources: ContextSource[]) {
    this.context = context;
    this.sources = sources;
  }
  
  async initialize(): Promise<void> {
    // Initial context fetch
    await this.refreshContext();
    
    // Set up periodic refresh
    const minInterval = Math.min(
      ...this.sources.map(s => s.refreshInterval || 300)
    );
    this.refreshTimer = setInterval(() => {
      this.refreshContext();
    }, minInterval * 1000);
  }
  
  async refreshContext(): Promise<void> {
    for (const source of this.sources) {
      try {
        const value = await this.fetchFromSource(source);
        this.context.dynamicContext[source.as] = value;
      } catch (error) {
        console.error(`Failed to refresh context source ${source.as}:`, error);
      }
    }
    this.context.lastUpdated = new Date();
  }
  
  private async fetchFromSource(source: ContextSource): Promise<unknown> {
    switch (source.type) {
      case 'github':
        return this.queryGitHub(source.query);
      case 'database':
        return this.queryDatabase(source.query);
      case 'connector':
        return this.queryConnector(source.query);
      default:
        throw new Error(`Unknown source type: ${source.type}`);
    }
  }
  
  // Generate heartbeat prompt for LLM
  generateHeartbeatPrompt(): string {
    return `
You are ${this.context.agent.name}, the ${this.context.agent.role} at ${this.context.organization.name}.

YOUR ROLE:
${this.context.agent.role}

YOUR VALUES:
${this.context.organization.values}

CURRENT CONTEXT:
${Object.entries(this.context.dynamicContext)
  .map(([key, value]) => `- ${key}: ${JSON.stringify(value)}`)
  .join('\n')}

CURRENT FOCUS: ${this.context.currentFocus}

LAST UPDATED: ${this.context.lastUpdated.toISOString()}
`.trim();
  }
}
```

### 4.3 Integration with QueryEngine

```typescript
// src/QueryEngine.ts modifications
export class QueryEngine {
  private heartbeatManager?: HeartbeatManager;
  
  // Add heartbeat context to every query
  async query(input: string, options: QueryOptions): Promise<QueryResult> {
    let systemPrompt = options.systemPrompt || '';
    
    // Prepend heartbeat context if available
    if (this.heartbeatManager) {
      const heartbeatPrompt = this.heartbeatManager.generateHeartbeatPrompt();
      systemPrompt = `${heartbeatPrompt}\n\n${systemPrompt}`;
    }
    
    // Continue with normal query processing
    return this.executeQuery(input, { ...options, systemPrompt });
  }
}
```

---

## 5. Circuit Breaker Integration

### 5.1 Circuit Breaker Implementation

```typescript
// src/circuitBreaker/types.ts
export interface CircuitBreakerConfig {
  maxIterations: number;
  maxToolCalls: number;
  maxTokensPerRun: number;
  timeout: number;
  failureThreshold: number;
  recoveryTimeout: number;
}

export enum CircuitState {
  CLOSED = 'CLOSED',      // Normal operation
  OPEN = 'OPEN',          // Failing, rejecting requests
  HALF_OPEN = 'HALF_OPEN' // Testing if recovered
}

export class CircuitBreaker {
  private state: CircuitState = CircuitState.CLOSED;
  private failures = 0;
  private lastFailureTime?: number;
  private config: CircuitBreakerConfig;
  
  constructor(config: CircuitBreakerConfig) {
    this.config = config;
  }
  
  async execute<T>(fn: () => Promise<T>): Promise<T> {
    if (this.state === CircuitState.OPEN) {
      if (this.shouldAttemptReset()) {
        this.state = CircuitState.HALF_OPEN;
      } else {
        throw new CircuitBreakerOpenError('Circuit breaker is open');
      }
    }
    
    try {
      const result = await this.executeWithTimeout(fn);
      this.onSuccess();
      return result;
    } catch (error) {
      this.onFailure();
      throw error;
    }
  }
  
  private shouldAttemptReset(): boolean {
    if (!this.lastFailureTime) return true;
    return Date.now() - this.lastFailureTime >= this.config.recoveryTimeout;
  }
  
  private onSuccess(): void {
    this.failures = 0;
    this.state = CircuitState.CLOSED;
  }
  
  private onFailure(): void {
    this.failures++;
    this.lastFailureTime = Date.now();
    
    if (this.failures >= this.config.failureThreshold) {
      this.state = CircuitState.OPEN;
    }
  }
}
```

### 5.2 Integration with Tool Permission System

```typescript
// src/hooks/toolPermission/handlers/interactiveHandler.ts modifications
export class InteractivePermissionHandler {
  private circuitBreakers: Map<string, CircuitBreaker>;
  private globalLimits: {
    totalToolCalls: number;
    totalTokens: number;
    startTime: number;
  };
  
  constructor(private config: CircuitBreakerConfig) {
    this.circuitBreakers = new Map();
    this.globalLimits = {
      totalToolCalls: 0,
      totalTokens: 0,
      startTime: Date.now()
    };
  }
  
  async handleToolPermission(
    tool: Tool,
    params: unknown,
    context: PermissionContext
  ): Promise<PermissionResult> {
    // Check global limits
    if (this.globalLimits.totalToolCalls >= this.config.maxToolCalls) {
      return {
        allowed: false,
        reason: `Maximum tool calls (${this.config.maxToolCalls}) exceeded`
      };
    }
    
    // Check timeout
    const elapsed = (Date.now() - this.globalLimits.startTime) / 1000;
    if (elapsed >= this.config.timeout) {
      return {
        allowed: false,
        reason: `Timeout (${this.config.timeout}s) exceeded`
      };
    }
    
    // Get or create circuit breaker for this tool
    let cb = this.circuitBreakers.get(tool.name);
    if (!cb) {
      cb = new CircuitBreaker(this.config);
      this.circuitBreakers.set(tool.name, cb);
    }
    
    // Execute with circuit breaker
    try {
      const result = await cb.execute(() => this.executeTool(tool, params));
      this.globalLimits.totalToolCalls++;
      return { allowed: true, result };
    } catch (error) {
      if (error instanceof CircuitBreakerOpenError) {
        return {
          allowed: false,
          reason: `Circuit breaker open for tool ${tool.name}`
        };
      }
      throw error;
    }
  }
}
```

---

## 6. Routine Scheduler Integration

### 6.1 Routine Scheduler Service

```typescript
// src/services/routineScheduler/RoutineScheduler.ts
import { CronJob } from 'cron';

export class RoutineScheduler {
  private jobs: Map<string, CronJob>;
  private blueprintEngine: BlueprintEngine;
  
  constructor(blueprintEngine: BlueprintEngine) {
    this.jobs = new Map();
    this.blueprintEngine = blueprintEngine;
  }
  
  async schedule(
    blueprint: Blueprint,
    routineId: string,
    options: ScheduleOptions
  ): Promise<string> {
    const routine = blueprint.routines[routineId];
    const jobId = `${blueprint.metadata.name}/${routineId}`;
    
    const job = new CronJob(
      routine.schedule.cron,
      async () => {
        try {
          console.log(`[Routine] Executing ${jobId}`);
          await this.blueprintEngine.executeRoutine(
            blueprint,
            routineId,
            { scheduled: true, timestamp: new Date() }
          );
        } catch (error) {
          console.error(`[Routine] Failed ${jobId}:`, error);
          // Notify on failure
          await this.notifyFailure(jobId, error);
        }
      },
      null,
      true,
      routine.schedule.timezone
    );
    
    this.jobs.set(jobId, job);
    return jobId;
  }
  
  cancel(jobId: string): void {
    const job = this.jobs.get(jobId);
    if (job) {
      job.stop();
      this.jobs.delete(jobId);
    }
  }
  
  list(): Array<{ id: string; running: boolean }> {
    return Array.from(this.jobs.entries()).map(([id, job]) => ({
      id,
      running: job.running
    }));
  }
}
```

### 6.2 New Command: /routine

```typescript
// src/commands/routine/routine.tsx
import React from 'react';
import { Box, Text } from 'ink';
import { useBlueprintStore } from '../../hooks/useBlueprintStore.js';

export default function RoutineCommand() {
  const { routines, scheduleRoutine, cancelRoutine } = useBlueprintStore();
  
  return (
    <Box flexDirection="column">
      <Text bold>Routine Management</Text>
      
      {/* List scheduled routines */}
      {routines.map(routine => (
        <Box key={routine.id}>
          <Text>{routine.id}</Text>
          <Text> - </Text>
          <Text color={routine.running ? 'green' : 'gray'}>
            {routine.running ? 'Running' : 'Scheduled'}
          </Text>
          <Text> (Next: {routine.nextRun})</Text>
        </Box>
      ))}
      
      {/* Schedule new routine */}
      <RoutineSchedulerForm 
        onSchedule={(blueprint, routine, cron) => {
          scheduleRoutine(blueprint, routine, cron);
        }}
      />
    </Box>
  );
}
```

---

## 7. Multi-Environment Configuration

### 7.1 Environment-Aware Config

```typescript
// src/config/environments.ts
export interface EnvironmentConfig {
  name: 'dev' | 'staging' | 'prod';
  model: {
    provider: string;
    model: string;
    temperature?: number;
  };
  connectors: Record<string, ConnectorEnvironmentConfig>;
  permissions: {
    requireApproval: boolean;
    approvers?: string[];
  };
  circuitBreakers: CircuitBreakerConfig;
}

export class EnvironmentManager {
  private currentEnv: string;
  private environments: Map<string, EnvironmentConfig>;
  
  constructor() {
    this.environments = new Map();
    this.loadEnvironments();
  }
  
  async switchEnvironment(env: string): Promise<void> {
    const config = this.environments.get(env);
    if (!config) {
      throw new Error(`Unknown environment: ${env}`);
    }
    
    // Apply environment-specific settings
    await this.applyModelConfig(config.model);
    await this.applyConnectorConfigs(config.connectors);
    await this.applyPermissionConfig(config.permissions);
    await this.applyCircuitBreakerConfig(config.circuitBreakers);
    
    this.currentEnv = env;
    console.log(`Switched to ${env} environment`);
  }
  
  getCurrentEnv(): string {
    return this.currentEnv;
  }
}
```

### 7.2 Blueprint Environment Override

```yaml
# Example blueprint with environment overrides
apiVersion: allternit.io/v1
kind: WorkflowBlueprint
metadata:
  name: engineering-workflow
  
agents:
  tech-lead:
    model:
      provider: anthropic
      model: claude-3-opus
      
    # Environment-specific overrides
    environments:
      dev:
        model:
          provider: openai
          model: gpt-3.5-turbo  # Cheaper for dev
        permissions:
          read_only: true
      staging:
        model:
          provider: anthropic
          model: claude-3-sonnet
      prod:
        permissions:
          require_approval: true
          approvers: [tech-lead]
        circuit_breakers:
          max_iterations: 3  # Stricter in prod
```

---

## 8. Migration Path

### 8.1 Config Migration

```typescript
// src/migrations/configMigration.ts
export async function migrateClaudeCodeConfig(): Promise<UnifiedConfig> {
  const existingConfig = await loadClaudeCodeConfig();
  
  return {
    // Preserve existing Claude Code settings
    ...existingConfig,
    
    // Add new Allternit sections
    allternit: {
      blueprints: {
        directory: '~/.allternit/blueprints',
        autoLoad: true
      },
      connectors: {
        directory: '~/.allternit/connectors'
      },
      environments: {
        default: 'dev',
        available: ['dev', 'staging', 'prod']
      },
      heartbeat: {
        enabled: true,
        refreshInterval: 300
      },
      circuitBreakers: {
        maxIterations: 5,
        maxToolCalls: 10,
        timeout: 300
      }
    }
  };
}
```

### 8.2 Command Aliases

```typescript
// Maintain backwards compatibility
const commandAliases: Record<string, string> = {
  // Claude Code commands remain unchanged
  '/commit': '/commit',
  '/review': '/review',
  '/compact': '/compact',
  
  // New Allternit commands
  '/blueprint': '/blueprint',
  '/routine': '/routine',
  '/connector': '/connector',
  '/env': '/environment',
  
  // Aliases for common operations
  '/workflow': '/blueprint',
  '/schedule': '/routine',
};
```

---

## 9. Testing Strategy

### 9.1 Unit Tests

```typescript
// tests/blueprint/BlueprintEngine.test.ts
describe('BlueprintEngine', () => {
  it('should execute a simple routine', async () => {
    const engine = new BlueprintEngine(mockTools);
    const blueprint = loadTestBlueprint('simple-routine');
    
    const result = await engine.executeRoutine(blueprint, 'test-routine', {});
    
    expect(result.success).toBe(true);
    expect(result.steps).toHaveLength(3);
  });
  
  it('should apply circuit breakers', async () => {
    const engine = new BlueprintEngine(mockTools);
    const blueprint = loadTestBlueprint('with-circuit-breakers');
    
    // Force failures
    mockTools.failingTool.mockRejectedValue(new Error('Failed'));
    
    await expect(
      engine.executeRoutine(blueprint, 'failing-routine', {})
    ).rejects.toThrow(CircuitBreakerOpenError);
  });
});
```

### 9.2 Integration Tests

```typescript
// tests/integration/blueprint-workflow.test.ts
describe('Blueprint Workflow Integration', () => {
  it('should run a complete blueprint with connectors', async () => {
    const cli = await startCli();
    
    // Load blueprint
    await cli.type('/blueprint load test-blueprint');
    await cli.pressEnter();
    
    // Run routine
    await cli.type('/blueprint run daily-standup');
    await cli.pressEnter();
    
    // Verify output
    expect(cli.output).toContain('Routine completed');
    expect(cli.output).toContain('Standup report generated');
  });
});
```

---

## 10. Summary

### Files to Create/Modify

| Action | File Path | Lines (Est.) |
|--------|-----------|--------------|
| **Create** | `src/blueprint/` | ~2,000 |
| **Create** | `src/connectors/` | ~1,500 |
| **Create** | `src/heartbeat/` | ~800 |
| **Create** | `src/circuitBreaker/` | ~600 |
| **Create** | `src/services/routineScheduler/` | ~500 |
| **Modify** | `src/QueryEngine.ts` | +200 |
| **Modify** | `src/hooks/toolPermission/` | +300 |
| **Modify** | `src/commands.ts` | +100 |
| **Create** | `src/commands/blueprint/` | ~800 |
| **Create** | `src/commands/routine/` | ~400 |

**Total Estimated Effort:** ~7,000 lines of code, 8-12 weeks

---

*Implementation guide generated: 2026-03-31*
