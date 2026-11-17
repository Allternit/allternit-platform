/**
 * TUI parity features switched on in script/features.mjs (2026-09-26):
 * gate defaults, auto-mode classifier prompt + model allowlist, small-model
 * resolution for side calls, and the cron tools. Run with the shipped flags
 * (script/ci-smoke-test.sh passes them as --feature=…).
 */
import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtempSync, rmSync, writeFileSync } from 'fs'
import { tmpdir } from 'os'
import { join } from 'path'
import { gizziGateDefault } from '../src/constants/gizziGates'
import { getFeatureValue_CACHED_MAY_BE_STALE } from '../src/cli/ui/ink-app/services/analytics/growthbook'
import { modelSupportsAutoMode } from '../src/cli/ui/ink-app/utils/betas'
import {
  buildDefaultExternalSystemPrompt,
  getDefaultExternalAutoModeRules,
} from '../src/cli/ui/ink-app/utils/permissions/yoloClassifier'
import { getSmallFastModel } from '../src/cli/ui/ink-app/utils/model/model'
import { CronCreateTool } from '../src/cli/ui/ink-app/tools/ScheduleCronTool/CronCreateTool'
import { CronDeleteTool } from '../src/cli/ui/ink-app/tools/ScheduleCronTool/CronDeleteTool'
import { CronListTool } from '../src/cli/ui/ink-app/tools/ScheduleCronTool/CronListTool'
import { getBuiltInAgents } from '../src/cli/ui/ink-app/tools/AgentTool/builtInAgents'

describe('gate defaults', () => {
  test('known gates return gizzi defaults, others the call-site fallback', () => {
    expect(gizziGateDefault('tengu_sedge_lantern', false)).toBe(true)
    expect(gizziGateDefault('tengu_passport_quail', false)).toBe(true)
    expect(gizziGateDefault('tengu_not_a_gate', 'fallback')).toBe('fallback')
  })

  test('GrowthBook getter uses the gizzi default when no remote value exists', () => {
    expect(getFeatureValue_CACHED_MAY_BE_STALE('tengu_sedge_lantern', false)).toBe(true)
    expect(
      getFeatureValue_CACHED_MAY_BE_STALE<{ enabled?: string }>('tengu_auto_mode_config', {})
        .enabled,
    ).toBe('enabled')
  })
})

describe('auto mode', () => {
  test('classifier prompt is fully assembled', () => {
    const prompt = buildDefaultExternalSystemPrompt()
    expect(prompt).toContain("safety classifier for gizzi's auto mode")
    expect(prompt).toContain('Irreversible local destruction')
    expect(prompt).not.toContain('<permissions_template>')
    expect(prompt).not.toMatch(/_to_replace>/)
    // replaceOutputFormatWithXml and the tool_use path both key on this line.
    expect(prompt.trimEnd().endsWith('Use the classify_result tool to report your classification.')).toBe(true)
  })

  test('default rules parse into the settings.autoMode shape', () => {
    const rules = getDefaultExternalAutoModeRules()
    expect(rules.allow.length).toBeGreaterThan(3)
    expect(rules.soft_deny.length).toBeGreaterThan(5)
    expect(rules.environment.length).toBeGreaterThan(0)
  })

  test('works with whichever model is chosen', () => {
    expect(modelSupportsAutoMode('claude-opus-5-5')).toBe(true)
    expect(modelSupportsAutoMode('claude-haiku-4-5-20251001')).toBe(true)
    expect(modelSupportsAutoMode('openrouter/z-ai/glm-4.7-flash')).toBe(true)
    expect(modelSupportsAutoMode('local-mlx/gemma')).toBe(true)
    expect(modelSupportsAutoMode('')).toBe(false)
  })
})

describe('small model for side calls', () => {
  let dir: string
  const saved = { ...process.env }
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'gizzi-small-model-'))
    process.env.GIZZI_CONFIG_DIR = dir
    delete process.env.ANTHROPIC_SMALL_FAST_MODEL
    delete process.env.NOKEY_API_KEY
  })
  afterEach(() => {
    // Restore in place: reassigning process.env detaches it from the real
    // environment, so later tests' env writes never reach child processes.
    for (const key of Object.keys(process.env)) if (!(key in saved)) delete process.env[key]
    Object.assign(process.env, saved)
    rmSync(dir, { recursive: true, force: true })
  })
  const writeConfig = (cfg: unknown) => writeFileSync(join(dir, 'gizzi.json'), JSON.stringify(cfg))
  const providers = {
    local: { options: { baseURL: 'http://localhost:9/v1' }, auth_type: 'none', models: {} },
    nokey: { options: { baseURL: 'https://example.invalid/v1' }, auth_type: 'api_key', models: {} },
  }

  test('uses gizzi.json small_model when its provider is usable', () => {
    writeConfig({ provider: providers, model: 'nokey/big', small_model: 'local/small' })
    expect(getSmallFastModel()).toBe('local/small')
  })

  test('falls back to a usable main model when small_model has no credentials', () => {
    writeConfig({ provider: providers, model: 'local/big', small_model: 'nokey/small' })
    expect(getSmallFastModel()).toBe('local/big')
  })

  test('falls back to Haiku when no provider model is usable', () => {
    writeConfig({ provider: providers, model: 'nokey/big', small_model: 'nokey/small' })
    expect(getSmallFastModel()).toContain('haiku')
  })
})

// The typed tool defs hide the (input, context) call signature; the cron
// tools never read context, so call them loosely.
type LooseTool = {
  validateInput: (input: unknown, ctx: unknown) => Promise<{ result: boolean }>
  call: (input: unknown, ctx: unknown) => Promise<{ data: any }>
}
const create = CronCreateTool as unknown as LooseTool
const del = CronDeleteTool as unknown as LooseTool
const list = CronListTool as unknown as LooseTool

describe('cron tools', () => {
  test('create, list, and delete a session-only job', async () => {
    const invalid = await create.validateInput({ cron: 'not a cron', prompt: 'x' }, {})
    expect(invalid.result).toBe(false)

    const created = await create.call(
      { cron: '7 9 * * 1-5', prompt: 'standup check', recurring: true, durable: false },
      {},
    )
    const id = created.data.id
    expect(created.data.durable).toBe(false)
    expect(created.data.nextFireAt).toBeGreaterThan(Date.now())

    const listed = await list.call({}, {})
    expect(listed.data.jobs.map((j: { id: string }) => j.id)).toContain(id)

    expect((await del.validateInput({ id }, {})).result).toBe(true)
    await del.call({ id }, {})
    const after = await list.call({}, {})
    expect(after.data.jobs.map((j: { id: string }) => j.id)).not.toContain(id)
  })
})

describe('built-in agents', () => {
  test('Explore and Plan are offered', () => {
    const types = getBuiltInAgents().map(a => a.agentType)
    expect(types).toContain('Explore')
    expect(types).toContain('Plan')
  })
})

describe('/fork', () => {
  test('asks for a fork through the Agent tool with the directive verbatim', async () => {
    const fork = (await import('../src/cli/ui/ink-app/commands/fork/index')).default
    const [block] = (await fork.getPromptForCommand('count the files  in src')) as Array<{ text: string }>
    expect(block.text).toContain('WITHOUT subagent_type')
    expect(block.text).toContain('<directive>\ncount the files  in src\n</directive>')
  })

  test('without a directive it only explains usage', async () => {
    const fork = (await import('../src/cli/ui/ink-app/commands/fork/index')).default
    const [block] = (await fork.getPromptForCommand('   ')) as Array<{ text: string }>
    expect(block.text).toContain('usage is /fork <directive>')
    expect(block.text).toContain('Do not call any tools')
  })
})

describe('terminal panel', () => {
  test('gate defaults on and each session gets its own gizzi-panel socket', async () => {
    expect(gizziGateDefault('tengu_terminal_panel', false)).toBe(true)
    const { getTerminalPanelSocket } = await import('../src/cli/ui/ink-app/utils/terminalPanel')
    expect(getTerminalPanelSocket()).toMatch(/^gizzi-panel-[0-9a-f-]{8}$/)
  })
})

describe('AST bash permission checks (TREE_SITTER_BASH)', () => {
  test('simple commands split; substitutions fail closed', async () => {
    const { parseCommandRaw } = await import('../src/cli/ui/ink-app/utils/bash/parser')
    const { parseForSecurityFromAst } = await import('../src/cli/ui/ink-app/utils/bash/ast')
    const check = async (cmd: string) => {
      const root = await parseCommandRaw(cmd)
      expect(root).not.toBeNull()
      return parseForSecurityFromAst(cmd, root as never)
    }
    const simple = await check('git status && git diff')
    expect(simple.kind).toBe('simple')
    expect(simple.kind === 'simple' && simple.commands.map(c => c.text)).toEqual(['git status', 'git diff'])
    expect((await check('echo $(rm -rf /)')).kind).toBe('too-complex')
    expect((await check('eval "$X"')).kind).toBe('too-complex')
  })
})

describe('shared memdir paths', () => {
  test('resolve to the same directories as src/memdir', async () => {
    const shared = await import('../src/shared/memdir/paths')
    const real = await import('../src/memdir/paths')
    expect(shared.getMemoryBaseDir()).toBe(real.getMemoryBaseDir())
    expect(shared.isAutoMemoryEnabled()).toBe(real.isAutoMemoryEnabled())
  })
})
