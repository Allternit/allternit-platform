import chalk from '@/shared/util/chalk'
import * as React from 'react'
import { useEffect, useState } from 'react'
import { RuntimeLocked, RuntimePicker, type RuntimePick } from '../../components/RuntimePicker'
import { COMMON_HELP_ARGS } from '../../constants/xml'
import { getActiveBotChat, setActiveBotChat } from '../../bootstrap/state'
import { Text } from '../../ink'
import { useAppState, useSetAppState } from '../../state/AppState'
import { persistModelPick } from '../../state/onChangeAppState'
import type { LocalJSXCommandCall, LocalJSXCommandOnDone } from '../../types/command'
import { getDefaultMainLoopModel } from '../../utils/model/model'
import { getBot, type Bot } from '@/runtime/bots/bot-store'
import { listRuntimes, resolveRuntime, type BotBrainLike, type RuntimeEntry } from '@/runtime/providers/discovery/runtime-registry'

// The record rules live in `gizzi agents bot edit --runtime/--brain`; loaded
// lazily so the TUI's command table does not pull the bot CLI in at start.
const botCli = () => import('@/cli/commands/bot')

/** provider/model, or null for an alias or no pick (the auto-switch needs a provider). */
function asRef(model: string | null | undefined): string | null {
  return model && model.indexOf('/') > 0 ? model : null
}

/** The runtime a plain session runs on: the CLI its brain belongs to, else Allternit. */
function sessionRuntime(model: string | null): string {
  const provider = model?.split('/')[0]
  return listRuntimes().find(r => r.tool && r.tool === provider)?.id ?? 'allternit'
}

type BotState =
  | { status: 'none' }
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'connected'; bot: Bot; vendor: string }
  | { status: 'ready'; bot: Bot }

function RuntimeCommand({ onDone, focusRuntime }: { onDone: LocalJSXCommandOnDone; focusRuntime?: string }) {
  const chat = getActiveBotChat()
  const mainLoopModel = useAppState(s => s.mainLoopModel)
  const setAppState = useSetAppState()
  const [state, setState] = useState<BotState>(chat ? { status: 'loading' } : { status: 'none' })

  useEffect(() => {
    if (!chat) return
    let cancelled = false
    void (async () => {
      const bot = await getBot(chat.botName)
      if (!bot) return setState({ status: 'error', message: `bot '${chat.botName}' not found` })
      const { connectedVendor } = await botCli()
      const vendor = await connectedVendor(bot)
      if (cancelled) return
      setState(vendor ? { status: 'connected', bot, vendor } : { status: 'ready', bot })
    })().catch(e => {
      if (!cancelled) setState({ status: 'error', message: (e as Error).message })
    })
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    if (state.status === 'error') onDone(`Could not open /runtime: ${state.message}`, { display: 'system' })
  }, [state])

  const [helpers, setHelpers] = useState<Awaited<ReturnType<typeof botCli>>>()
  useEffect(() => {
    void botCli().then(setHelpers).catch(e => onDone(`Could not open /runtime: ${(e as Error).message}`, { display: 'system' }))
  }, [])

  if (state.status === 'loading' || !helpers) return <Text dimColor={true}>Loading runtimes…</Text>
  if (state.status === 'error') return null
  if (state.status === 'connected') {
    return <RuntimeLocked botName={state.bot.name} vendor={state.vendor} onDone={() => onDone(`${state.bot.name} is run by ${state.vendor}.`, { display: 'system' })} />
  }

  const bot = state.status === 'ready' ? state.bot : null
  const currentBrain = bot ? asRef(bot.model ?? bot.botBrain?.modelRef) : asRef(mainLoopModel) ?? asRef(getDefaultMainLoopModel())
  const currentRuntime = bot ? resolveRuntime((bot.botBrain ?? {}) as BotBrainLike).runtime : sessionRuntime(currentBrain)

  // Same auto-switch rule as `bot edit --runtime`: keep a brain that fits,
  // else the runtime CLI's default model, with the one-line note.
  const planRuntime = (r: RuntimeEntry) => {
    const change = helpers.applyRuntimeBrain({ prev: bot?.botBrain ?? null, currentModel: currentBrain, runtime: r.id })
    return { brain: change.model !== undefined ? change.model : currentBrain, note: change.note }
  }

  const handleSelect = async ({ runtime, brain, note }: RuntimePick) => {
    const suffix = note ? `\n${note}` : ''
    if (bot) {
      try {
        // The record write `gizzi agents bot edit --runtime --brain` does.
        const change = helpers.applyRuntimeBrain({ prev: bot.botBrain, currentModel: bot.model, runtime: runtime.id, brain })
        const { updateBot } = await import('@/runtime/bots/bot-store')
        const saved = await updateBot(bot.name, { model: change.model, botBrain: change.botBrain })
        const live = getActiveBotChat()
        if (live && live.botName === chat?.botName) setActiveBotChat({ ...live, model: saved.model ?? null })
        onDone(`${saved.name} now works with ${chalk.bold(runtime.name)} and thinks with ${chalk.bold(saved.model ?? brain)}.${change.note && change.note !== note ? `\n${change.note}` : suffix}`)
      } catch (e) {
        onDone(`Could not save: ${(e as Error).message}`, { display: 'system' })
      }
      return
    }
    // A plain session: the brain is set the way /model sets it. The session
    // has no runtime field of its own; the runtime is the brain's CLI.
    if (brain === mainLoopModel) persistModelPick(brain)
    setAppState(prev => ({ ...prev, mainLoopModel: brain, mainLoopModelForSession: null }))
    onDone(`Runtime ${chalk.bold(runtime.name)} · brain ${chalk.bold(brain)}${suffix}`)
  }

  return (
    <RuntimePicker
      initialRuntime={currentRuntime}
      focusRuntime={focusRuntime}
      initialBrain={currentBrain}
      subject={bot?.title || bot?.name || 'This session'}
      planRuntime={planRuntime}
      onSelect={pick => void handleSelect(pick)}
      onCancel={() => onDone('Kept the runtime and brain', { display: 'system' })}
    />
  )
}

export const call: LocalJSXCommandCall = async (onDone, _context, args) => {
  args = args?.trim() || ''
  if (COMMON_HELP_ARGS.includes(args)) {
    onDone('Run /runtime to choose the runtime (how it works), then the brain (how it thinks). /runtime <id> starts on that runtime. List them with gizzi runtime catalog --all.', {
      display: 'system',
    })
    return
  }
  if (args) {
    const { closeRuntimeIds } = await botCli()
    const { getRuntime } = await import('@/runtime/providers/discovery/runtime-registry')
    const r = getRuntime(args)
    if (!r) {
      const close = closeRuntimeIds(args)
      onDone(`Unknown runtime '${args}'.${close.length ? ` Did you mean ${close.join(', ')}?` : ''} See gizzi runtime catalog --all.`, { display: 'system' })
      return
    }
    return <RuntimeCommand onDone={onDone} focusRuntime={r.id} />
  }
  return <RuntimeCommand onDone={onDone} />
}
