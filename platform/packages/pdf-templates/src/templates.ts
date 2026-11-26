import { resolveTheme } from './themes';
import {
  PDF_TEMPLATES,
  type PdfBlock,
  type PdfDocModel,
  type PdfPageSize,
  type PdfRenderRequest,
  type PdfSignatureSlot,
  type PdfTemplateId,
  type PdfTemplateInfo,
  type PdfVariant,
} from './types';

type Rec = Record<string, unknown>;

const str = (v: unknown, d = ''): string => (typeof v === 'string' ? v : typeof v === 'number' ? String(v) : d);
const num = (v: unknown, d = 0): number => {
  const n = typeof v === 'number' ? v : typeof v === 'string' ? Number(v.replace(/[^0-9.\-]/g, '')) : NaN;
  return Number.isFinite(n) ? n : d;
};
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const rec = (v: unknown): Rec => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Rec) : {});
const strs = (v: unknown): string[] => arr(v).map((x) => str(x)).filter(Boolean);
const lines = (v: unknown): string[] =>
  typeof v === 'string' ? v.split('\n').map((s) => s.trim()).filter(Boolean) : strs(v);

export function money(n: number, currency = 'USD'): string {
  try {
    return new Intl.NumberFormat('en-US', { style: 'currency', currency }).format(n);
  } catch {
    return `${currency} ${n.toFixed(2)}`;
  }
}

const cols = (items: { label: string; lines: string[] }[]): PdfBlock => ({
  t: 'cols',
  cols: items.filter((c) => c.lines.length > 0),
});
const meta = (pairs: [string, string][]): [string, string][] => pairs.filter(([, v]) => v);

function party(v: unknown): string[] {
  const r = rec(v);
  return [str(r.name), ...lines(r.lines ?? r.address), str(r.email), str(r.phone)].filter(Boolean);
}

function pickVariant(v: PdfVariant | undefined, allowed: PdfVariant[]): PdfVariant {
  return v && allowed.includes(v) ? v : allowed[0];
}

// ---- signatures ------------------------------------------------------------

/**
 * A `signatures` block from template data, or null when the data asks for none.
 * Accepts `true` (use the template's defaults), an array of slots, or
 * `{ note?, slots? }`. Slot ids are kept unique; a missing id becomes `slot-N`.
 */
function signatures(v: unknown, fallback: { note?: string; slots: PdfSignatureSlot[] }): PdfBlock | null {
  if (v == null || v === false) return null;
  const r = Array.isArray(v) ? { slots: v } : rec(v);
  const given = arr(r.slots).map(rec).map((s, i): PdfSignatureSlot => ({
    id: str(s.id) || `slot-${i + 1}`,
    label: str(s.label) || `Signature ${i + 1}`,
    name: str(s.name) || undefined,
    signed_on: str(s.signed_on ?? s.signedOn) || undefined,
  }));
  const slots = given.length ? given : fallback.slots;
  if (!slots.length) return null;
  const seen = new Set<string>();
  const unique = slots.map((s) => {
    let id = s.id;
    for (let n = 2; seen.has(id); n++) id = `${s.id}-${n}`;
    seen.add(id);
    return id === s.id ? s : { ...s, id };
  });
  const note = 'note' in r ? str(r.note) : fallback.note;
  return { t: 'signatures', note: note || undefined, slots: unique };
}

// ---- line items (invoice, quote) -------------------------------------------

function lineItems(d: Rec, totalLabel: string): { table: PdfBlock; totals: PdfBlock } {
  const cur = str(d.currency, 'USD');
  const items = arr(d.items).map(rec);
  const rows = items.map((it) => {
    const qty = num(it.qty ?? it.quantity, 1);
    const unit = num(it.unitPrice ?? it.unit ?? it.price);
    const amount = it.amount != null ? num(it.amount) : qty * unit;
    return { desc: str(it.description ?? it.name), detail: str(it.detail), qty, unit, amount };
  });
  const subtotal = rows.reduce((s, r) => s + r.amount, 0);
  const discount = num(d.discount);
  const taxRate = num(d.taxRate);
  const tax = Math.round((subtotal - discount) * taxRate) / 100;
  const total = subtotal - discount + tax;
  const totals: { label: string; value: string; strong?: boolean }[] = [{ label: 'Subtotal', value: money(subtotal, cur) }];
  if (discount) totals.push({ label: 'Discount', value: `-${money(discount, cur)}` });
  if (taxRate) totals.push({ label: `Tax (${taxRate}%)`, value: money(tax, cur) });
  totals.push({ label: totalLabel, value: money(total, cur), strong: true });
  return {
    table: {
      t: 'table', head: ['Description', 'Qty', 'Unit', 'Amount'], align: ['l', 'r', 'r', 'r'], widths: [0.52, 0.1, 0.18, 0.2],
      rows: rows.map((r) => [r.detail ? `${r.desc}\n${r.detail}` : r.desc, String(r.qty), money(r.unit, cur), money(r.amount, cur)]),
    },
    totals: { t: 'totals', rows: totals },
  };
}

// ---- invoice ---------------------------------------------------------------

function invoice(d: Rec, variant: PdfVariant): PdfBlock[] {
  const { table, totals } = lineItems(d, d.paid ? 'Total paid' : 'Amount due');
  const client = d.to ?? d.billTo ?? d.client;
  const blocks: PdfBlock[] = [
    {
      t: 'masthead', title: 'Invoice', brand: str(rec(d.from).name), style: variant,
      meta: meta([['Number', str(d.number)], ['Issued', str(d.issued)], ['Due', str(d.due)]]),
    },
    cols([
      { label: 'From', lines: party(d.from) },
      { label: 'Bill to', lines: party(client) },
    ]),
    table,
    totals,
  ];
  if (str(d.paymentInstructions)) blocks.push({ t: 'heading', text: 'Payment' }, { t: 'text', text: str(d.paymentInstructions) });
  if (str(d.notes)) blocks.push({ t: 'heading', text: 'Notes' }, { t: 'text', text: str(d.notes) });
  const sig = signatures(d.signatures, {
    note: 'By signing, both parties confirm the services listed above are complete.',
    slots: [
      { id: 'provider', label: 'Service provider', name: str(rec(d.from).name) || undefined },
      { id: 'client', label: 'Client', name: str(rec(client).name) || undefined },
    ],
  });
  if (sig) blocks.push(sig);
  return blocks;
}

// ---- quote -----------------------------------------------------------------

function quote(d: Rec, variant: PdfVariant): PdfBlock[] {
  const { table, totals } = lineItems(d, 'Quote total');
  const client = d.to ?? d.client ?? d.preparedFor;
  const validUntil = str(d.validUntil ?? d.valid_until);
  const blocks: PdfBlock[] = [
    {
      t: 'masthead', title: 'Quote', brand: str(rec(d.from).name), style: variant,
      meta: meta([['Number', str(d.number)], ['Issued', str(d.issued ?? d.date)], ['Valid until', validUntil]]),
    },
    cols([
      { label: 'From', lines: party(d.from) },
      { label: 'Prepared for', lines: party(client) },
    ]),
    table,
    totals,
  ];
  if (str(d.scope)) blocks.push({ t: 'heading', text: 'Scope' }, { t: 'text', text: str(d.scope) });
  if (str(d.terms)) blocks.push({ t: 'heading', text: 'Terms' }, { t: 'text', text: str(d.terms) });
  if (str(d.notes)) blocks.push({ t: 'heading', text: 'Notes' }, { t: 'text', text: str(d.notes) });
  const sig = signatures(d.signatures ?? true, {
    note: validUntil
      ? `By signing, the client accepts this quote and its terms. Prices hold until ${validUntil}.`
      : 'By signing, the client accepts this quote and its terms.',
    slots: [
      { id: 'provider', label: 'Service provider', name: str(rec(d.from).name) || undefined },
      { id: 'client', label: 'Client', name: str(rec(client).name) || undefined },
    ],
  });
  if (sig) blocks.push(sig);
  return blocks;
}

// ---- contract --------------------------------------------------------------

function contract(d: Rec, variant: PdfVariant): PdfBlock[] {
  const parties = arr(d.parties).map(rec);
  const blocks: PdfBlock[] = [
    {
      t: 'masthead', title: str(d.title, 'Agreement'), subtitle: str(d.subtitle), brand: str(parties[0]?.name), style: variant,
      meta: meta([['Number', str(d.number)], ['Effective', str(d.effective ?? d.date)], ['Term', str(d.term)]]),
    },
    cols(parties.map((p, i) => ({ label: str(p.role) || `Party ${i + 1}`, lines: party(p) }))),
  ];
  if (str(d.intro)) blocks.push({ t: 'text', text: str(d.intro) });
  arr(d.clauses).forEach((raw, i) => {
    const cl = typeof raw === 'string' ? { body: raw } : rec(raw);
    const heading = str(cl.heading ?? cl.title);
    blocks.push({ t: 'heading', text: heading ? `${i + 1}. ${heading}` : `${i + 1}.` });
    if (str(cl.body)) blocks.push({ t: 'text', text: str(cl.body) });
    if (arr(cl.items).length) blocks.push({ t: 'list', items: strs(cl.items) });
  });
  const sig = signatures(d.signatures ?? true, {
    note: 'By signing, each party agrees to the terms above.',
    slots: parties.map((p, i) => ({
      id: str(p.id) || `party-${i + 1}`,
      label: str(p.role) || `Party ${i + 1}`,
      name: str(p.name) || undefined,
    })),
  });
  if (sig) blocks.push(sig);
  return blocks;
}

// ---- report ----------------------------------------------------------------

function report(d: Rec, variant: PdfVariant): PdfBlock[] {
  const blocks: PdfBlock[] = [
    {
      t: 'masthead', title: str(d.title, 'Report'), subtitle: str(d.subtitle), brand: str(d.org), style: variant,
      meta: meta([['Period', str(d.period)], ['Prepared', str(d.date)], ['By', str(d.author)]]),
    },
  ];
  const stats = arr(d.metrics).map(rec).map((m) => ({ label: str(m.label), value: str(m.value), note: str(m.note) || undefined }));
  if (stats.length) blocks.push({ t: 'stats', items: stats });
  if (str(d.summary)) blocks.push({ t: 'heading', text: 'Summary' }, { t: 'text', text: str(d.summary) });
  for (const s of arr(d.sections).map(rec)) {
    if (str(s.heading)) blocks.push({ t: 'heading', text: str(s.heading) });
    if (str(s.body)) blocks.push({ t: 'text', text: str(s.body) });
    if (arr(s.bullets).length) blocks.push({ t: 'list', items: strs(s.bullets) });
    const tbl = rec(s.table);
    const head = strs(tbl.head);
    if (head.length) {
      const body = arr(tbl.rows).map((r) => arr(r).map((c) => str(c)));
      blocks.push({
        t: 'table', head, rows: body,
        align: head.map((_, i) => (i === 0 ? 'l' : 'r')),
        widths: head.map((_, i) => (i === 0 ? (head.length === 1 ? 1 : 0.4) : 0.6 / Math.max(1, head.length - 1))),
      });
    }
  }
  return blocks;
}

// ---- ticket ----------------------------------------------------------------

function ticket(d: Rec, variant: PdfVariant): PdfBlock[] {
  const pairs: [string, string][] = [
    ['Date', str(d.date)], ['Time', str(d.time)], ['Venue', str(d.venue)], ['Address', str(d.address)],
    ['Section', str(d.section)], ['Seat', str(d.seat)], ['Admission', str(d.admission)], ['Price', str(d.price)],
  ];
  const blocks: PdfBlock[] = [
    { t: 'masthead', title: 'Admission ticket', brand: str(d.organizer), style: variant, meta: meta([['Order', str(d.order)]]) },
    {
      t: 'ticket', event: str(d.event, 'Event'), holder: str(d.holder) || undefined,
      lines: pairs.filter(([, v]) => v).map(([label, value]) => ({ label, value })),
      code: str(d.code),
    },
  ];
  if (str(d.terms)) blocks.push({ t: 'note', text: str(d.terms) });
  return blocks;
}

// ---- meeting minutes -------------------------------------------------------

function minutes(d: Rec, variant: PdfVariant): PdfBlock[] {
  const blocks: PdfBlock[] = [
    {
      t: 'masthead', title: str(d.title, 'Meeting minutes'), brand: str(d.org), style: variant,
      meta: meta([['Date', str(d.date)], ['Time', str(d.time)], ['Location', str(d.location)]]),
    },
    cols([
      { label: 'Attendees', lines: strs(d.attendees) },
      { label: 'Absent', lines: strs(d.absent) },
    ]),
  ];
  if (arr(d.agenda).length) blocks.push({ t: 'heading', text: 'Agenda' }, { t: 'list', items: strs(d.agenda), numbered: true });
  for (const s of arr(d.discussion).map(rec)) {
    blocks.push({ t: 'heading', text: str(s.topic) || 'Discussion' }, { t: 'text', text: str(s.notes) });
    if (arr(s.decisions).length) blocks.push({ t: 'list', items: strs(s.decisions).map((x) => `Decision: ${x}`) });
  }
  const actions = arr(d.actions).map(rec);
  if (actions.length) {
    blocks.push({
      t: 'heading', text: 'Action items',
    }, {
      t: 'table', head: ['Action', 'Owner', 'Due'], align: ['l', 'l', 'l'], widths: [0.6, 0.22, 0.18],
      rows: actions.map((a) => [str(a.task), str(a.owner), str(a.due)]),
    });
  }
  if (str(d.nextMeeting)) blocks.push({ t: 'note', text: `Next meeting: ${str(d.nextMeeting)}` });
  return blocks;
}

// ---- receipt ---------------------------------------------------------------

function receipt(d: Rec, variant: PdfVariant): PdfBlock[] {
  const cur = str(d.currency, 'USD');
  const items = arr(d.items).map(rec).map((it) => {
    const qty = num(it.qty ?? it.quantity, 1);
    const amount = it.amount != null ? num(it.amount) : qty * num(it.unitPrice ?? it.price);
    return { name: str(it.name ?? it.description), qty, amount };
  });
  const subtotal = items.reduce((s, r) => s + r.amount, 0);
  const tax = num(d.tax);
  const tip = num(d.tip);
  const total = d.total != null ? num(d.total) : subtotal + tax + tip;
  const totals: { label: string; value: string; strong?: boolean }[] = [{ label: 'Subtotal', value: money(subtotal, cur) }];
  if (tax) totals.push({ label: 'Tax', value: money(tax, cur) });
  if (tip) totals.push({ label: 'Tip', value: money(tip, cur) });
  totals.push({ label: 'Total', value: money(total, cur), strong: true });
  const blocks: PdfBlock[] = [
    {
      t: 'masthead', title: 'Receipt', brand: str(d.merchant), style: variant,
      meta: meta([['Receipt', str(d.number)], ['Date', str(d.date)], ['Paid with', str(d.method)]]),
    },
    cols([{ label: 'Merchant', lines: [str(d.merchant), ...lines(d.address)].filter(Boolean) }, { label: 'Customer', lines: lines(d.customer) }]),
    {
      t: 'table', head: ['Item', 'Qty', 'Amount'], align: ['l', 'r', 'r'], widths: [0.62, 0.12, 0.26],
      rows: items.map((r) => [r.name, String(r.qty), money(r.amount, cur)]),
    },
    { t: 'totals', rows: totals },
  ];
  if (str(d.footer)) blocks.push({ t: 'note', text: str(d.footer) });
  return blocks;
}

// ---- registry ---------------------------------------------------------------

const BUILDERS: Record<PdfTemplateId, (d: Rec, v: PdfVariant) => PdfBlock[]> = { invoice, quote, contract, report, ticket, minutes, receipt };

export const PDF_TEMPLATE_INFO: Record<PdfTemplateId, PdfTemplateInfo> = {
  invoice: {
    id: 'invoice', label: 'Invoice', description: 'Line items, tax and totals. Classic, minimal or modern masthead.',
    variants: ['classic', 'minimal', 'modern'],
    sample: {
      number: 'INV-1042', issued: 'Nov 9, 2026', due: 'Dec 9, 2026', currency: 'USD', taxRate: 8.25,
      from: { name: 'Northwind Studio', lines: ['410 Market Street', 'Austin, TX 78701'], email: 'billing@northwind.example' },
      to: { name: 'Harbor Coffee Co.', lines: ['22 Pier Road', 'Galveston, TX 77550'] },
      items: [
        { description: 'Brand audit', detail: 'Two-week review', qty: 1, unitPrice: 1800 },
        { description: 'Menu design', qty: 3, unitPrice: 240 },
      ],
      paymentInstructions: 'Pay by ACH or card using the link in the email. Net 30.',
      notes: 'Thank you for your business.',
    },
  },
  quote: {
    id: 'quote', label: 'Quote', description: 'Line items and totals with a valid-until date, and slots for both sides to accept.',
    variants: ['classic', 'minimal', 'modern'],
    sample: {
      number: 'Q-2051', issued: 'Nov 26, 2026', validUntil: 'Dec 26, 2026', currency: 'USD', taxRate: 8.25,
      from: { name: 'Northwind Studio', lines: ['410 Market Street', 'Austin, TX 78701'], email: 'hello@northwind.example' },
      to: { name: 'Harbor Coffee Co.', lines: ['22 Pier Road', 'Galveston, TX 77550'] },
      items: [
        { description: 'Website design', detail: 'Five pages, two rounds of changes', qty: 15, unitPrice: 100 },
        { description: 'Website development', qty: 10, unitPrice: 100 },
        { description: 'Copywriting', qty: 5, unitPrice: 50 },
      ],
      scope: 'Design and build of the new Harbor Coffee website, from wireframes to launch.',
      terms: '50% due on acceptance, 50% on launch. Work starts within five business days of acceptance.',
    },
  },
  contract: {
    id: 'contract', label: 'Contract', description: 'Parties, numbered clauses, and a signature slot for each party.',
    variants: ['classic', 'minimal'],
    sample: {
      title: 'Services agreement', number: 'SA-118', effective: 'Dec 1, 2026', term: '6 months',
      parties: [
        { id: 'provider', role: 'Service provider', name: 'Northwind Studio', lines: ['410 Market Street', 'Austin, TX 78701'] },
        { id: 'client', role: 'Client', name: 'Harbor Coffee Co.', lines: ['22 Pier Road', 'Galveston, TX 77550'] },
      ],
      intro: 'This agreement sets out how Northwind Studio will provide design services to Harbor Coffee Co.',
      clauses: [
        { heading: 'Services', body: 'Northwind Studio will design and build the website described in quote Q-2051.' },
        { heading: 'Fees and payment', body: 'The client pays the fees in quote Q-2051. Invoices are due within 30 days.' },
        { heading: 'Ownership', body: 'The client owns the finished work once it is paid for in full.' },
        { heading: 'Ending the agreement', body: 'Either party may end this agreement with 14 days written notice. Work done until then is paid for.' },
      ],
    },
  },
  report: {
    id: 'report', label: 'Report', description: 'Headline numbers, then sections with text, bullets and tables.',
    variants: ['classic', 'minimal', 'modern'],
    sample: {
      title: 'Quarterly operations report', subtitle: 'Q3 2026', org: 'Northwind Studio', period: 'Jul to Sep 2026', date: 'Oct 2, 2026', author: 'Operations',
      metrics: [{ label: 'Revenue', value: '$184,200', note: '+12% vs Q2' }, { label: 'Active clients', value: '27' }, { label: 'On-time delivery', value: '94%' }],
      summary: 'Revenue grew for the third quarter in a row. Delivery slipped on two projects because of late client assets.',
      sections: [
        { heading: 'Highlights', bullets: ['Closed three annual retainers', 'Launched the new onboarding flow'] },
        { heading: 'Revenue by line', table: { head: ['Line', 'Q2', 'Q3'], rows: [['Design', '$70,000', '$82,500'], ['Strategy', '$51,000', '$56,700'], ['Support', '$34,000', '$45,000']] } },
      ],
    },
  },
  ticket: {
    id: 'ticket', label: 'Event ticket', description: 'One admission ticket with event details and a stub code.',
    variants: ['classic', 'minimal', 'modern'],
    sample: {
      event: 'Gulf Coast Makers Night', organizer: 'Harbor Arts Council', order: 'A-88213', holder: 'Jordan Reyes',
      date: 'Sat, Dec 12, 2026', time: 'Doors 6:30 PM', venue: 'The Pier Hall', address: '22 Pier Road, Galveston', section: 'GA', admission: '1 adult', price: '$25.00',
      code: 'HAC-88213-7F2K', terms: 'Non-transferable. Bring a photo ID. Doors close at 8:00 PM.',
    },
  },
  minutes: {
    id: 'minutes', label: 'Meeting minutes', description: 'Attendees, agenda, discussion with decisions, and action items.',
    variants: ['classic', 'minimal'],
    sample: {
      title: 'Product sync', org: 'Northwind Studio', date: 'Nov 9, 2026', time: '10:00 AM', location: 'Room 2 / video',
      attendees: ['Avery Park', 'Sam Okafor', 'Lena Ruiz'], absent: ['Chris Doyle'],
      agenda: ['Release plan', 'Hiring', 'Open items'],
      discussion: [{ topic: 'Release plan', notes: 'Beta moves one week to cover accessibility fixes.', decisions: ['Ship beta Nov 23'] }],
      actions: [{ task: 'Update the release calendar', owner: 'Sam', due: 'Nov 11' }, { task: 'Post the design role', owner: 'Lena', due: 'Nov 13' }],
      nextMeeting: 'Nov 16, 10:00 AM',
    },
  },
  receipt: {
    id: 'receipt', label: 'Receipt', description: 'Itemised proof of payment with tax and tip.',
    variants: ['minimal', 'classic', 'modern'],
    sample: {
      number: 'R-30991', date: 'Nov 9, 2026', method: 'Visa ending 4242', currency: 'USD', merchant: 'Harbor Coffee Co.', address: '22 Pier Road\nGalveston, TX', customer: 'Jordan Reyes',
      items: [{ name: 'Flat white', qty: 2, unitPrice: 4.5 }, { name: 'Almond croissant', qty: 1, unitPrice: 3.75 }], tax: 1.05, tip: 2,
      footer: 'Thank you. Keep this receipt for your records.',
    },
  },
};

export const PDF_PAGE_POINTS: Record<PdfPageSize, [number, number]> = { letter: [612, 792], a4: [595.28, 841.89] };

/** Validate a request's shape; returns an error message or null. */
export function validateRequest(req: unknown): string | null {
  const r = rec(req);
  if (!PDF_TEMPLATES.includes(r.template as PdfTemplateId)) return `template must be one of: ${PDF_TEMPLATES.join(', ')}`;
  if (r.data != null && (typeof r.data !== 'object' || Array.isArray(r.data))) return 'data must be an object';
  return null;
}

/** Turn a request into the block model both renderers draw. */
export function buildDocument(req: PdfRenderRequest): PdfDocModel {
  const info = PDF_TEMPLATE_INFO[req.template];
  const data = rec(req.data);
  const variant = pickVariant(req.variant, info.variants);
  const blocks = BUILDERS[req.template](data, variant);
  const head = blocks[0];
  const title = head?.t === 'masthead' ? head.title : info.label;
  return {
    title,
    template: req.template,
    theme: resolveTheme(req.theme),
    pageSize: req.pageSize === 'a4' ? 'a4' : 'letter',
    blocks,
    footer: str(data.footer) && req.template !== 'receipt' ? str(data.footer) : undefined,
  };
}
