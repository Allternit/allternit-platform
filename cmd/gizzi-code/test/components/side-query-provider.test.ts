import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdtempSync, rmSync, writeFileSync } from 'fs'
import { tmpdir } from 'os'
import { join } from 'path'
import { sideQuery } from '../../src/cli/ui/ink-app/utils/sideQuery'

// Auto mode's classifier (and other side queries) run on a provider model
// through its OpenAI-compatible endpoint, not the Anthropic client.
describe('sideQuery on a provider model', () => {
  let server: ReturnType<typeof Bun.serve>
  let dir: string
  const savedConfigDir = process.env.GIZZI_CONFIG_DIR
  const requests: Array<Record<string, any>> = []
  // Replies to send before the normal tool call (e.g. a reasoning cut-off).
  const queued: unknown[] = []

  beforeAll(() => {
    server = Bun.serve({
      port: 0,
      async fetch(req) {
        const url = new URL(req.url)
        if (url.pathname.endsWith('/models')) {
          return Response.json({ data: [{ id: 'other/model' }, { id: 'z-ai/glm-test' }] })
        }
        const body = await req.json()
        requests.push({ auth: req.headers.get('authorization'), body })
        if (queued.length) return Response.json(queued.shift())
        return Response.json({
          id: 'chatcmpl-1',
          choices: [{
            finish_reason: 'tool_calls',
            message: {
              content: null,
              tool_calls: [{ id: 'call_1', function: { name: 'classify_result', arguments: '{"shouldBlock":true,"reason":"deletes files"}' } }],
            },
          }],
          usage: { prompt_tokens: 120, completion_tokens: 14 },
        })
      },
    })
    dir = mkdtempSync(join(tmpdir(), 'gizzi-side-query-'))
    process.env.GIZZI_CONFIG_DIR = dir
    writeFileSync(join(dir, 'gizzi.json'), JSON.stringify({
      provider: { fakeroute: { options: { baseURL: `http://localhost:${server.port}/v1`, apiKey: 'sk-test' }, models: {} } },
    }))
  })

  afterAll(() => {
    server.stop(true)
    rmSync(dir, { recursive: true, force: true })
    if (savedConfigDir === undefined) delete process.env.GIZZI_CONFIG_DIR
    else process.env.GIZZI_CONFIG_DIR = savedConfigDir
  })

  test('forced tool call maps to a tool_use block', async () => {
    const result = (await sideQuery({
      querySource: 'auto_mode' as any,
      model: 'fakeroute/z-ai/glm-test',
      system: 'You are a safety classifier.',
      skipSystemPromptPrefix: true,
      messages: [{ role: 'user', content: [{ type: 'text', text: 'rm -rf build' }] }],
      tools: [{ name: 'classify_result', description: 'Report', input_schema: { type: 'object', properties: { shouldBlock: { type: 'boolean' } } } }],
      tool_choice: { type: 'tool', name: 'classify_result' },
      max_tokens: 256,
    })) as unknown as {
      stop_reason: string
      content: Array<{ type: string; name?: string; input?: Record<string, unknown> }>
      usage: { input_tokens: number }
    }

    const req = requests.at(-1)!
    expect(req.auth).toBe('Bearer sk-test')
    expect(req.body.model).toBe('z-ai/glm-test')
    expect(req.body.tool_choice).toEqual({ type: 'function', function: { name: 'classify_result' } })
    expect(req.body.tools[0].function.name).toBe('classify_result')
    // Like the main loop, the system prompt folds into the first user turn
    // (local chat templates reject a system role).
    expect(req.body.messages).toEqual([{ role: 'user', content: 'You are a safety classifier.\n\nrm -rf build' }])

    expect(result.stop_reason).toBe('tool_use')
    const block = result.content.find(b => b.type === 'tool_use')!
    expect(block.name).toBe('classify_result')
    expect(block.input).toEqual({ shouldBlock: true, reason: 'deletes files' })
    expect(result.usage.input_tokens).toBe(120)
  })
  const classify = (thinking?: number | false) =>
    sideQuery({
      querySource: 'auto_mode' as any,
      model: 'fakeroute/z-ai/glm-test',
      messages: [{ role: 'user', content: 'rm -rf build' }],
      tools: [{ name: 'classify_result', input_schema: { type: 'object' } }],
      tool_choice: { type: 'tool', name: 'classify_result' },
      max_tokens: 4096,
      thinking,
    }) as unknown as Promise<{ content: Array<{ type: string }> }>

  test('asks the provider to skip reasoning when thinking is off', async () => {
    await classify(false)
    expect(requests.at(-1)!.body.reasoning).toEqual({ enabled: false })
  })

  test('retries once with room to answer when reasoning used up the budget', async () => {
    queued.push({ choices: [{ finish_reason: 'length', message: { content: null, reasoning: 'thinking…' } }] })
    const before = requests.length
    const result = await classify(false)
    expect(requests.length - before).toBe(2)
    expect(requests.at(-1)!.body.max_tokens).toBe(4096 + 16_384)
    expect(requests.at(-1)!.body.reasoning).toEqual({ effort: 'low' })
    expect(result.content.some(b => b.type === 'tool_use')).toBe(true)
  })
})
