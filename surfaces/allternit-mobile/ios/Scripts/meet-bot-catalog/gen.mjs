const m = await import('./bundle.mjs');
const drop = (o) => Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined));
function wire(input) {
  const config = { ...(input.config || {}), ...(input.avatar ? { avatar: input.avatar } : {}), ...(input.brainId ? { brainId: input.brainId } : {}), ...(input.brain ? { botBrain: input.brain } : {}) };
  const api = drop({
    name: input.name, description: input.description, type: input.type || 'worker', model: input.model, provider: input.provider,
    capabilities: input.capabilities || [], system_prompt: input.systemPrompt, tools: input.tools || [], max_iterations: input.maxIterations || 10,
    temperature: input.temperature ?? 0.7, config, avatar: input.avatar ? JSON.stringify(input.avatar) : undefined, character_json: input.characterLayer,
    trust_tier: input.trustTier, harness_config: input.harness, enabled_modes: input.allowedSurfaces || ['chat'], allowed_skills: input.allowedSkills,
    allowed_tools: input.allowedTools, category: input.category, tags: input.tags, data_classification: input.dataClassification, write_scope: input.writeScope,
    is_bot: input.isBot, bot_profile: input.botProfile, vm_operator: input.vmOperator,
  });
  if (input.isBot) api.config = { ...api.config, isBot: true, botProfile: input.botProfile, ...(input.brain ? { botBrain: input.brain } : {}) };
  if (input.vmOperator) api.config = { ...api.config, vmOperator: input.vmOperator };
  return api;
}
const M = 'kimi/kimi-for-coding', P = 'custom';
function placeholder(v) {
  if (typeof v === 'string') return v === M ? '__MODEL__' : v === P ? '__PROVIDER__' : v;
  if (Array.isArray(v)) return v.map(placeholder);
  if (v && typeof v === 'object') return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, (k === 'provider' || k === 'providerID') && x === P ? '__PROVIDER__' : placeholder(x)]));
  return v;
}
function entry(t, withBody) {
  const routines = t.routines.map((r) => {
    const cron = 'cron' in r.schedule ? r.schedule.cron : null;
    const rule = cron ? m.ruleFromCron(cron) : null;
    return { id: r.id, label: r.label, title: m.routineTitle(r.label), cron, event: 'event' in r.schedule ? r.schedule.event : null, prompt: r.prompt, scheduleLabel: rule ? m.ruleLabel(rule) : null };
  });
  const out = { id: t.id, version: t.version, name: t.name, tagline: t.tagline, category: t.category, color: t.color, look: t.look ?? null,
    welcome: t.welcome ?? null, firstTask: t.firstTask, starterPrompts: t.starterPrompts ?? [], handoffs: (t.handoffs ?? []).map((h) => h.to), routines };
  if (withBody) {
    const avatar = m.castAvatarConfig(t.look, t.color) ?? m.buildAvatarConfigFromEditor(m.defaultAvatarEditorValues(t.color), t.color);
    const payload = m.buildCreateBotPayload({ formData: m.defaultBotFormData(m.botTemplateToFormData(t)), avatar });
    out.createBody = placeholder(wire(payload));
  }
  return out;
}
// The primary bot: applyBotTemplate(PRIMARY_BOT) without a payload → createBotFromTemplate default avatar.
function primary() {
  const t = m.PRIMARY_BOT;
  const payload = m.buildCreateBotPayload({ formData: m.defaultBotFormData(m.botTemplateToFormData(t)), avatar: m.buildAvatarConfigFromEditor(m.defaultAvatarEditorValues(t.color), t.color) });
  return { ...entry(t, false), createBody: placeholder(wire(payload)) };
}
const looks = m.LOOKS.filter((l) => l.kind === 'cast').map((l) => ({ id: l.id, label: l.label, field: l.field, avatar: m.lookAvatarConfig(l.id) }));
const data = { source: 'allternit-ai src/lib/bots/templates (BUILTIN_BOT_TEMPLATES, PRIMARY_BOT)', primary: primary(), templates: m.BUILTIN_BOT_TEMPLATES.map((t) => entry(t, true)), looks };
const json = JSON.stringify(data);
if (json.includes('"""#') || json.includes('\\#')) throw new Error('raw string delimiter clash');
const fs = await import('node:fs');
fs.writeFileSync('catalog.json', JSON.stringify(data, null, 1));
console.error('templates', data.templates.length, 'looks', looks.map(l=>l.id).join(','), 'bytes', json.length);
const swift = `import Foundation

// GENERATED from allternit-ai \`src/lib/bots/templates\` (BUILTIN_BOT_TEMPLATES,
// PRIMARY_BOT, TEMPLATE_LOOKS, the cast looks) and the web's own create path
// (\`botTemplateToFormData\` → \`defaultBotFormData\` → \`buildCreateBotPayload\`
// → agent.service \`createAgent\` wire body). The server's
// \`/api/v1/agent-templates\` catalog is the pattern-crew catalog (no routines,
// none of these ids), so the iPhone ships the web's data as-is.
// \`__MODEL__\` / \`__PROVIDER__\` are filled at create time with the person's
// model, like the web's \`templateModel()\`. Regenerate; don't hand-edit.
enum MeetBotCatalogData {
    static let json = #"""
${json}
"""#
}
`;
fs.writeFileSync('MeetBotCatalogData.swift', swift);
