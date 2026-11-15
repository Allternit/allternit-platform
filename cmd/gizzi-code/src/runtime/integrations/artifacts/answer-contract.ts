/** Versioned, data-only answers. Layout and executable expressions belong to the app. */
import { z } from 'zod/v4';
import { CatalogAnswerContract, CATALOG_BY_ID, catalogManual } from './answer-catalog';

const text = z.string().max(16000);
const label = z.string().min(1).max(240);
const strings = z.array(text).max(100);
const scalar = z.union([text, z.number(), z.boolean(), z.null()]);
const source = z.string().min(1).max(1000);
const id = z.string().regex(/^[a-z][a-z0-9_]{0,39}$/);
const value = z.union([z.number(), z.strictObject({ input: id })]);
// A bounded expression, not JavaScript or a model-written OpenUI program.
type Calculation = number | { input: string } | { op: 'add' | 'subtract' | 'multiply' | 'divide'; left: Calculation; right: Calculation };
const expression: z.ZodType<Calculation> = z.lazy(() => z.union([value, z.strictObject({ op: z.enum(['add', 'subtract', 'multiply', 'divide']), left: expression, right: expression })]));
const format = z.enum(['number', 'currency', 'percent', 'compact']);
const field = z.discriminatedUnion('type', [
  z.strictObject({ id, type: z.literal('number'), label, value: z.number(), min: z.number().optional(), max: z.number().optional(), step: z.number().positive().optional() }),
  z.strictObject({ id, type: z.literal('text'), label, value: text }),
  z.strictObject({ id, type: z.literal('date'), label, value: z.iso.date() }),
  z.strictObject({ id, type: z.literal('select'), label, value: text, options: z.array(label).min(1).max(50) }),
  z.strictObject({ id, type: z.literal('toggle'), label, value: z.boolean() }),
]);
const query = z.strictObject({ tool: z.string().regex(/^[a-zA-Z0-9_.-]+__[a-zA-Z0-9_.-]+$/), args: z.record(z.string(), z.json()), rowsPath: z.string().regex(/^[a-zA-Z0-9_]+(?:\.[a-zA-Z0-9_]+)*$/).optional() });
const base = { version: z.literal(1), title: label, summary: text.optional() };
const variant = <T extends string, S extends z.ZodType>(template: T, data: S) => z.strictObject({ ...base, template: z.literal(template), data });

/** No HTML, component names, CSS, actions or expression source is accepted. */
export const BaseAnswerContract = z.discriminatedUnion('template', [
  variant('quick-answer', z.strictObject({ text })),
  variant('comparison', z.strictObject({ columns: z.array(z.strictObject({ title: label, points: strings })).max(8), recommendation: text.optional() })),
  variant('decision', z.strictObject({ question: label, options: z.array(label).min(2).max(9), recommendation: text.optional() })),
  variant('plan', z.strictObject({ items: strings, checked: strings.optional() })),
  variant('how-to', z.strictObject({ steps: strings, verification: text })),
  variant('troubleshooting', z.strictObject({ cause: text, fix: strings, verify: text })),
  variant('status-report', z.strictObject({ done: strings, inProgress: strings, blocked: strings, next: strings })),
  variant('research-brief', z.strictObject({ findings: z.array(z.strictObject({ text, url: z.url().regex(/^https:\/\//i) })).max(50), openQuestions: strings })),
  variant('table', z.strictObject({ headers: z.array(label).min(1).max(30), rows: z.array(z.array(scalar).max(30)).max(200), query: query.optional(), source })),
  variant('chart', z.strictObject({ kind: z.enum(['bar', 'line', 'area', 'pie']), labels: z.array(label).max(100), values: z.array(z.number()).max(100), source })),
  variant('form', z.strictObject({ fields: z.array(field).min(1).max(20), submitLabel: label })),
  variant('calculator', z.strictObject({ inputs: z.array(z.strictObject({ id, label, value: z.number(), min: z.number(), max: z.number(), step: z.number().positive().optional() })).min(1).max(12), results: z.array(z.strictObject({ label, value: expression, format: format.optional(), unit: label.optional(), note: source })).min(1).max(12) })),
]);

export const AnswerContract = z.union([BaseAnswerContract, CatalogAnswerContract]);

export type AnswerEnvelope = z.infer<typeof AnswerContract>;
export type AnswerTemplateId = AnswerEnvelope['template'];
export const ANSWER_BODY_FORMAT = 'application/vnd.allternit.answer+json';
export const MAX_ANSWER_BYTES = 65536;

/** Shared by browser and server; the semantic rules below are checked on both. */
export const answerContractJsonSchema = () => z.toJSONSchema(AnswerContract);

export function parseAnswerEnvelope(code: string): { answer?: AnswerEnvelope; problems: string[] } {
  if (new TextEncoder().encode(code).length > MAX_ANSWER_BYTES) return { problems: ['Answer exceeds 64 KiB.'] };
  let raw: unknown;
  try { raw = JSON.parse(code); } catch { return { problems: ['Expected a versioned answer template with JSON data.'] }; }
  // Bound recursive query arguments before Zod walks them. Never accept
  // prototype keys in data that will be passed through another evaluator.
  const pending: Array<[unknown, number]> = [[raw, 0]];
  let nodes = 0;
  while (pending.length) {
    const [node, depth] = pending.pop()!;
    if (++nodes > 10000 || depth > 32) return { problems: ['Answer data is too deeply nested or too complex.'] };
    if (node && typeof node === 'object') for (const [key, child] of Object.entries(node)) {
      if (['__proto__', 'prototype', 'constructor'].includes(key)) return { problems: ['Reserved data key.'] };
      pending.push([child, depth + 1]);
    }
  }
  const parsed = AnswerContract.safeParse(raw);
  if (!parsed.success) return { problems: parsed.error.issues.slice(0, 6).map((i) => `${i.path.join('.') || 'answer'}: ${i.message}`) };
  const answer = parsed.data;
  const problems: string[] = [];
  if (answer.template === 'chart' && answer.data.labels.length !== answer.data.values.length) problems.push('Chart labels and values must have the same length.');
  if (answer.template === 'chart' && answer.data.kind === 'pie' && answer.data.values.some((v) => v < 0)) problems.push('Pie values must not be negative.');
  if (answer.template === 'table' && answer.data.rows.some((r) => r.length !== answer.data.headers.length)) problems.push('Every table row must match its headers.');
  if (answer.template === 'plan' && answer.data.checked?.some((s) => !answer.data.items.includes(s))) problems.push('Checked items must belong to the plan.');
  if (answer.template === 'form' || answer.template === 'calculator') {
    const fields = answer.template === 'form' ? answer.data.fields : answer.data.inputs;
    if (new Set(fields.map((f) => f.id)).size !== fields.length) problems.push('Input IDs must be unique.');
    for (const f of fields) {
      if ('min' in f && 'max' in f && f.min !== undefined && f.max !== undefined && f.min > f.max) problems.push(`${f.id}: minimum exceeds maximum.`);
      if (typeof f.value === 'number' && (('min' in f && f.min !== undefined && f.value < f.min) || ('max' in f && f.max !== undefined && f.value > f.max))) problems.push(`${f.id}: value is outside its bounds.`);
      if ('type' in f && f.type === 'select' && !f.options.includes(f.value)) problems.push(`${f.id}: selected value is not an option.`);
    }
    if (answer.template === 'calculator') {
      const ids = new Set(fields.map((f) => f.id));
      for (const result of answer.data.results) {
        const values = [result.value];
        while (values.length) {
          const v = values.pop()!;
          if (typeof v === 'object') {
            if ('op' in v) values.push(v.left, v.right);
            else if (!ids.has(v.input)) problems.push('Calculation refers to an unknown input.');
          }
        }
      }
    }
  }
  return problems.length ? { problems } : { answer, problems: [] };
}

/** Only a checked envelope can reach the compiler. All strings are JSON-escaped. */
export function compileAnswerTemplate(answer: AnswerEnvelope): string {
  const q = JSON.stringify;
  const catalog = CATALOG_BY_ID.get(answer.template);
  if (catalog) {
    const parts = catalog.compile(answer.data);
    if (answer.summary) parts.unshift(`Text(${q(answer.summary)})`);
    if (!parts.length) parts.push('Text("No data yet.")');
    return `root = Answer(${q(answer.title)}, [${parts.join(', ')}])`;
  }
  const lines: string[] = [];
  const children: string[] = [];
  const add = (s: string) => {
    const name = `part${children.length}`;
    lines.push(`${name} = ${s}`);
    children.push(name);
  };
  const section = (title: string, items: string[]) => `Section(${q(title)}, [List(${q(items)})])`;
  if (answer.summary) add(`Text(${q(answer.summary)})`);
  switch (answer.template) {
    case 'quick-answer': add(`Text(${q(answer.data.text)})`); break;
    case 'comparison':
      add(answer.data.columns.length ? `Compare(${q(answer.data.columns)})` : 'Text("No options to compare.")');
      if (answer.data.recommendation) add(`Callout(${q(answer.data.recommendation)})`);
      break;
    case 'decision':
      if (answer.data.recommendation) add(`Text(${q(answer.data.recommendation)})`);
      add(`Choice(${q(answer.data.question)}, ${q(answer.data.options)})`); break;
    case 'plan': lines.push(`$checked = ${q(answer.data.checked ?? [])}`); add(answer.data.items.length ? `Checklist("Plan", ${q(answer.data.items)}, $checked)` : 'Text("No plan items yet.")'); break;
    case 'how-to': add(answer.data.steps.length ? `Steps(${q(answer.data.steps)})` : 'Text("No steps yet.")'); add(`Callout(${q(answer.data.verification)})`); break;
    case 'troubleshooting': add(`Section("Cause", [Text(${q(answer.data.cause)})])`); add(`Steps(${q(answer.data.fix)})`); add(`Section("Verify", [Text(${q(answer.data.verify)})])`); break;
    case 'status-report': for (const [key, title] of [['done', 'Done'], ['inProgress', 'In progress'], ['blocked', 'Blocked'], ['next', 'Next']] as const) add(section(title, answer.data[key].length ? answer.data[key] : ['None reported.'])); break;
    case 'research-brief':
      if (!answer.data.findings.length) add('Text("No findings yet.")');
      for (const finding of answer.data.findings) { add(`Text(${q(finding.text)})`); add(`Link("Source", ${q(finding.url)})`); }
      add(section('Open questions', answer.data.openQuestions.length ? answer.data.openQuestions : ['None reported.'])); break;
    case 'table': {
      let rows = q(answer.data.rows);
      if (answer.data.query) {
        const query = answer.data.query;
        lines.push(`records = Query(${q(query.tool)}, ${q(query.args)}, {"rows":${rows}})`);
        rows = `records.${query.rowsPath ?? 'rows'}`;
      }
      add(`Table(${q(answer.data.headers)}, ${rows})`); add(`Text(${q(answer.data.source)}, "muted")`); break;
    }
    case 'chart': {
      const component = { bar: 'BarChart', line: 'LineChart', area: 'AreaChart', pie: 'PieChart' }[answer.data.kind];
      add(answer.data.values.length ? `${component}(${q(answer.data.labels)}, ${q(answer.data.values)})` : 'Text("No chart data.")');
      add(`Text(${q(answer.data.source)}, "muted")`); break;
    }
    case 'form':
      for (const f of answer.data.fields) {
        lines.push(`$${f.id} = ${q(f.value)}`);
        const value = `$${f.id}`;
        const field = f.type === 'number' ? `NumberField(${q(f.label)}, ${value}, ${f.step ?? 1}, null, null, ${f.min ?? 'null'}, ${f.max ?? 'null'})`
          : f.type === 'select' ? `Select(${q(f.label)}, ${value}, ${q(f.options)})`
          : `${{ text: 'TextField', date: 'DateField', toggle: 'Toggle' }[f.type]}(${q(f.label)}, ${value})`;
        add(field);
      }
      // A form submits the current values, never a model-written action.
      add(`Button(${q(answer.data.submitLabel)}, Action([@ToAssistant(${q(answer.data.submitLabel + ': ')} + ${answer.data.fields.map((f) => `${q(f.label + '=')} + $${f.id}`).join(' + ", " + ' )})]))`);
      break;
    case 'calculator': {
      for (const f of answer.data.inputs) { lines.push(`$${f.id} = ${q(f.value)}`); add(`NumberField(${q(f.label)}, $${f.id}, ${f.step ?? 1}, null, null, ${f.min}, ${f.max})`); add(`Slider(${q("Adjust " + f.label)}, $${f.id}, ${f.min}, ${f.max}, ${f.step ?? 1})`); }
      let calculation = 0;
      const val = (e: Calculation): string => {
        if (typeof e === 'number') return String(e);
        if ('input' in e) return `$${e.input}`;
        const left = val(e.left), right = val(e.right);
        const name = `calculation${calculation++}`;
        const operation = e.op === 'divide' ? `(${right} == 0 ? "Cannot divide by zero" : ${left} / ${right})` : `(${left} ${{ add: '+', subtract: '-', multiply: '*' }[e.op]} ${right})`;
        lines.push(`${name} = (${left} == "Cannot divide by zero" ? "Cannot divide by zero" : (${right} == "Cannot divide by zero" ? "Cannot divide by zero" : ${operation}))`);
        return name;
      };
      for (const r of answer.data.results) {
        add(`Metric(${q(r.label)}, ${val(r.value)}, ${q(r.format ?? 'number')}, ${q(r.unit ?? '')}, ${q(r.note)})`);
      }
      break;
    }
  }
  return [...lines, `root = Answer(${q(answer.title)}, [${children.join(', ')}])`].join('\n');
}

export function canonicalAnswer(code: string): string | null {
  const parsed = parseAnswerEnvelope(code);
  return parsed.answer ? JSON.stringify(parsed.answer) : null;
}

/** Copy only valid user input into a new version; state cannot change the template. */
export function answerWithState(code: string, state: Record<string, unknown>): string | null {
  const answer = parseAnswerEnvelope(code).answer;
  if (!answer) return null;
  if (answer.template === 'plan' && Array.isArray(state.$checked)) {
    answer.data.checked = state.$checked.filter((v): v is string => typeof v === 'string' && answer.data.items.includes(v));
  } else if (answer.template === 'form' || answer.template === 'calculator') {
    const fields = answer.template === 'form' ? answer.data.fields : answer.data.inputs;
    for (const f of fields) {
      const v = state[`$${f.id}`];
      if (v === undefined || typeof v !== typeof f.value) continue;
      if (typeof v === 'number' && !Number.isFinite(v)) continue;
      if (typeof v === 'number' && ('min' in f || 'max' in f)) {
        f.value = Math.min(('max' in f ? f.max : undefined) ?? Infinity, Math.max(('min' in f ? f.min : undefined) ?? -Infinity, v));
      } else if ('type' in f && f.type === 'select') {
        if (typeof v === 'string' && f.options.includes(v)) f.value = v;
      } else if (typeof v === 'string' || typeof v === 'boolean') {
        (f as { value: string | number | boolean }).value = v;
      }
    }
  }
  return canonicalAnswer(JSON.stringify(answer));
}

export const ANSWER_TEMPLATE_MANUAL = `## Enforced answer templates
Interactive answers use a data-only JSON object inside a fenced code block tagged \`openui\`. Raw OpenUI programs are not accepted for new cards.
Use exactly {"version":1,"template":"…","title":"…","data":{…}} with optional summary. No components, CSS, HTML or executable code.
Templates and required data:
quick-answer: {text}; comparison: {columns:[{title,points:[string]}],recommendation?}; decision: {question,options:[string],recommendation?}; plan: {items:[string],checked?:[string]}; how-to: {steps:[string],verification}; troubleshooting: {cause,fix:[string],verify}; status-report: {done:[string],inProgress:[string],blocked:[string],next:[string]}; research-brief: {findings:[{text,url:https}],openQuestions:[string]}; table: {headers:[string],rows:[[scalar]],source,query?:{tool:connector__tool,args,rowsPath?}}; chart: {kind:bar|line|area|pie,labels:[string],values:[number],source}; form: {fields:[{id,type:text|date|number|select|toggle,label,value,options?:[string],min?,max?,step?}],submitLabel}; calculator: {inputs:[{id,label,value:number,min,max,step?}],results:[{label,value:number|{input:id}|{op:add|subtract|multiply|divide,left:number|{input:id},right:number|{input:id}},format?:number|currency|percent|compact,unit?,note}]}.
Calculation operands may themselves be expression objects; the envelope depth limit still applies.
Use only real data with its source. Empty arrays are allowed for empty data. Chart arrays must match; table rows must match headers. Input IDs are unique lowercase identifiers. Do not add keys. A form asks the next assistant turn; it never performs a connector write. Table queries use read-only tools only. Other connector actions belong in the existing approval flow.
Example: {"version":1,"template":"decision","title":"Choose a format","data":{"question":"Which format?","options":["PDF","DOCX"]}}
If a card is not needed, write the answer in Markdown. The app renders prose through its fixed text template. Invalid cards fall back to readable text; they never execute model-written layouts.

Additional catalog templates (same envelope and validation rules; examples in UI Forge are synthetic):
${catalogManual()}`;
