// Who may talk to the bot through this number. Defaults are the safe ones:
// owner-only DMs (everyone else needs pairing approval) and mention-required groups.

export const WARNING =
  'Unofficial: this links your personal WhatsApp number as a linked device through an unofficial library. ' +
  'WhatsApp can ban numbers that automate messaging, and a ban cannot be undone by Allternit. ' +
  'Use a dedicated number you can afford to lose, not your main one.';

/** `1234:5@s.whatsapp.net` / `1234@lid` -> user part only, so device suffixes compare equal. */
export function bare(jid) {
  return String(jid ?? '').split('@')[0].split(':')[0];
}

export const isGroup = (jid) => String(jid ?? '').endsWith('@g.us');

export function textOf(msg) {
  const m = msg?.message;
  if (!m) return '';
  return m.conversation ?? m.extendedTextMessage?.text ?? m.imageMessage?.caption ?? m.videoMessage?.caption ?? '';
}

function mentioned(msg, own) {
  const ids = msg.message?.extendedTextMessage?.contextInfo?.mentionedJid ?? [];
  const mine = new Set(own.map(bare).filter(Boolean));
  return ids.some((j) => mine.has(bare(j)));
}

/**
 * Decide what to do with one Baileys message.
 * -> { action: 'forward' | 'ignore' | 'pairing', reason }
 */
export function decide(msg, { own, allow, pairing }) {
  const chat = msg.key?.remoteJid;
  if (!chat || chat === 'status@broadcast' || chat.endsWith('@broadcast') || chat.endsWith('@newsletter')) return { action: 'ignore', reason: 'broadcast' };
  if (!textOf(msg).trim()) return { action: 'ignore', reason: 'no_text' };
  const ownIds = own.map(bare);
  if (isGroup(chat)) {
    const sender = msg.key.participant ?? '';
    if (msg.key.fromMe) return { action: 'ignore', reason: 'own_message' };
    if (!mentioned(msg, own)) return { action: 'ignore', reason: 'group_mention_required' };
    if (!allow.has(bare(sender)) && !ownIds.includes(bare(sender))) return { action: 'pairing', reason: 'group_sender_not_approved', jid: sender };
    return { action: 'forward', reason: 'group_mention' };
  }
  // DM. The owner's "message yourself" chat is the owner talking to the bot.
  if (msg.key.fromMe) return ownIds.includes(bare(chat)) ? { action: 'forward', reason: 'self_chat' } : { action: 'ignore', reason: 'own_message' };
  if (ownIds.includes(bare(chat)) || allow.has(bare(chat))) return { action: 'forward', reason: 'approved_dm' };
  return { action: 'pairing', reason: 'dm_not_approved', jid: chat };
}
