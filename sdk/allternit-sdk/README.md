# @allternit/sdk

The official Allternit SDK for AI interactions.

## Installation

```bash
npm install @allternit/sdk
```

## Quick Start

### BYOK Mode (Bring Your Own Key)

```typescript
import { AllternitHarness } from '@allternit/sdk';

const harness = new AllternitHarness({
  mode: 'byok',
  byok: {
    anthropic: { apiKey: process.env.ANTHROPIC_API_KEY },
    openai: { apiKey: process.env.OPENAI_API_KEY },
    google: { apiKey: process.env.GOOGLE_API_KEY },
  }
});

const stream = harness.stream({
  provider: 'anthropic',
  model: 'claude-3-haiku-20240307',
  messages: [{ role: 'user', content: 'Hello!' }]
});

for await (const chunk of stream) {
  if (chunk.type === 'text') {
    process.stdout.write(chunk.text);
  }
}
```

### Cloud Mode

```typescript
const harness = new AllternitHarness({
  mode: 'cloud',
  cloud: {
    baseURL: 'https://api.allternit.com',
    accessToken: 'your-oauth-token'
  }
});
```

### Local Mode (Ollama)

```typescript
const harness = new AllternitHarness({
  mode: 'local',
  local: {
    baseURL: 'http://localhost:11434'
  }
});
```

## Supported Providers

The SDK includes built-in support for 10 AI providers:

| Provider | Import | Features |
|----------|--------|----------|
| Anthropic Claude | `AllternitAI` | Streaming, Tools, Vision |
| OpenAI | `AllternitOpenAI` | Streaming, Tools, Vision, JSON Mode |
| Google AI | `AllternitGoogleAI` | Streaming, Tools, Vision, Multi-modal |
| Mistral AI | `AllternitMistral` | Streaming, Tools, JSON Mode |
| Cohere | `AllternitCohere` | Streaming, Tools |
| Groq | `AllternitGroq` | Streaming, Tools, Ultra-fast |
| Together AI | `AllternitTogether` | Streaming, Tools, OSS Models |
| Azure OpenAI | `AllternitAzure` | Streaming, Tools, Vision |
| AWS Bedrock | `AllternitBedrock` | Streaming, Tools, Vision |
| Ollama | `AllternitOllama` | Streaming, Tools, Local |

### Provider Usage Examples

```typescript
import { 
  AllternitAI,        // Anthropic
  AllternitOpenAI,    // OpenAI
  AllternitGoogleAI,  // Google
  AllternitMistral,   // Mistral
  AllternitCohere,    // Cohere
  AllternitGroq,      // Groq
  AllternitTogether,  // Together AI
  AllternitAzure,     // Azure OpenAI
  AllternitBedrock,   // AWS Bedrock
  AllternitOllama,    // Ollama
} from '@allternit/sdk';

// Anthropic
const anthropic = new AllternitAI({ apiKey: 'your-key' });

// OpenAI
const openai = new AllternitOpenAI({ apiKey: 'your-key' });

// Google
const google = new AllternitGoogleAI({ apiKey: 'your-key' });

// Mistral
const mistral = new AllternitMistral({ apiKey: 'your-key' });

// Cohere
const cohere = new AllternitCohere({ apiKey: 'your-key' });

// Groq (ultra-fast inference)
const groq = new AllternitGroq({ apiKey: 'your-key' });

// Together AI
const together = new AllternitTogether({ apiKey: 'your-key' });

// Azure OpenAI
const azure = new AllternitAzure({
  apiKey: 'your-key',
  endpoint: 'https://your-resource.openai.azure.com',
  deploymentId: 'your-deployment'
});

// AWS Bedrock
const bedrock = new AllternitBedrock({
  region: 'us-east-1',
  accessKeyId: 'your-access-key',
  secretAccessKey: 'your-secret-key'
});

// Ollama (local)
const ollama = new AllternitOllama({
  baseURL: 'http://localhost:11434'
});
```

## Provider Registry

The provider registry provides metadata and discovery for all supported providers:

```typescript
import { 
  listProviders, 
  getProvider, 
  findProvidersByFeature,
  createProvider 
} from '@allternit/sdk';

// List all providers
const providers = listProviders();
// ['anthropic', 'openai', 'google', 'ollama', 'mistral', 
//  'cohere', 'groq', 'together', 'azure', 'bedrock']

// Get provider metadata
const anthropicMeta = getProvider('anthropic');
console.log(anthropicMeta);
// {
//   name: 'anthropic',
//   displayName: 'Anthropic Claude',
//   supportsStreaming: true,
//   supportsTools: true,
//   supportsVision: true,
//   models: ['claude-3-5-sonnet-20241022', ...],
//   features: ['streaming', 'tools', 'vision', ...]
// }

// Find providers by feature
const visionProviders = findProvidersByFeature('vision');
const streamingWithTools = findProvidersByFeature('streaming', 'tools');

// Create provider via factory
const provider = createProvider('ollama', { 
  baseURL: 'http://localhost:11434' 
});
```

## ACP (Agent Capability Protocol)

The SDK includes a complete implementation of the Agent Capability Protocol for inter-agent communication.

### ACP Registry

```typescript
import { acpRegistry, ACPRegistry } from '@allternit/sdk';

// Use the global registry
acpRegistry.register({
  agentId: 'my-agent',
  name: 'My Agent',
  description: 'An AI agent',
  version: '1.0.0',
  capabilities: [{
    name: 'chat',
    description: 'Chat capability',
    version: '1.0.0',
    tools: []
  }],
  endpoints: {},
  authentication: { type: 'none' },
  metadata: {
    tags: ['ai', 'chat'],
    category: 'agent',
    author: 'You',
    license: 'MIT'
  },
  status: 'active',
  registeredAt: new Date().toISOString(),
  lastSeenAt: new Date().toISOString()
});

// Or create your own registry
const registry = new ACPRegistry({
  maxEntries: 1000,
  ttlMs: 3600000, // 1 hour
  enableHeartbeat: true
});
```

### ACP Messages

```typescript
import { 
  validateMessage, 
  ACPMessageSchema,
  type ACPMessage 
} from '@allternit/sdk';

// Create a valid message
const message: ACPMessage = {
  id: crypto.randomUUID(),
  version: '1.0',
  timestamp: new Date().toISOString(),
  source: {
    agentId: 'agent-1',
    capability: 'chat'
  },
  target: {
    agentId: 'agent-2'
  },
  type: 'request',
  payload: {
    action: 'chat',
    parameters: {
      message: 'Hello!'
    }
  },
  metadata: {
    priority: 'normal'
  }
};

// Validate the message
const result = validateMessage(message);
if (result.valid) {
  console.log('Message is valid:', result.data);
} else {
  console.error('Validation errors:', result.errors);
}
```

### ACP Harness Bridge

Connect ACP agents to the AllternitHarness:

```typescript
import { ACPHarnessBridge, AllternitHarness } from '@allternit/sdk';

const harness = new AllternitHarness({
  mode: 'byok',
  byok: { anthropic: { apiKey: process.env.ANTHROPIC_API_KEY } }
});

const bridge = new ACPHarnessBridge({
  harness,
  agentId: 'my-acp-agent',
  capabilities: ['chat', 'stream'],
  enableStreaming: true
});

// Register the agent
bridge.register();

// Process incoming ACP messages
const response = await bridge.processMessage(incomingMessage);

// Get bridge stats
const stats = bridge.getStats();
console.log(stats);
```

## AI Runtime (`@allternit/sdk/ai-runtime`)

The AI Runtime subpath provides stateful agent execution with a native tool registry, capability system, and run persistence.

> 📖 See [`ARCHITECTURE.md`](./ARCHITECTURE.md) for the canonical architecture reference covering the execution model, tool registry, harness providers, capabilities, persistence, and platform integration.

### Quick Start

```typescript
import { AllternitHarness, AllternitAgent } from '@allternit/sdk/ai-runtime';

const harness = new AllternitHarness({
  mode: 'byok',
  byok: { anthropic: { apiKey: process.env.ANTHROPIC_API_KEY } }
});

const agent = new AllternitAgent(harness, {
  capabilities: ['filesystem', 'computer-use'],
  computerUseBaseUrl: 'http://127.0.0.1:8760',
  persistencePath: './.allternit/runs.db'
});

const run = agent.run({
  provider: 'anthropic',
  model: 'claude-sonnet-4-20250514',
  messages: [{ role: 'user', content: 'List files in my workspace' }]
});

run.on('text', (text) => process.stdout.write(text));
run.on('tool_call', (tc) => console.log('Tool called:', tc.name));
run.on('completed', () => console.log('Done'));
```

### Tool Registry

```typescript
import { ToolRegistry, NativeToolBelt } from '@allternit/sdk/ai-runtime';

const registry = new ToolRegistry();

// Register an active tool
registry.registerTool({
  name: 'calculate',
  description: 'Perform a calculation',
  input_schema: { type: 'object', properties: { expression: { type: 'string' } }, required: ['expression'] },
  execute: async (args) => eval(args.expression)
});

// Register a deferred tool (discoverable via tool_search)
registry.registerDeferredTool({
  id: 'advanced_math',
  name: 'advanced_math',
  description: 'Advanced mathematical operations',
  input_schema: { type: 'object', properties: {} },
  activate: () => ({ /* full ToolDefinition */ })
});

// Search and activate
const results = registry.search('math');
registry.activateTool('advanced_math');

// Snapshot for persistence
const snapshot = registry.snapshot();
registry.rehydrate(snapshot);
```

### Capabilities

| Capability | Tools Provided | Config Option |
|-----------|----------------|---------------|
| `filesystem` | `read_file`, `write_file` | — |
| `computer-use` (deprecated) | `computer` (mouse, keyboard, screenshots) | `computerUseBaseUrl` |
| `brain` | `query_brain` | — |
| HITL | `ask_user_question` | — |

The `computer-use` capability and the `@allternit/sdk/computer-use` client are deprecated (2026-10-09). New code uses `@allternit/computer-driver`, which drives hosted computers through the computer toolset (`/v1/computers/:id/toolset`) and ships Anthropic, OpenAI and Gemini adapters. The old pieces are not drop-ins for it, so they stay as thin shims.

### AgentRun Events

```typescript
run.on('text', (text: string) => { /* text delta */ });
run.on('tool_call', (chunk: ToolCallChunk) => { /* tool call started */ });
run.on('tool_call_complete', (chunk: ToolCallCompleteChunk) => { /* tool call finished */ });
run.on('reply_requested', (req: ReplyRequest) => { /* permission/question */ });
run.on('completed', () => { /* run finished */ });
run.on('error', (err) => { /* run failed */ });
```

### Persistence

`AgentStorage` auto-detects the runtime and chooses the best backend:
- **Bun**: `bun:sqlite`
- **Node 22.5+**: `node:sqlite`
- **Fallback**: In-memory

```typescript
import { AgentStorage } from '@allternit/sdk/ai-runtime';

const storage = new AgentStorage({ path: './runs.db' });
storage.saveRun(runId, status, messages, metadata);
const record = storage.getRun(runId);
```

## System Prompts

The SDK includes built-in system prompt management:

```typescript
import { 
  ALLTERNIT_SYSTEM_PROMPT, 
  injectSystemPrompt 
} from '@allternit/sdk';

// View the default system prompt
console.log(ALLTERNIT_SYSTEM_PROMPT);

// Inject system prompt into messages
const messages = [
  { role: 'user', content: 'Hello!' }
];

const withSystemPrompt = injectSystemPrompt(messages, false);
// [
//   { role: 'system', content: 'You are Allternit...' },
//   { role: 'user', content: 'Hello!' }
// ]
```

## API Reference

### AllternitHarness

Main class for AI interactions.

#### Constructor

```typescript
new AllternitHarness(config: HarnessConfig)
```

#### Methods

- `stream(request: StreamRequest): AsyncGenerator<HarnessStreamChunk>` - Stream responses
- `complete(request: StreamRequest): Promise<HarnessResponse>` - Get complete response

### Types

```typescript
interface HarnessConfig {
  mode: 'byok' | 'cloud' | 'local' | 'subprocess';
  byok?: {
    anthropic?: { apiKey: string; baseURL?: string };
    openai?: { apiKey: string; baseURL?: string };
    google?: { apiKey: string };
  };
  cloud?: { baseURL: string; accessToken: string };
  local?: { baseURL: string };
  subprocess?: { command: string; args?: string[]; env?: Record<string, string> };
}

interface StreamRequest {
  provider: string;
  model: string;
  messages: Message[];
  temperature?: number;
  maxTokens?: number;
  topP?: number;
  topK?: number;
  tools?: Tool[];
}

type HarnessStreamChunk =
  | { type: 'text'; text: string }
  | { type: 'tool_call'; id: string; name: string; arguments: string }
  | { type: 'tool_call_complete'; id: string; name: string; arguments: Record<string, unknown> }
  | { type: 'tool_result'; toolCallId: string; content: string }
  | { type: 'error'; error: Error; code?: string }
  | { type: 'done'; usage?: { promptTokens: number; completionTokens: number; totalTokens: number } };
```

## Modes

### BYOK
Use your own API keys for providers. Full control over model selection and configuration.

### Cloud
Use Allternit's managed service. Simplified setup with unified billing.

### Local
Connect to local Ollama server. Run open-source models on your hardware.

### Subprocess
Use CLI tools as backends. Integrate custom model runners.

## Examples

See the `examples/` directory for complete examples:

- `byok-example.ts` - Bring Your Own Key setup
- `cloud-example.ts` - Cloud mode usage
- `ollama-example.ts` - Local Ollama integration

## License

MIT

## Version

Current version: `1.0.0`
