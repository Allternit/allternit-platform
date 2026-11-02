/** Allternit's extensible, application-owned catalog. Model data never defines a layout. */
import { z } from 'zod/v4';

const word = z.string().min(1).max(240);
const text = z.string().max(16000);
const note = z.string().min(1).max(1000);
const strings = z.array(text).max(100);
const number = z.number().min(-1e100).max(1e100);
const https = z.url().regex(/^https:\/\//i);
const tone = z.enum(['neutral', 'info', 'success', 'warning', 'danger']);
const facts = z.array(z.strictObject({ label: word, value: z.union([text, number, z.boolean()]) })).max(40);
const points = z.array(z.strictObject({ label: word, value: number })).max(100);
const metric = z.strictObject({ label: word, value: number, unit: word.optional(), note });
const event = z.strictObject({ title: word, time: word, detail: text });
const checks = z.array(z.strictObject({ label: word, status: word, detail: text })).max(100);

/** Only instances created by trusted source code serialize as component calls. */
class Call { constructor(readonly name: string, readonly args: unknown[]) {} }
const call = (name: string, ...args: unknown[]) => new Call(name, args);
function emit(value: unknown): string {
  if (value instanceof Call) return `${value.name}(${value.args.map(emit).join(', ')})`;
  if (Array.isArray(value)) return `[${value.map(emit).join(', ')}]`;
  return JSON.stringify(value);
}
const caption = (value: string) => call('Text', value, 'muted');
const paragraph = (value: string) => call('Text', value);
const list = (items: string[]) => items.length ? call('List', items) : paragraph('No items.');
const section = (title: string, children: Call[]) => call('Section', title, children);
const factSheet = (items: z.infer<typeof facts>) => call('KeyValue', items);
const chart = (kind: 'bar' | 'line' | 'area' | 'pie', data: z.infer<typeof points>) => data.length
  ? call(({ bar: 'BarChart', line: 'LineChart', area: 'AreaChart', pie: 'PieChart' } as const)[kind], data.map(p => p.label), data.map(p => p.value))
  : paragraph('No chart data.');
const table = (headers: string[], rows: unknown[][]) => call('Table', headers, rows);
const panels = (items: Array<{title: string; body: string}>) => items.length
  ? call('Grid', items.map(item => call('Panel', item.title, [paragraph(item.body)])), 2, 'md') : paragraph('No records.');
const checkTable = (items: z.infer<typeof checks>) => table(['Check', 'Status', 'Detail'], items.map(c => [c.label,c.status,c.detail]));
const review = (question: string, approve: string, decline = 'Request changes') => call('Choice', question, [approve, decline]);
const UPSTREAM_COMMIT = 'd168487e7a753a1527256c7e4c79344377f3d092';
const upstream = (path: string) => `https://github.com/CopilotKit/CopilotKit/blob/${UPSTREAM_COMMIT}/${path}`;
const skin = (name: string) => upstream(`examples/showcases/reskinnable-demo/src/skins/${name}/catalog/${['airline','bookstore','keel','myelin'].includes(name) ? 'index.tsx' : 'definitions.ts'}`);
const sample = 'Synthetic example; replace with verified data';

export type CatalogCategory = 'Display' | 'Research' | 'Travel' | 'Finance' | 'Commerce' | 'People' | 'Operations' | 'Agents' | 'Development' | 'Planning';
export interface CatalogEntry<T extends string = string> {
  id: T; version: 1; name: string; category: CatalogCategory; description: string;
  origin: 'allternit' | 'copilotkit-reference'; reference?: string;
  schema: z.ZodType; example: Record<string, unknown>;
  compile(data: unknown): string[];
}
function define<const T extends string, S extends z.ZodType>(
  id: T, name: string, category: CatalogCategory, description: string, schema: S,
  example: z.input<S>, render: (data: z.output<S>) => Call[], reference?: string,
): CatalogEntry<T> {
  return Object.freeze({ id, name, category, description, version: 1 as const,
    origin: reference ? 'copilotkit-reference' as const : 'allternit' as const, reference,
    schema, example: example as Record<string, unknown>,
    compile: (data: unknown) => {
      const parsed = schema.parse(data);
      const views = render(parsed);
      const lists = Object.values(parsed as Record<string, unknown>).filter(Array.isArray);
      if (lists.length && lists.every(items=>items.length===0)) views.unshift(call('Callout','No list records supplied.','info'));
      return views.map(emit);
    },
  });
}

/** These are distinct data contracts and fixed views, not renamed prompt examples. */
export const CATALOG_TEMPLATES = [
  define('metric-grid','Metric grid','Display','Related numeric results with provenance.',
    z.strictObject({metrics:z.array(metric).max(12)}), {metrics:[{label:'Completed',value:8,note:sample}]},
    d=>[call('Row',d.metrics.map(m=>call('Metric',m.label,m.value,'number',m.unit??'',m.note)))]),
  define('fact-sheet','Fact sheet','Display','A labelled record with status and context.',
    z.strictObject({facts,status:word,note}),{facts:[{label:'Owner',value:'Example team'}],status:'Draft',note:sample},
    d=>[call('Badge',d.status,'info'),factSheet(d.facts),caption(d.note)]),
  define('progress-tracker','Progress tracker','Planning','Progress toward named targets.',
    z.strictObject({goals:z.array(z.strictObject({label:word,value:number.min(0),target:number.positive()})).max(20),source:note}),
    {goals:[{label:'Reviewed',value:3,target:10}],source:sample},d=>[...d.goals.map(g=>call('Progress',g.label,Math.min(g.value,g.target),g.target)),caption(d.source)]),
  define('timeline','Timeline','Display','Time-labelled events with descriptions.',z.strictObject({events:z.array(event).max(100),source:note}),
    {events:[{title:'Review',time:'09:00',detail:'Example review'}],source:sample},d=>[call('Timeline',d.events.map(e=>({title:e.title,time:e.time,detail:e.detail}))),caption(d.source)]),
  define('code-explanation','Code explanation','Development','Source with language, explanation and review notes.',
    z.strictObject({code:text,language:word,explanation:text,notes:strings}),{code:'const ready = true;',language:'typescript',explanation:'Example declaration.',notes:['Check real requirements.']},
    d=>[call('Code',d.code,d.language),call('Divider'),paragraph(d.explanation),list(d.notes)]),
  define('source-list','Source list','Research','Named HTTPS references with evidence notes.',
    z.strictObject({sources:z.array(z.strictObject({title:word,url:https,note:text})).max(50)}),{sources:[{title:'Example source',url:'https://example.com',note:sample}]},
    d=>d.sources.flatMap(s=>[call('Link',s.title,s.url),caption(s.note)])),
  define('image-gallery','Image gallery','Display','Captioned images with supplied HTTPS URLs.',
    z.strictObject({images:z.array(z.strictObject({url:https,alt:word,caption:text})).max(20)}),{images:[{url:'https://example.com/example.png',alt:'Example image',caption:sample}]},
    d=>[call('Grid',d.images.map(i=>call('Image',i.url,i.alt,i.caption)),2,'md')]),
  define('tabbed-report','Tabbed report','Display','Fixed tabbed sections with labelled facts.',
    z.strictObject({tabs:z.array(z.strictObject({title:word,facts,summary:text})).min(1).max(8)}),{tabs:[{title:'Overview',facts:[{label:'State',value:'Draft'}],summary:sample}]},
    d=>[call('Tabs',d.tabs.map(t=>call('Tab',t.title,[paragraph(t.summary),factSheet(t.facts)])))]),
  define('faq','Questions and answers','Display','Expandable question and answer entries.',
    z.strictObject({entries:z.array(z.strictObject({question:word,answer:text})).max(30)}),{entries:[{question:'How does this work?',answer:'This is a fixed template.'}]},
    d=>d.entries.map(e=>call('Collapsible',e.question,[paragraph(e.answer)],false))),
  define('alert','Alert','Operations','A severity, message, evidence and next steps.',
    z.strictObject({severity:tone,message:text,steps:strings,source:note}),{severity:'warning',message:'Example review needed.',steps:['Inspect the evidence.'],source:sample},
    d=>[call('Callout',d.message,d.severity==='neutral'?'info':d.severity),list(d.steps),caption(d.source)]),
  define('option-grid','Option grid','Planning','Options with details and a conversation choice.',
    z.strictObject({options:z.array(z.strictObject({title:word,body:text})).min(2).max(9),question:word}),
    {options:[{title:'A',body:'Example option A'},{title:'B',body:'Example option B'}],question:'Which option?'},
    d=>[panels(d.options),call('Choice',d.question,d.options.map(o=>o.title))]),
  define('evidence-quote','Evidence quote','Research','A quotation, attribution and linked evidence.',
    z.strictObject({quote:text,attribution:word,url:https,context:text}),{quote:'Example quotation.',attribution:'Example author',url:'https://example.com',context:sample},
    d=>[call('Quote',d.quote,d.attribution),call('Link','Source',d.url),caption(d.context)]),

  define('weather','Weather','Travel','Weather observations for a place and time.',
    z.strictObject({location:word,observedAt:word,temperature:number,unit:z.enum(['C','F']),conditions:word,humidity:number.min(0).max(100),wind:word,source:note}),
    {location:'Example city',observedAt:'Example observation',temperature:21,unit:'C',conditions:'Clear',humidity:40,wind:'Light',source:sample},
    d=>[call('Metric',d.location,d.temperature,'number',`°${d.unit}`,d.observedAt),factSheet([{label:'Conditions',value:d.conditions},{label:'Humidity',value:`${d.humidity}%`},{label:'Wind',value:d.wind}]),caption(d.source)],
    upstream('examples/showcases/generative-ui-playground/src/app/components/static-tools/WeatherCard.tsx')),
  define('stock','Stock summary','Finance','An observed price and historical points; no trading action.',
    z.strictObject({symbol:word,company:word,price:number.min(0),currency:word,asOf:word,history:points,source:note}),
    {symbol:'DEMO',company:'Example company',price:100,currency:'USD',asOf:'Example date',history:[{label:'Day 1',value:95},{label:'Day 2',value:100}],source:sample},
    d=>[call('Metric',`${d.company} (${d.symbol})`,d.price,'currency',d.currency,d.asOf),chart('line',d.history),caption(d.source)],upstream('examples/showcases/generative-ui-playground/src/app/components/static-tools/StockCard.tsx')),
  define('task-approval','Task review','Agents','A proposed task and explicit conversation-level review choice.',
    z.strictObject({task:word,requestedBy:word,changes:strings,risks:strings}),{task:'Example task',requestedBy:'Example agent',changes:['Review one file'],risks:['Unverified input']},
    d=>[factSheet([{label:'Task',value:d.task},{label:'Requested by',value:d.requestedBy}]),section('Changes',[list(d.changes)]),section('Risks',[list(d.risks)]),review('How should this task proceed?','Approve task for review')],upstream('examples/showcases/generative-ui-playground/src/app/components/static-tools/TaskApprovalCard.tsx')),
  define('itinerary','Itinerary','Travel','Passenger details and ordered travel legs.',
    z.strictObject({passenger:word,reference:word,legs:z.array(z.strictObject({service:word,origin:word,destination:word,depart:word,arrive:word,status:word})).max(20),source:note}),
    {passenger:'Example passenger',reference:'DEMO',legs:[{service:'Example flight',origin:'AAA',destination:'BBB',depart:'Example departure',arrive:'Example arrival',status:'Planned'}],source:sample},
    d=>[factSheet([{label:'Passenger',value:d.passenger},{label:'Reference',value:d.reference}]),table(['Service','From','To','Departure','Arrival','Status'],d.legs.map(l=>[l.service,l.origin,l.destination,l.depart,l.arrive,l.status])),caption(d.source)],skin('airline')),
  define('flight-options','Flight options','Travel','Comparable flight records with prices and availability.',
    z.strictObject({flights:z.array(z.strictObject({flight:word,route:word,depart:word,duration:word,price:number.min(0),currency:word})).max(30),source:note}),
    {flights:[{flight:'Example 1',route:'AAA → BBB',depart:'Example time',duration:'2h',price:200,currency:'USD'}],source:sample},
    d=>[table(['Flight','Route','Departure','Duration','Price','Currency'],d.flights.map(f=>[f.flight,f.route,f.depart,f.duration,f.price,f.currency])),caption(d.source)],skin('airline')),
  define('boarding-pass','Boarding details','Travel','Reference, seat, gate and boarding details.',
    z.strictObject({passenger:word,flight:word,seat:word,gate:word,boarding:word,reference:word,source:note}),
    {passenger:'Example passenger',flight:'Demo flight',seat:'12A',gate:'B2',boarding:'Example time',reference:'DEMO',source:sample},
    d=>[call('Badge',d.flight,'info'),factSheet([{label:'Passenger',value:d.passenger},{label:'Seat',value:d.seat},{label:'Gate',value:d.gate},{label:'Boarding',value:d.boarding},{label:'Reference',value:d.reference}]),caption(d.source)],skin('airline')),
  define('baggage-tracker','Baggage tracker','Travel','Bag identifier, state and tracking events.',
    z.strictObject({bag:word,status:word,events:z.array(event).max(30),source:note}),{bag:'DEMO BAG',status:'In transit',events:[{title:'Checked in',time:'Example time',detail:'Example terminal'}],source:sample},
    d=>[call('Badge',d.status,'info'),paragraph(d.bag),call('Timeline',d.events.map(e=>({title:e.title,time:e.time,detail:e.detail}))),caption(d.source)],skin('airline')),
  define('seat-map','Seat options','Travel','Seat availability with a request button for available seats.',
    z.strictObject({seats:z.array(z.strictObject({label:word,status:z.enum(['available','reserved','selected']),section:word})).max(100),source:note}),
    {seats:[{label:'12A',status:'available',section:'Economy'},{label:'12B',status:'reserved',section:'Economy'}],source:sample},
    d=>[call('Grid',d.seats.map(s=>call('Panel',s.label,[call('Badge',s.status,s.status==='available'?'success':'neutral'),caption(s.section),...(s.status==='available'?[call('Button',`Request seat ${s.label}`)]:[])])),4,'sm'),caption(d.source)],skin('airline')),
  define('loyalty-summary','Loyalty summary','Travel','Member tier, points and next-tier progress.',
    z.strictObject({member:word,tier:word,points:number.min(0),nextTier:word,target:number.positive(),source:note}),
    {member:'Example member',tier:'Basic',points:100,nextTier:'Next tier',target:500,source:sample},
    d=>[call('Badge',d.tier,'info'),paragraph(d.member),call('Progress',d.nextTier,Math.min(d.points,d.target),d.target),caption(d.source)],skin('airline')),
  define('hotel-options','Hotel options','Travel','Accommodation choices with verified details and prices.',
    z.strictObject({hotels:z.array(z.strictObject({name:word,location:word,nightly:number.min(0),currency:word,amenities:strings})).max(20),source:note}),
    {hotels:[{name:'Example hotel',location:'Example city',nightly:150,currency:'USD',amenities:['Example amenity']}],source:sample},
    d=>[call('Grid',d.hotels.map(h=>call('Panel',h.name,[caption(h.location),call('Metric','Nightly',h.nightly,'currency',h.currency,d.source),list(h.amenities)])),2,'md')],upstream('examples/showcases/mcp-apps/README.md')),
  define('restaurant-results','Restaurant results','Travel','Places with supplied links and availability notes.',
    z.strictObject({restaurants:z.array(z.strictObject({name:word,cuisine:word,location:word,availability:text,url:https})).max(30),source:note}),
    {restaurants:[{name:'Example place',cuisine:'Example cuisine',location:'Example location',availability:'Unconfirmed',url:'https://example.com'}],source:sample},
    d=>[call('Grid',d.restaurants.map(r=>call('Panel',r.name,[caption(`${r.cuisine} · ${r.location}`),paragraph(r.availability),call('Link','View place',r.url)])),2,'md'),caption(d.source)],upstream('examples/integrations/a2a-a2ui/README.md')),
  define('reading-list','Reading list','Research','Books or documents with recommendation context.',
    z.strictObject({items:z.array(z.strictObject({title:word,author:word,reason:text,url:https})).max(30)}),
    {items:[{title:'Example book',author:'Example author',reason:sample,url:'https://example.com'}]},
    d=>[call('Stack',d.items.map(i=>call('Panel',i.title,[caption(i.author),paragraph(i.reason),call('Link','Read more',i.url)])),'vertical','md')],skin('bookstore')),

  define('transaction-ledger','Transaction ledger','Finance','Dated financial records with status and provenance.',
    z.strictObject({transactions:z.array(z.strictObject({date:word,description:word,amount:number,currency:word,status:word})).max(100),source:note}),
    {transactions:[{date:'Example date',description:'Example expense',amount:25,currency:'USD',status:'Pending'}],source:sample},
    d=>[table(['Date','Description','Amount','Currency','Status'],d.transactions.map(t=>[t.date,t.description,t.amount,t.currency,t.status])),caption(d.source)],skin('banking')),
  define('spending-breakdown','Spending breakdown','Finance','Spend by category with a sourced distribution.',
    z.strictObject({categories:z.array(z.strictObject({label:word,value:number.min(0)})).max(40),period:word,currency:word,source:note}),
    {categories:[{label:'Example category',value:25}],period:'Example period',currency:'USD',source:sample},
    d=>[caption(`${d.period} · ${d.currency}`),chart('pie',d.categories),caption(d.source)],skin('banking')),
  define('budget-usage','Budget usage','Finance','Actual spend against per-category limits.',
    z.strictObject({budgets:z.array(z.strictObject({label:word,spent:number.min(0),limit:number.positive()})).max(30),currency:word,source:note}),
    {budgets:[{label:'Example budget',spent:25,limit:100}],currency:'USD',source:sample},
    d=>[...d.budgets.map(b=>section(b.label,[call('Progress','Used',Math.min(b.spent,b.limit),b.limit,'currency',d.currency),...(b.spent>b.limit?[call('Callout','Over budget','warning')]:[])])),caption(d.source)],skin('banking')),
  define('income-expenses','Income and expenses','Finance','Period income, expenses and arithmetic net.',
    z.strictObject({income:number.min(0),expenses:number.min(0),currency:word,period:word,source:note}),
    {income:100,expenses:70,currency:'USD',period:'Example period',source:sample},
    d=>[caption(d.period),call('Row',[call('Metric','Income',d.income,'currency',d.currency,d.source),call('Metric','Expenses',d.expenses,'currency',d.currency,d.source),call('Metric','Net',d.income-d.expenses,'currency',d.currency,'Income minus expenses')])],skin('banking')),
  define('product-grid','Product grid','Commerce','Products with SKU, availability and supplied prices.',
    z.strictObject({products:z.array(z.strictObject({sku:word,name:word,price:number.min(0),currency:word,stock:number.int().min(0),description:text})).max(40),source:note}),
    {products:[{sku:'DEMO',name:'Example product',price:20,currency:'USD',stock:3,description:'Example description'}],source:sample},
    d=>[call('Grid',d.products.map(p=>call('Panel',p.name,[caption(p.sku),paragraph(p.description),call('Metric','Price',p.price,'currency',p.currency,d.source),call('Badge',`${p.stock} available`,p.stock?'success':'warning')])),2,'md')],skin('commerce')),
  define('trading-list','Trading list','Commerce','Product performance by sales, margin and availability.',
    z.strictObject({items:z.array(z.strictObject({sku:word,name:word,sales:number.min(0),margin:number,stock:number.int().min(0)})).max(100),source:note}),
    {items:[{sku:'DEMO',name:'Example item',sales:10,margin:20,stock:3}],source:sample},
    d=>[table(['SKU','Product','Sales','Margin','Stock'],d.items.map(i=>[i.sku,i.name,i.sales,i.margin,i.stock])),caption(d.source)],skin('commerce')),
  define('inventory-risk','Inventory risk','Commerce','Stock coverage with declared risk and replenishment notes.',
    z.strictObject({items:z.array(z.strictObject({sku:word,available:number.int().min(0),required:number.int().min(0),risk:word,action:text})).max(100),source:note}),
    {items:[{sku:'DEMO',available:3,required:5,risk:'Shortfall',action:'Review replenishment'}],source:sample},
    d=>[table(['SKU','Available','Required','Risk','Action'],d.items.map(i=>[i.sku,i.available,i.required,i.risk,i.action])),caption(d.source)],skin('logistics')),
  define('shipment-tracker','Shipment tracker','Operations','Shipment route, status and event history.',
    z.strictObject({reference:word,origin:word,destination:word,status:word,events:z.array(event).max(50),source:note}),
    {reference:'DEMO',origin:'Example origin',destination:'Example destination',status:'In transit',events:[{title:'Dispatched',time:'Example time',detail:'Example warehouse'}],source:sample},
    d=>[call('Badge',d.status,'info'),factSheet([{label:'Reference',value:d.reference},{label:'Origin',value:d.origin},{label:'Destination',value:d.destination}]),call('Timeline',d.events.map(e=>({title:e.title,time:e.time,detail:e.detail}))),caption(d.source)],skin('logistics')),
  define('exception-board','Exception board','Operations','Operational exceptions with owners, impact and next actions.',
    z.strictObject({exceptions:z.array(z.strictObject({title:word,owner:word,severity:tone,impact:text,nextAction:text})).max(40),source:note}),
    {exceptions:[{title:'Example exception',owner:'Example team',severity:'warning',impact:'Example impact',nextAction:'Review the evidence'}],source:sample},
    d=>[call('Grid',d.exceptions.map(e=>call('Panel',e.title,[call('Badge',e.owner,e.severity),paragraph(e.impact),call('Callout',e.nextAction,'info')])),2,'md'),caption(d.source)],skin('logistics')),
  define('tradeoff-table','Trade-off table','Planning','Options with cost, benefit, risk and recommendation evidence.',
    z.strictObject({options:z.array(z.strictObject({name:word,cost:text,benefit:text,risk:text})).max(20),recommendation:text,source:note}),
    {options:[{name:'Example option',cost:'Example cost',benefit:'Example benefit',risk:'Example risk'}],recommendation:'Review the options.',source:sample},
    d=>[table(['Option','Cost','Benefit','Risk'],d.options.map(o=>[o.name,o.cost,o.benefit,o.risk])),call('Callout',d.recommendation,'info'),caption(d.source)],skin('logistics')),
  define('people-directory','People directory','People','People, teams, roles and availability.',
    z.strictObject({people:z.array(z.strictObject({name:word,role:word,team:word,status:word})).max(100),source:note}),
    {people:[{name:'Example person',role:'Example role',team:'Example team',status:'Available'}],source:sample},
    d=>[table(['Name','Role','Team','Status'],d.people.map(p=>[p.name,p.role,p.team,p.status])),caption(d.source)],skin('people')),
  define('team-breakdown','Team breakdown','People','Headcount by team or level.',
    z.strictObject({groups:z.array(z.strictObject({label:word,value:number.int().min(0)})).max(40),asOf:word,source:note}),
    {groups:[{label:'Example level',value:5}],asOf:'Example date',source:sample},
    d=>[caption(d.asOf),chart('bar',d.groups),caption(d.source)],skin('people')),
  define('executive-brief','Executive brief','Planning','Key metrics, variances, decisions and exceptions.',
    z.strictObject({period:word,metrics:z.array(metric).max(12),exceptions:strings,decisions:strings}),
    {period:'Example period',metrics:[{label:'Example metric',value:10,note:sample}],exceptions:['Example exception'],decisions:['Example decision']},
    d=>[caption(d.period),call('Row',d.metrics.map(m=>call('Metric',m.label,m.value,'number',m.unit??'',m.note))),section('Exceptions',[list(d.exceptions)]),section('Decisions',[list(d.decisions)])],skin('exec')),
  define('initiative-tracker','Initiative tracker','Planning','Initiatives with owners, dates and measurable progress.',
    z.strictObject({initiatives:z.array(z.strictObject({name:word,owner:word,due:word,completed:number.min(0),total:number.positive(),status:word})).max(30),source:note}),
    {initiatives:[{name:'Example initiative',owner:'Example owner',due:'Example date',completed:2,total:10,status:'In progress'}],source:sample},
    d=>[call('Stack',d.initiatives.map(i=>section(i.name,[caption(`${i.owner} · ${i.due}`),call('Badge',i.status,'info'),call('Progress','Progress',Math.min(i.completed,i.total),i.total)])),'vertical','md'),caption(d.source)],skin('exec')),
  define('deal-pipeline','Deal pipeline','Commerce','Deals grouped into fixed stage panels.',
    z.strictObject({stages:z.array(z.strictObject({name:word,deals:z.array(z.strictObject({name:word,owner:word,value:number.min(0),currency:word})).max(30)})).max(8),source:note}),
    {stages:[{name:'Review',deals:[{name:'Example deal',owner:'Example owner',value:100,currency:'USD'}]}],source:sample},
    d=>[call('Grid',d.stages.map(s=>call('Panel',s.name,[...s.deals.map(x=>section(x.name,[caption(x.owner),call('Metric','Value',x.value,'currency',x.currency,d.source)]))])),3,'md')],upstream('examples/showcases/strands-crm/frontend/components/PipelineBoard.tsx')),
  define('deal-brief','Deal brief','Commerce','Account, stage, value, blockers and next steps.',
    z.strictObject({account:word,stage:word,value:number.min(0),currency:word,blockers:strings,nextSteps:strings,source:note}),
    {account:'Example account',stage:'Review',value:100,currency:'USD',blockers:['Unverified input'],nextSteps:['Review source'],source:sample},
    d=>[call('Badge',d.stage,'info'),call('Metric',d.account,d.value,'currency',d.currency,d.source),section('Blockers',[list(d.blockers)]),section('Next steps',[list(d.nextSteps)])],upstream('examples/showcases/strands-crm/frontend/components/DealBriefCard.tsx')),
  define('account-research','Account research','Research','Account facts, evidence links and open questions.',
    z.strictObject({account:word,facts,sources:z.array(z.strictObject({title:word,url:https})).max(30),openQuestions:strings}),
    {account:'Example account',facts:[{label:'State',value:'Unverified'}],sources:[{title:'Example source',url:'https://example.com'}],openQuestions:['Confirm details']},
    d=>[paragraph(d.account),factSheet(d.facts),...d.sources.map(s=>call('Link',s.title,s.url)),section('Open questions',[list(d.openQuestions)])],upstream('examples/showcases/strands-crm/frontend/components/AccountResearch.tsx')),
  define('followup-review','Follow-up review','Commerce','A draft message and its intended recipient; no sending action.',
    z.strictObject({recipient:word,subject:word,body:text,evidence:strings}),{recipient:'Example recipient',subject:'Example follow-up',body:'Draft message for review.',evidence:[sample]},
    d=>[factSheet([{label:'Recipient',value:d.recipient},{label:'Subject',value:d.subject}]),paragraph(d.body),list(d.evidence),review('How should this draft change?','Keep this draft')],upstream('examples/showcases/strands-crm/frontend/components/FollowupApprovalCard.tsx')),
  define('sentiment-split','Sentiment split','Research','Labelled sentiment counts with sample size and caveats.',
    z.strictObject({groups:z.array(z.strictObject({label:word,value:number.int().min(0)})).max(10),method:text,caveats:strings,source:note}),
    {groups:[{label:'Positive',value:3},{label:'Neutral',value:2}],method:'Example classification',caveats:['Synthetic sample'],source:sample},
    d=>[chart('pie',d.groups),caption(d.method),section('Caveats',[list(d.caveats)]),caption(d.source)],upstream('examples/showcases/grok-generative-ui/components/SentimentSplit.tsx')),
  define('argument-map','Argument map','Research','Claims with supporting and opposing evidence.',
    z.strictObject({claims:z.array(z.strictObject({claim:word,support:strings,opposition:strings})).max(20),source:note}),
    {claims:[{claim:'Example claim',support:['Example evidence'],opposition:['Example counterpoint']}],source:sample},
    d=>[call('Stack',d.claims.map(c=>call('Panel',c.claim,[section('Supports',[list(c.support)]),section('Opposes',[list(c.opposition)])])),'vertical','md'),caption(d.source)],upstream('examples/showcases/grok-generative-ui/components/ArgumentMap.tsx')),
  define('social-posts','Social evidence','Research','Attributed excerpts, timestamps and supplied source links.',
    z.strictObject({posts:z.array(z.strictObject({author:word,postedAt:word,excerpt:text,url:https})).max(30)}),
    {posts:[{author:'Example author',postedAt:'Example date',excerpt:'Synthetic excerpt.',url:'https://example.com'}]},
    d=>[call('Stack',d.posts.map(p=>call('Panel',p.author,[caption(p.postedAt),call('Quote',p.excerpt,p.author),call('Link','Original',p.url)])),'vertical','md')],upstream('examples/showcases/grok-generative-ui/components/XPost.tsx')),
  define('pdf-analysis','Document analysis','Research','Document findings with page-level evidence.',
    z.strictObject({document:word,findings:z.array(z.strictObject({finding:text,page:number.int().min(1),quote:text})).max(50),questions:strings}),
    {document:'Example document',findings:[{finding:'Example finding',page:1,quote:'Synthetic excerpt.'}],questions:['Verify against the original.']},
    d=>[paragraph(d.document),...d.findings.map(f=>section(`Page ${f.page}`,[paragraph(f.finding),call('Quote',f.quote,d.document)])),section('Questions',[list(d.questions)])],upstream('examples/showcases/a2ui-pdf-analyst/README.md')),
  define('repo-review','Repository review','Development','Ranked findings with file paths and verification status.',
    z.strictObject({repository:word,revision:word,findings:z.array(z.strictObject({severity:tone,path:word,finding:text,verification:text})).max(50)}),
    {repository:'Example repository',revision:'Example revision',findings:[{severity:'warning',path:'example.ts',finding:'Example finding',verification:'Not run'}]},
    d=>[caption(`${d.repository} · ${d.revision}`),...d.findings.map(f=>call('Panel',f.path,[call('Badge',f.severity,f.severity),paragraph(f.finding),caption(f.verification)]))],upstream('examples/showcases/antigravity-repo-doctor/README.md')),

  define('agent-roster','Agent roster','Agents','Agents, assignments, states and current work.',
    z.strictObject({agents:z.array(z.strictObject({name:word,role:word,status:word,task:text})).max(50)}),
    {agents:[{name:'Example agent',role:'Reviewer',status:'Ready',task:'Example review'}]},
    d=>[call('Grid',d.agents.map(a=>call('Panel',a.name,[call('Badge',a.status,'info'),caption(a.role),paragraph(a.task)])),2,'md')]),
  define('tool-run','Tool run','Agents','A tool invocation summary with checks and result.',
    z.strictObject({tool:word,status:word,checks,result:text}),{tool:'Example tool',status:'Complete',checks:[{label:'Validation',status:'Passed',detail:sample}],result:'Example result'},
    d=>[call('Badge',d.status,'info'),paragraph(d.tool),checkTable(d.checks),paragraph(d.result)]),
  define('build-report','Build report','Development','Build source, target, checks and outputs.',
    z.strictObject({revision:word,target:word,checks,outputs:strings}),{revision:'Example revision',target:'Example target',checks:[{label:'Compile',status:'Not run',detail:'Example only'}],outputs:['Example artifact']},
    d=>[factSheet([{label:'Revision',value:d.revision},{label:'Target',value:d.target}]),checkTable(d.checks),section('Outputs',[list(d.outputs)])]),
  define('deployment-report','Deployment report','Development','Environment, revision, checks and rollback notes.',
    z.strictObject({environment:word,revision:word,status:word,checks,rollback:text}),{environment:'Preview',revision:'Example revision',status:'Not deployed',checks:[],rollback:'Example rollback note'},
    d=>[call('Badge',d.status,'info'),factSheet([{label:'Environment',value:d.environment},{label:'Revision',value:d.revision}]),checkTable(d.checks),section('Rollback',[paragraph(d.rollback)])]),
  define('incident-report','Incident report','Operations','Severity, impact, timeline and recovery actions.',
    z.strictObject({severity:tone,impact:text,events:z.array(event).max(50),recovery:strings}),{severity:'warning',impact:'Example impact',events:[],recovery:['Example recovery step']},
    d=>[call('Callout',d.impact,d.severity==='neutral'?'info':d.severity),call('Timeline',d.events.map(e=>({title:e.title,time:e.time,detail:e.detail}))),section('Recovery',[list(d.recovery)])]),
  define('connector-health','Connector health','Operations','Connection states, last checks and actionable errors.',
    z.strictObject({connectors:z.array(z.strictObject({name:word,status:word,checkedAt:word,error:text})).max(50)}),
    {connectors:[{name:'Example connector',status:'Unverified',checkedAt:'Example time',error:'Example only'}]},
    d=>[table(['Connector','State','Checked','Error'],d.connectors.map(c=>[c.name,c.status,c.checkedAt,c.error]))]),
  define('capability-audit','Capability audit','Agents','Supported capabilities with evidence and verification state.',
    z.strictObject({subject:word,capabilities:checks,limitations:strings}),{subject:'Example runtime',capabilities:[{label:'Read files',status:'Verified',detail:sample}],limitations:['Example limitation']},
    d=>[paragraph(d.subject),checkTable(d.capabilities),section('Limits',[list(d.limitations)])]),
  define('permissions-review','Permissions review','Agents','Requested access, scope, justification and review choices.',
    z.strictObject({requester:word,permissions:z.array(z.strictObject({resource:word,access:word,reason:text})).max(30),risks:strings}),
    {requester:'Example agent',permissions:[{resource:'Example folder',access:'Read',reason:'Example task'}],risks:['Review scope']},
    d=>[paragraph(d.requester),table(['Resource','Access','Reason'],d.permissions.map(p=>[p.resource,p.access,p.reason])),list(d.risks),review('How should this access request proceed?','Review requested access')]),
  define('invoice-review','Invoice review','Finance','Draft line items and a calculated total for human review.',
    z.strictObject({client:word,currency:word,items:z.array(z.strictObject({description:word,quantity:number.min(0),rate:number.min(0)})).max(100),note:text}),
    {client:'Example client',currency:'USD',items:[{description:'Example service',quantity:2,rate:50}],note:'Draft only'},
    d=>[call('Badge','Draft','warning'),paragraph(d.client),table(['Item','Quantity','Rate','Amount'],d.items.map(i=>[i.description,i.quantity,i.rate,i.quantity*i.rate])),call('Metric','Total',d.items.reduce((sum,i)=>sum+i.quantity*i.rate,0),'currency',d.currency,'Calculated from line items'),paragraph(d.note),review('Review this draft invoice','Keep for human review')]),
  define('quote-review','Quote review','Commerce','Draft scope, price, exclusions and assumptions.',
    z.strictObject({scope:strings,amount:number.min(0),currency:word,exclusions:strings,assumptions:strings}),
    {scope:['Example scoped task'],amount:100,currency:'USD',exclusions:['Example exclusion'],assumptions:['Audit required where applicable']},
    d=>[call('Badge','Draft','warning'),list(d.scope),call('Metric','Draft price',d.amount,'currency',d.currency,'Provided draft amount'),section('Exclusions',[list(d.exclusions)]),section('Assumptions',[list(d.assumptions)]),review('Review the draft scope','Keep for human review')]),
  define('task-board','Task board','Planning','Tasks grouped by state, with owners and due dates.',
    z.strictObject({columns:z.array(z.strictObject({name:word,tasks:z.array(z.strictObject({title:word,owner:word,due:word})).max(40)})).max(8)}),
    {columns:[{name:'To do',tasks:[{title:'Example task',owner:'Example owner',due:'Example date'}]}]},
    d=>[call('Grid',d.columns.map(c=>call('Panel',c.name,c.tasks.map(t=>section(t.title,[caption(`${t.owner} · ${t.due}`)])))),3,'md')]),
  define('workflow-run','Workflow run','Operations','Workflow steps, states and execution notes.',
    z.strictObject({workflow:word,runId:word,steps:checks,summary:text}),{workflow:'Example workflow',runId:'DEMO',steps:[{label:'Review',status:'Pending',detail:sample}],summary:'Example run'},
    d=>[factSheet([{label:'Workflow',value:d.workflow},{label:'Run',value:d.runId}]),checkTable(d.steps),paragraph(d.summary)]),
  define('session-handoff','Session handoff','Agents','Completed work, decisions, remaining tasks and references.',
    z.strictObject({completed:strings,decisions:strings,remaining:strings,references:strings}),
    {completed:['Example completed step'],decisions:['Example decision'],remaining:['Example next step'],references:['Example revision']},
    d=>[section('Completed',[list(d.completed)]),section('Decisions',[list(d.decisions)]),section('Remaining',[list(d.remaining)]),section('References',[list(d.references)])]),
  define('cost-breakdown','Cost breakdown','Finance','Provider or category costs with a computed total.',
    z.strictObject({items:z.array(z.strictObject({label:word,value:number.min(0)})).max(50),currency:word,period:word,source:note}),
    {items:[{label:'Example category',value:2}],currency:'USD',period:'Example period',source:sample},
    d=>[caption(d.period),chart('bar',d.items),call('Metric','Total',d.items.reduce((sum,i)=>sum+i.value,0),'currency',d.currency,d.source)]),
  define('design-review','Design review','Development','Screens, usability findings and accessibility checks.',
    z.strictObject({surface:word,findings:z.array(z.strictObject({area:word,severity:tone,detail:text})).max(40),accessibility:checks}),
    {surface:'Example screen',findings:[{area:'Example control',severity:'warning',detail:'Example finding'}],accessibility:[]},
    d=>[paragraph(d.surface),...d.findings.map(f=>call('Panel',f.area,[call('Badge',f.severity,f.severity),paragraph(f.detail)])),section('Accessibility',[checkTable(d.accessibility)])]),
  define('media-shotlist','Media shot list','Planning','Timed shots with visuals, audio and required assets.',
    z.strictObject({shots:z.array(z.strictObject({name:word,start:number.min(0),duration:number.positive(),visual:text,audio:text,asset:word})).max(100)}),
    {shots:[{name:'Example shot',start:0,duration:3,visual:'Example scene',audio:'Example sound',asset:'Example asset'}]},
    d=>[table(['Shot','Start','Duration','Visual','Audio','Asset'],d.shots.map(s=>[s.name,s.start,s.duration,s.visual,s.audio,s.asset]))]),
  define('decision-log','Decision log','Planning','Decisions with rationale, owners and review dates.',
    z.strictObject({decisions:z.array(z.strictObject({decision:word,rationale:text,owner:word,reviewAt:word})).max(50)}),
    {decisions:[{decision:'Example decision',rationale:'Example rationale',owner:'Example owner',reviewAt:'Example date'}]},
    d=>[table(['Decision','Rationale','Owner','Review'],d.decisions.map(x=>[x.decision,x.rationale,x.owner,x.reviewAt]))]),
  define('source-verification','Source verification','Research','Claims, source links, verification states and gaps.',
    z.strictObject({claims:z.array(z.strictObject({claim:word,url:https,status:word,gap:text})).max(50)}),
    {claims:[{claim:'Example claim',url:'https://example.com',status:'Unverified',gap:'Inspect original source'}]},
    d=>d.claims.map(c=>call('Panel',c.claim,[call('Badge',c.status,'info'),call('Link','Evidence',c.url),caption(c.gap)]))),
  define('goal-tracker','Goal tracker','Planning','Goal targets, progress, blockers and next steps.',
    z.strictObject({goal:word,current:number.min(0),target:number.positive(),blockers:strings,nextSteps:strings}),
    {goal:'Example goal',current:2,target:10,blockers:['Example blocker'],nextSteps:['Example next action']},
    d=>[call('Progress',d.goal,Math.min(d.current,d.target),d.target),section('Blockers',[list(d.blockers)]),section('Next steps',[list(d.nextSteps)])]),
  define('knowledge-map','Knowledge relationships','Research','Named concepts and evidence-linked relationships.',
    z.strictObject({concepts:z.array(z.strictObject({name:word,description:text})).max(50),relationships:z.array(z.strictObject({from:word,to:word,relation:word})).max(100)}),
    {concepts:[{name:'Example A',description:'First concept'},{name:'Example B',description:'Second concept'}],relationships:[{from:'Example A',to:'Example B',relation:'Supports'}]},
    d=>[panels(d.concepts.map(c=>({title:c.name,body:c.description}))),table(['From','Relationship','To'],d.relationships.map(r=>[r.from,r.relation,r.to]))]),
  define('file-browser','File results','Development','Files with paths, types, sizes and available next steps.',
    z.strictObject({files:z.array(z.strictObject({path:word,type:word,size:word,note:text})).max(100)}),
    {files:[{path:'example.md',type:'Markdown',size:'1 KiB',note:sample}]},
    d=>[table(['Path','Type','Size','Note'],d.files.map(f=>[f.path,f.type,f.size,f.note]))]),
  define('trend-comparison','Actual and baseline trend','Display','A two-series line or area chart from aligned observations.',
    z.strictObject({observations:z.array(z.strictObject({label:word,actual:number,baseline:number})).max(100),area:z.boolean(),source:note}),
    {observations:[{label:'A',actual:3,baseline:4},{label:'B',actual:5,baseline:4}],area:true,source:sample},
    d=>[d.observations.length?call(d.area?'AreaChart':'LineChart',d.observations.map(p=>p.label),null,[{name:'Actual',values:d.observations.map(p=>p.actual)},{name:'Baseline',values:d.observations.map(p=>p.baseline)}]):paragraph('No observations.'),caption(d.source)],skin('exec')),
  define('calendar-agenda','Calendar agenda','Planning','Scheduled events with time, location and owner.',
    z.strictObject({events:z.array(z.strictObject({title:word,start:word,end:word,location:word,owner:word})).max(100),timezone:word,source:note}),
    {events:[{title:'Example review',start:'09:00',end:'09:30',location:'Example location',owner:'Example owner'}],timezone:'Example timezone',source:sample},
    d=>[caption(d.timezone),table(['Event','Start','End','Location','Owner'],d.events.map(e=>[e.title,e.start,e.end,e.location,e.owner])),caption(d.source)]),
] as const;

export type CatalogTemplateId = typeof CATALOG_TEMPLATES[number]['id'];
export interface CatalogEnvelope { version: 1; template: CatalogTemplateId; title: string; summary?: string; data: Record<string, unknown> }
const ids = new Set<string>();
for (const entry of CATALOG_TEMPLATES) {
  if (ids.has(entry.id)) throw new Error(`Duplicate template: ${entry.id}`);
  ids.add(entry.id);
  entry.schema.parse(entry.example);
}
export const CATALOG_BY_ID = new Map<string, CatalogEntry>(CATALOG_TEMPLATES.map(t=>[t.id,t]));
export const CatalogAnswerContract = z.union(CATALOG_TEMPLATES.map(t=>z.strictObject({version:z.literal(1),template:z.literal(t.id),title:word,summary:text.optional(),data:t.schema})) as unknown as [z.ZodType,...z.ZodType[]]) as z.ZodType<CatalogEnvelope>;
function shape(schema: Record<string, any>, depth = 0): unknown {
  if (depth > 4) return schema.type ?? 'value';
  if (schema.enum) return schema.enum;
  if (schema.anyOf) return schema.anyOf.map((s: Record<string, any>) => shape(s,depth+1));
  if (schema.type === 'array') return [shape(schema.items,depth+1)];
  if (schema.properties) return Object.fromEntries(Object.entries(schema.properties).map(([k,v])=>[k+(schema.required?.includes(k)?'':'?'),shape(v as Record<string, any>,depth+1)]));
  return schema.type ?? 'value';
}
export const catalogManual = () => CATALOG_TEMPLATES.map(t=>`${t.id}: ${t.description} Data: ${JSON.stringify(shape(z.toJSONSchema(t.schema)))}`).join('\n');
