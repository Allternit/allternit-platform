/** App-owned turn routing and final-output validation; shared with the runtime. */
import { CATALOG_TEMPLATES } from './answer-catalog';
import { parseAnswerEnvelope, type AnswerTemplateId } from './answer-contract';

export const ANSWER_POLICY_VERSION = 1;
export const ANSWER_POLICY_MAX_CHARS = 66000;
const core = [
  ['quick-answer', 'Quick answer'], ['comparison', 'Comparison'], ['decision', 'Decision'],
  ['plan', 'Plan'], ['how-to', 'How-to'], ['troubleshooting', 'Troubleshooting'],
  ['status-report', 'Status report'], ['research-brief', 'Research brief'],
  ['table', 'Table'], ['chart', 'Chart'], ['form', 'Form'], ['calculator', 'Calculator'],
] as const;
export const ANSWER_POLICY_TEMPLATES = [...core.map(([id, name]) => ({ id, name })), ...CATALOG_TEMPLATES.map(({ id, name }) => ({ id, name }))];
const normalize = (text: string) => text.toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim();
const contains = (text: string, phrase: string) => ` ${text} `.includes(` ${normalize(phrase)} `);
const aliases: Record<string, string[]> = {
  weather: ['weather', 'temperature forecast'], stock: ['stock price', 'stock summary'],
  'flight-options': ['flights', 'flight options'], 'hotel-options': ['hotels', 'hotel options'],
  'restaurant-results': ['restaurants'], 'transaction-ledger': ['transactions', 'transaction history'],
  'reading-list': ['reading list', 'book recommendations'], 'product-grid': ['products', 'product catalog'],
  'shipment-tracker': ['shipment status', 'track my package'], 'calendar-agenda': ['my agenda', 'my calendar'],
  'task-board': ['task board', 'kanban'], 'agent-roster': ['my agents', 'agent roster'],
  'connector-health': ['connector health', 'connection status'], 'repo-review': ['repository review', 'review this repository'],
  'pdf-analysis': ['document analysis', 'analyze this pdf'], 'media-shotlist': ['shot list', 'shotlist'],
  'invoice-review': ['invoices', 'invoice review'], 'quote-review': ['quote review', 'review this quote'],
  'budget-usage': ['budget usage', 'budget remaining'], 'file-browser': ['files', 'file results'],
  'incident-report': ['incident summary', 'incident report'], 'cost-breakdown': ['cost breakdown', 'cost report'],
  calculator: ['calculator', 'split the bill', 'calculate'], table: ['data table'], chart: ['chart', 'plot'],
  form: ['input form'], comparison: ['compare', 'versus'], decision: ['help me choose', 'help me decide'],
  plan: ['make a plan', 'create a plan', 'project plan'], 'how-to': ['step by step instructions'],
  troubleshooting: ['troubleshooting guide'], 'research-brief': ['research brief'],
};
export type AnswerPolicy =
  | { version: 1; mode: 'text'; reason: 'text-only' | 'artifact-output' | 'structured-output' | 'conversation' }
  | { version: 1; mode: 'template'; template: AnswerTemplateId; reason: 'explicit' | 'job' };

export function selectAnswerPolicy(message: string, options: { visuals: boolean; artifactOutput?: boolean; structuredOutput?: boolean }): AnswerPolicy {
  const text = message.slice(0, 8000);
  const exempt = (reason: Extract<AnswerPolicy, { mode: 'text' }>['reason']): AnswerPolicy => ({ version: 1, mode: 'text', reason });
  if (!options.visuals || /\b(?:plain text|text only|no cards|without cards)\b/i.test(text)) return exempt('text-only');
  if (options.artifactOutput) return exempt('artifact-output');
  if (options.structuredOutput) return exempt('structured-output');
  // Ignore examples and quoted documents when interpreting a turn's UI request.
  const request = text.replace(/```[\s\S]*?(?:```|$)/g, '').replace(/^\s*>.*$/gm, '');
  const explicit = request.match(/\b(?:use|render|show|select)\s+(?:the\s+)?(?:answer\s+)?template\s*:?\s*["'`]?([a-z][a-z0-9-]*)|\btemplate\s*:\s*["'`]?([a-z][a-z0-9-]*)/i);
  const named = explicit?.[1]?.toLowerCase() ?? explicit?.[2]?.toLowerCase();
  if (named && ANSWER_POLICY_TEMPLATES.some(t => t.id === named)) return { version: 1, mode: 'template', template: named as AnswerTemplateId, reason: 'explicit' };
  const normalized = normalize(request);
  const naturalExplicit = ANSWER_POLICY_TEMPLATES.find(t => [t.id, t.name].some(label =>
    ['use', 'render', 'show', 'select'].some(verb => contains(normalized, `${verb} ${label} template`) || contains(normalized, `${verb} the ${label} template`))));
  if (naturalExplicit) return { version: 1, mode: 'template', template: naturalExplicit.id as AnswerTemplateId, reason: 'explicit' };
  if (/\b(?:implement|refactor|debug|fix|css|component|write code)\b/.test(normalized)) return exempt('conversation');
  if (!/\b(?:show|display|list|compare|summarize|calculate|track|find|give|make|create|prepare|review|analyze|check|plan)\b/.test(normalized) && !/^(?:what is|what s|how is|how s) (?:the )?(?:weather|temperature|stock price)\b/.test(normalized)) return exempt('conversation');
  const matches = ANSWER_POLICY_TEMPLATES.flatMap(t => {
    // Names give every catalog template a deterministic natural-language route.
    const phrases = [...(aliases[t.id] ?? []), ...(core.some(([id]) => id === t.id) ? [] : [t.name, t.id])];
    return phrases.filter(phrase => contains(normalized, phrase)).map(phrase => ({ id: t.id, phrase, priority: core.some(([id]) => id === t.id) ? 1 : 0 }));
  }).sort((a, b) => a.priority - b.priority || normalize(b.phrase).length - normalize(a.phrase).length || a.id.localeCompare(b.id));
  return matches.length ? { version: 1, mode: 'template', template: matches[0].id as AnswerTemplateId, reason: 'job' } : exempt('conversation');
}

export function answerPolicyInstruction(policy: AnswerPolicy): string {
  if (policy.mode === 'text') return '';
  return `Required answer template (policy v1): ${policy.template}. Return exactly one fenced openui JSON envelope, version 1, with template "${policy.template}". No prose or other fences outside it. The application validates the completed response before showing it. Gather missing facts first; never invent data. This turn rule takes precedence over optional card suggestions.`;
}

export function checkAnswerResponse(policy: AnswerPolicy, response: string): { valid: boolean; problems: string[]; returned?: string } {
  if (policy.mode === 'text') return { valid: true, problems: [] };
  if (response.length > ANSWER_POLICY_MAX_CHARS) return { valid: false, problems: ['Response exceeds the answer limit.'] };
  const fence = response.trim().match(/^```openui(?:-lang)?[ \t]*\r?\n([\s\S]*?)\r?\n```$/);
  if (!fence) return { valid: false, problems: [`Return only one complete openui envelope for ${policy.template}.`] };
  const parsed = parseAnswerEnvelope(fence[1]);
  if (!parsed.answer) return { valid: false, problems: parsed.problems.map(p => p.slice(0, 256)) };
  const returned = parsed.answer.template;
  return returned === policy.template ? { valid: true, returned, problems: [] } : { valid: false, returned, problems: [`Expected ${policy.template}; received ${returned}.`] };
}
