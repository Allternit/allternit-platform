/** Turn boundary enforcement. Only validated answer text reaches public parts. */
import { createHash } from 'node:crypto';
import { ANSWER_POLICY_MAX_CHARS, answerPolicyInstruction, checkAnswerResponse, selectAnswerPolicy, type AnswerPolicy } from '../integrations/artifacts/answer-policy';
import { MessageV2 } from './message-v2';
import { Session } from './index';
import { SessionTrace } from './trace';
import { SecretRedactor } from './environment/redactor';
import { Bus } from '@/shared/bus';
import { Identifier } from '@/shared/id/id';

export class AnswerTurnGuard {
  readonly policy: AnswerPolicy;
  readonly system: string | undefined;
  private held = new Map<string, MessageV2.TextPart>();
  private problems: string[] = [];
  private attempts = 0;
  private repairs = 0;
  needsRepair = false;
  constructor(readonly userID: string, message: string, user: MessageV2.User) {
    this.system = user.system;
    this.policy = selectAnswerPolicy(message, {
      visuals: !!user.system?.includes('## Enforced answer templates'),
      artifactOutput: /For this turn the user (?:picked Output|asked for an artifact)/.test(user.system ?? ''),
      structuredOutput: user.format?.type === 'json_schema',
    });
  }
  recordSelection(sessionID: string) {
    SessionTrace.append({ sessionID, messageID: this.userID, kind: 'answer.policy', data: {
      version: this.policy.version, mode: this.policy.mode, reason: this.policy.reason,
      ...(this.policy.mode === 'template' ? { selected: this.policy.template } : {}),
    } });
  }
  get required() { return this.policy.mode === 'template'; }
  instruction() {
    return answerPolicyInstruction(this.policy) + (this.needsRepair ? `\nFormatting correction: ${this.problems.join(' ')}\nUse the original request and already gathered facts. Return only the required envelope. Do not repeat tool actions.` : '');
  }
  hold(part: MessageV2.TextPart): MessageV2.TextPart {
    this.held.set(part.id, { ...part, text: part.text.slice(0, ANSWER_POLICY_MAX_CHARS + 1) });
    return { ...part, text: '', metadata: { ...part.metadata, answerPolicy: { status: 'pending' } } };
  }
  context(messages: MessageV2.WithParts[]): MessageV2.WithParts[] {
    return messages.map(message => ({ ...message, parts: message.parts.map(part => part.type === 'text' && !part.ignored ? this.held.get(part.id) ?? part : part) }));
  }
  text(messageID: string) { return [...this.held.values()].filter(part => part.messageID === messageID).map(part => part.text).join('\n').trim(); }
  async restart(messageID: string, sessionID: string) {
    const previous = [...this.held.values()].filter(part => part.messageID === messageID);
    if (!previous.length) return;
    const text = SecretRedactor.text(this.text(messageID));
    SessionTrace.append({ sessionID, messageID, kind: 'answer.validation', data: {
      version: 1, selected: this.policy.mode === 'template' ? this.policy.template : undefined,
      status: 'rejected', attempt: this.attempts + 1, interrupted: true, restarted: true,
      problems: ['Provider request restarted before answer validation.'],
      rejectedText: text.slice(0, ANSWER_POLICY_MAX_CHARS), truncated: text.length > ANSWER_POLICY_MAX_CHARS,
      responseHash: createHash('sha256').update(text.slice(0, ANSWER_POLICY_MAX_CHARS)).digest('hex'),
    } });
    for (const part of previous) {
      this.held.delete(part.id);
      const stored = await Session.updatePart({ ...part, text: '', ignored: true, metadata: { ...part.metadata, answerPolicy: { status: 'rejected', restarted: true } } });
      Bus.publish(MessageV2.Event.PartUpdated, { part: stored });
    }
  }
  async interrupted(message: MessageV2.Assistant, reason: string) {
    const record = { version: 1 as const, selected: this.policy.mode === 'template' ? this.policy.template : undefined, status: 'rejected' as const, attempt: this.attempts + 1, problems: [reason] };
    message.answerPolicy = record;
    this.needsRepair = false;
    const text = SecretRedactor.text(this.text(message.id));
    SessionTrace.append({ sessionID: message.sessionID, messageID: message.id, kind: 'answer.validation', data: {
      ...record, interrupted: true, rejectedText: text.slice(0, ANSWER_POLICY_MAX_CHARS),
      truncated: text.length > ANSWER_POLICY_MAX_CHARS,
      responseHash: createHash('sha256').update(text.slice(0, ANSWER_POLICY_MAX_CHARS)).digest('hex'),
    } });
    for (const part of this.held.values()) if (part.messageID === message.id) {
      const stored = await Session.updatePart({ ...part, text: '', ignored: true, metadata: { ...part.metadata, answerPolicy: record } });
      Bus.publish(MessageV2.Event.PartUpdated, { part: stored });
    }
    await Session.updateMessage(message);
  }
  async complete(message: MessageV2.Assistant, allowRepair = true): Promise<'accepted' | 'repairing' | 'rejected'> {
    const parts = [...this.held.values()].filter(part => part.messageID === message.id);
    const text = SecretRedactor.text(this.text(message.id));
    const checked = checkAnswerResponse(this.policy, text);
    this.attempts++;
    const status = checked.valid ? 'accepted' : allowRepair && this.repairs < 1 ? 'repairing' : 'rejected';
    if (status === 'repairing') this.repairs++;
    const record = { version: 1 as const, selected: this.policy.mode === 'template' ? this.policy.template : undefined, returned: checked.returned, status, attempt: this.attempts, problems: checked.problems };
    SessionTrace.append({ sessionID: message.sessionID, messageID: message.id, kind: 'answer.validation', data: SecretRedactor.deep({
      ...record, responseHash: createHash('sha256').update(text.slice(0, ANSWER_POLICY_MAX_CHARS)).digest('hex'),
      ...(!checked.valid ? { rejectedText: text.slice(0, ANSWER_POLICY_MAX_CHARS), truncated: text.length > ANSWER_POLICY_MAX_CHARS } : {}),
    }) });
    this.needsRepair = status === 'repairing';
    this.problems = checked.problems;
    message.answerPolicy = record;
    const visible = checked.valid ? text : status === 'rejected' ? `The ${record.selected} answer could not be validated. Please try again or choose Text only.` : '';
    // One public text part also handles providers that split a JSON fence over blocks.
    const first = parts[0] ?? { id: Identifier.ascending('part'), messageID: message.id, sessionID: message.sessionID, type: 'text' as const, text: '' };
    for (const [index, part] of [first, ...parts.slice(1)].entries()) {
      const stored = await Session.updatePart({ ...part, text: index === 0 ? visible : '', ignored: index > 0 || status === 'repairing', metadata: { ...part.metadata, answerPolicy: record } });
      Bus.publish(MessageV2.Event.PartUpdated, { part: stored });
    }
    if (status === 'repairing') message.finish = 'unknown';
    if (status === 'rejected') {
      message.finish = 'stop';
      message.error = { name: 'AnswerTemplateError', message: `Required template ${record.selected} did not pass validation${this.repairs ? ' after one correction' : ''}.`, retries: this.repairs };
    }
    await Session.updateMessage(message);
    if (status === 'rejected') Bus.publish(Session.Event.Error, { sessionID: message.sessionID, error: message.error! });
    return status;
  }
}
