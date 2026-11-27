import { getCwd } from '../../utils/cwd.js'
import { useStartupNotification } from './useStartupNotification'

/**
 * Session start: sync skill growth with the server (about 2 s, in the
 * background; it never blocks the prompt) and, when skills the person
 * approved in a review screen wait to be turned on here, say so once.
 * Nothing is activated: /knowledge asks first (skill_manage).
 * A terminal session has no incognito mode, so nothing to skip here.
 */
export function useSkillGrowthNotification(): void {
  useStartupNotification(async () => {
    const [{ Instance }, { startupCheck }] = await Promise.all([
      import('@/runtime/context/project/instance'),
      import('@/runtime/skills/growth-person'),
    ])
    const text = await Instance.provide({ directory: getCwd(), fn: () => startupCheck() }).catch(() => undefined)
    return text ? { key: 'skill-growth-waiting', text, color: 'suggestion', priority: 'medium', timeoutMs: 15_000 } : null
  })
}
