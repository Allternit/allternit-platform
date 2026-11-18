/**
 * TUI parity features switched on in script/features.mjs (2026-09-26):
 * gate defaults, auto-mode classifier prompt + model allowlist, small-model
 * resolution for side calls, and the automation tool's in-session queue. Run with the shipped flags
 * (script/ci-smoke-test.sh passes them as --feature=…).
 */
import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'fs'
import { tmpdir } from 'os'
import { join } from 'path'
import { gizziGateDefault } from '../src/constants/gizziGates'
import { getFeatureValue_CACHED_MAY_BE_STALE } from '../src/cli/ui/ink-app/services/analytics/growthbook'
import { modelSupportsAutoMode } from '../src/cli/ui/ink-app/utils/betas'
import '../src/cli/devMacro' // getSystemPrompt reads the build-time MACRO global
import { getSystemPrompt } from '../src/cli/ui/ink-app/constants/prompts'
import {
  buildDefaultExternalSystemPrompt,
  getDefaultExternalAutoModeRules,
} from '../src/cli/ui/ink-app/utils/permissions/yoloClassifier'
import { getSmallFastModel } from '../src/cli/ui/ink-app/utils/model/model'
import { AutomationTool } from '../src/cli/ui/ink-app/tools/AutomationTool/AutomationTool'
import { getProjectRoot, setProjectRoot } from '../src/cli/ui/ink-app/bootstrap/state'
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

describe('system prompt vendor strip', () => {
  test('default external prompts name no outside vendor', async () => {
    const classifier = buildDefaultExternalSystemPrompt()
    // The REPL prompt lists commands, which needs a credential to be configured (any value).
    process.env.GIZZI_CODE_OAUTH_TOKEN ??= 'test-token'
    const repl = (await getSystemPrompt([] as never, 'gizzi-test-model')).join('\n')
    for (const text of [classifier, repl]) {
      expect(text).not.toMatch(/Claude|Anthropic|Allternit Shell/)
    }
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

// The typed tool def hides the (input, context) call signature; the paths
// tested here never read context, so call it loosely.
type LooseTool = {
  validateInput: (input: unknown, ctx: unknown) => Promise<{ result: boolean; message?: string }>
  call: (input: unknown, ctx: unknown) => Promise<{ data: any }>
}
const automation = AutomationTool as unknown as LooseTool

describe('automation tool over the old cron file store', () => {
  const legacy = { id: '0a1b2c3d', cron: '7 9 * * 1-5', prompt: 'standup check', createdAt: Date.now(), recurring: true }
  let dir: string
  let file: string
  let savedRoot: string
  const savedFetch = globalThis.fetch
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'gizzi-automation-'))
    mkdirSync(join(dir, '.claude'))
    file = join(dir, '.claude', 'scheduled_tasks.json')
    writeFileSync(file, JSON.stringify({ tasks: [legacy] }, null, 2) + '\n')
    savedRoot = getProjectRoot()
    setProjectRoot(dir)
    // No account and no engine on this machine: both are network edges.
    globalThis.fetch = (async () => {
      throw new TypeError('fetch failed')
    }) as unknown as typeof fetch
  })
  afterEach(() => {
    globalThis.fetch = savedFetch
    setProjectRoot(savedRoot)
    rmSync(dir, { recursive: true, force: true })
  })
  const ids = (data: { rows: Array<{ id: string; where: string }> }) => data.rows.map(r => `${r.where}:${r.id}`)

  test("an old task lists, can't be changed, and deletes from the file", async () => {
    const listed = (await automation.call({ action: 'list' }, {})).data
    expect(ids(listed)).toEqual([`legacy:${legacy.id}`])
    expect(listed.text).toContain(legacy.id)
    // The account and the computer scheduler are unreachable: said, not hidden.
    expect(listed.note).toContain("aren't listed")

    const update = await automation.validateInput({ action: 'update', id: legacy.id, title: 'x' }, {})
    expect(update.result).toBe(false)
    expect(update.message).toContain("can't be changed")

    const deleted = (await automation.call({ action: 'delete', id: legacy.id }, {})).data
    expect(deleted.where).toBe('legacy')
    expect(JSON.parse(readFileSync(file, 'utf8')).tasks).toEqual([])
    expect(ids((await automation.call({ action: 'list' }, {})).data)).toEqual([])
  })

  test('a new this-thread task stays in the session and never touches the file', async () => {
    const before = readFileSync(file, 'utf8')
    const created = (
      await automation.call({ action: 'create', prompt: 'check the deploy', schedule: { delay_minutes: 30 } }, {})
    ).data
    expect(created.status).toBe('ok')
    expect(created.where).toBe('session')
    expect(created.text).toContain('ends with the session')
    expect(readFileSync(file, 'utf8')).toBe(before)

    const listed = (await automation.call({ action: 'list' }, {})).data
    const session = listed.rows.find((r: { where: string }) => r.where === 'session')
    expect(session.title).toBe('check the deploy')
    expect(new Date(session.nextRunAt).getTime()).toBeGreaterThan(Date.now() + 29 * 60_000)

    await automation.call({ action: 'delete', id: session.id }, {})
    expect(ids((await automation.call({ action: 'list' }, {})).data)).toEqual([`legacy:${legacy.id}`])
    expect(readFileSync(file, 'utf8')).toBe(before)
  })

  test('a new-session task with no account and no engine is not scheduled anywhere', async () => {
    const before = readFileSync(file, 'utf8')
    const created = (
      await automation.call(
        { action: 'create', prompt: 'weekly brief', target: 'new_session', schedule: { cron: '0 9 * * 1' } },
        {},
      )
    ).data
    expect(created.status).toBe('no_scheduler')
    expect(created.text).toContain('Not scheduled')
    expect(readFileSync(file, 'utf8')).toBe(before)
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
