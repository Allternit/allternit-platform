export const BOT_COMPUTER_DETACHED_SURFACE = 'bot-computer';

export type BotComputerWindowOptions = {
  botId: string;
  title?: string;
  sandboxId?: string;
};

export function buildBotComputerWindowUrl(
  platformUrl: string,
  options: BotComputerWindowOptions,
): string {
  if (!options?.botId) throw new Error('A bot ID is required');
  const url = new URL('/bot-computer', platformUrl);
  url.searchParams.set('botId', options.botId);
  if (options.title) url.searchParams.set('title', options.title);
  if (options.sandboxId) url.searchParams.set('sandboxId', options.sandboxId);
  return url.toString();
}

export function isBotComputerWindowUrl(url: URL): boolean {
  if (url.pathname === '/bot-computer' && Boolean(url.searchParams.get('botId'))) {
    return true;
  }
  return (
    (url.pathname === '/platform' || url.pathname === '/shell') &&
    url.searchParams.get('detachedSurface') === BOT_COMPUTER_DETACHED_SURFACE &&
    Boolean(url.searchParams.get('botId'))
  );
}

/** The bot phone in its own window (`/bot-phone?botId=`), opened by "Pop out". */
export function buildBotPhoneWindowUrl(platformUrl: string, botId: string): string {
  if (!botId) throw new Error('A bot ID is required');
  const url = new URL('/bot-phone', platformUrl);
  url.searchParams.set('botId', botId);
  return url.toString();
}

export function isBotPhoneWindowUrl(url: URL): boolean {
  return url.pathname === '/bot-phone' && Boolean(url.searchParams.get('botId'));
}

export const BOT_PHONE_WINDOW_SIZE = { width: 420, height: 880, minWidth: 360, minHeight: 560 };
