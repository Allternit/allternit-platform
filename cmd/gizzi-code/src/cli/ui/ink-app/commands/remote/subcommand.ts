/**
 * /remote owns session, desktop, mobile and native (overhaul plan D4):
 * `/remote <view> [args]` runs the old command; the old names stay as hidden
 * aliases for one release.
 */
export type RemoteView = 'session' | 'desktop' | 'mobile' | 'native'

const WORDS: Readonly<Record<string, RemoteView>> = {
  session: 'session',
  url: 'session',
  desktop: 'desktop',
  app: 'desktop',
  mobile: 'mobile',
  ios: 'mobile',
  android: 'mobile',
  native: 'native',
  'cli-session': 'native',
}

export function remoteSubcommand(args: string | undefined): { view: RemoteView; rest: string } | undefined {
  const trimmed = (args ?? '').trim()
  const [word = '', ...rest] = trimmed.split(/\s+/)
  const view = WORDS[word.toLowerCase()]
  return view ? { view, rest: rest.join(' ') } : undefined
}
