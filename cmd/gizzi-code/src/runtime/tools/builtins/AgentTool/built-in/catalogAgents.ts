// @ts-nocheck
import { BUILTIN_CATALOG } from '@/runtime/agents/builtin-catalog'
import type { BuiltInAgentDefinition } from '../loadAgentsDir.js'

/**
 * The 15-type Allternit subagent catalog as ink-app AgentDefinitions.
 * One source: runtime/agents/builtin-catalog.ts. `model` carries the class
 * name (small/standard/deep/inherit); utils/model/agent.ts resolves it.
 */
export function getCatalogAgents(): BuiltInAgentDefinition[] {
  return BUILTIN_CATALOG.map(entry => ({
    agentType: entry.name,
    whenToUse: entry.description,
    ...(entry.tools ? { tools: [...entry.tools] } : {}),
    model: entry.modelClass,
    maxTurns: entry.steps,
    title: entry.title,
    face: entry.face,
    source: 'built-in' as const,
    baseDir: 'built-in' as const,
    getSystemPrompt: () => entry.prompt,
  }))
}
