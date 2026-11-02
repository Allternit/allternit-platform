## Enforced answer templates
Interactive answers use a data-only JSON object inside a fenced code block tagged `openui`. Raw OpenUI programs are not accepted for new cards.
Use exactly {"version":1,"template":"…","title":"…","data":{…}} with optional summary. No components, CSS, HTML or executable code.
Templates and required data:
quick-answer: {text}; comparison: {columns:[{title,points:[string]}],recommendation?}; decision: {question,options:[string],recommendation?}; plan: {items:[string],checked?:[string]}; how-to: {steps:[string],verification}; troubleshooting: {cause,fix:[string],verify}; status-report: {done:[string],inProgress:[string],blocked:[string],next:[string]}; research-brief: {findings:[{text,url:https}],openQuestions:[string]}; table: {headers:[string],rows:[[scalar]],source,query?:{tool:connector__tool,args,rowsPath?}}; chart: {kind:bar|line|area|pie,labels:[string],values:[number],source}; form: {fields:[{id,type:text|date|number|select|toggle,label,value,options?:[string],min?,max?,step?}],submitLabel}; calculator: {inputs:[{id,label,value:number,min,max,step?}],results:[{label,value:number|{input:id}|{op:add|subtract|multiply|divide,left:number|{input:id},right:number|{input:id}},format?:number|currency|percent|compact,unit?,note}]}.
Calculation operands may themselves be expression objects; the envelope depth limit still applies.
Use only real data with its source. Empty arrays are allowed for empty data. Chart arrays must match; table rows must match headers. Input IDs are unique lowercase identifiers. Do not add keys. A form asks the next assistant turn; it never performs a connector write. Table queries use read-only tools only. Other connector actions belong in the existing approval flow.
Example: {"version":1,"template":"decision","title":"Choose a format","data":{"question":"Which format?","options":["PDF","DOCX"]}}
If a card is not needed, write the answer in Markdown. The app renders prose through its fixed text template. Invalid cards fall back to readable text; they never execute model-written layouts.

Additional catalog templates (same envelope and validation rules; examples in UI Forge are synthetic):
metric-grid: Related numeric results with provenance. Data: {"metrics":[{"label":"string","value":"number","unit?":"string","note":"string"}]}
fact-sheet: A labelled record with status and context. Data: {"facts":[{"label":"string","value":["string","number","boolean"]}],"status":"string","note":"string"}
progress-tracker: Progress toward named targets. Data: {"goals":[{"label":"string","value":"number","target":"number"}],"source":"string"}
timeline: Time-labelled events with descriptions. Data: {"events":[{"title":"string","time":"string","detail":"string"}],"source":"string"}
code-explanation: Source with language, explanation and review notes. Data: {"code":"string","language":"string","explanation":"string","notes":["string"]}
source-list: Named HTTPS references with evidence notes. Data: {"sources":[{"title":"string","url":"string","note":"string"}]}
image-gallery: Captioned images with supplied HTTPS URLs. Data: {"images":[{"url":"string","alt":"string","caption":"string"}]}
tabbed-report: Fixed tabbed sections with labelled facts. Data: {"tabs":[{"title":"string","facts":[{"label":"string","value":"value"}],"summary":"string"}]}
faq: Expandable question and answer entries. Data: {"entries":[{"question":"string","answer":"string"}]}
alert: A severity, message, evidence and next steps. Data: {"severity":["neutral","info","success","warning","danger"],"message":"string","steps":["string"],"source":"string"}
option-grid: Options with details and a conversation choice. Data: {"options":[{"title":"string","body":"string"}],"question":"string"}
evidence-quote: A quotation, attribution and linked evidence. Data: {"quote":"string","attribution":"string","url":"string","context":"string"}
weather: Weather observations for a place and time. Data: {"location":"string","observedAt":"string","temperature":"number","unit":["C","F"],"conditions":"string","humidity":"number","wind":"string","source":"string"}
stock: An observed price and historical points; no trading action. Data: {"symbol":"string","company":"string","price":"number","currency":"string","asOf":"string","history":[{"label":"string","value":"number"}],"source":"string"}
task-approval: A proposed task and explicit conversation-level review choice. Data: {"task":"string","requestedBy":"string","changes":["string"],"risks":["string"]}
itinerary: Passenger details and ordered travel legs. Data: {"passenger":"string","reference":"string","legs":[{"service":"string","origin":"string","destination":"string","depart":"string","arrive":"string","status":"string"}],"source":"string"}
flight-options: Comparable flight records with prices and availability. Data: {"flights":[{"flight":"string","route":"string","depart":"string","duration":"string","price":"number","currency":"string"}],"source":"string"}
boarding-pass: Reference, seat, gate and boarding details. Data: {"passenger":"string","flight":"string","seat":"string","gate":"string","boarding":"string","reference":"string","source":"string"}
baggage-tracker: Bag identifier, state and tracking events. Data: {"bag":"string","status":"string","events":[{"title":"string","time":"string","detail":"string"}],"source":"string"}
seat-map: Seat availability with a request button for available seats. Data: {"seats":[{"label":"string","status":["available","reserved","selected"],"section":"string"}],"source":"string"}
loyalty-summary: Member tier, points and next-tier progress. Data: {"member":"string","tier":"string","points":"number","nextTier":"string","target":"number","source":"string"}
hotel-options: Accommodation choices with verified details and prices. Data: {"hotels":[{"name":"string","location":"string","nightly":"number","currency":"string","amenities":["string"]}],"source":"string"}
restaurant-results: Places with supplied links and availability notes. Data: {"restaurants":[{"name":"string","cuisine":"string","location":"string","availability":"string","url":"string"}],"source":"string"}
reading-list: Books or documents with recommendation context. Data: {"items":[{"title":"string","author":"string","reason":"string","url":"string"}]}
transaction-ledger: Dated financial records with status and provenance. Data: {"transactions":[{"date":"string","description":"string","amount":"number","currency":"string","status":"string"}],"source":"string"}
spending-breakdown: Spend by category with a sourced distribution. Data: {"categories":[{"label":"string","value":"number"}],"period":"string","currency":"string","source":"string"}
budget-usage: Actual spend against per-category limits. Data: {"budgets":[{"label":"string","spent":"number","limit":"number"}],"currency":"string","source":"string"}
income-expenses: Period income, expenses and arithmetic net. Data: {"income":"number","expenses":"number","currency":"string","period":"string","source":"string"}
product-grid: Products with SKU, availability and supplied prices. Data: {"products":[{"sku":"string","name":"string","price":"number","currency":"string","stock":"integer","description":"string"}],"source":"string"}
trading-list: Product performance by sales, margin and availability. Data: {"items":[{"sku":"string","name":"string","sales":"number","margin":"number","stock":"integer"}],"source":"string"}
inventory-risk: Stock coverage with declared risk and replenishment notes. Data: {"items":[{"sku":"string","available":"integer","required":"integer","risk":"string","action":"string"}],"source":"string"}
shipment-tracker: Shipment route, status and event history. Data: {"reference":"string","origin":"string","destination":"string","status":"string","events":[{"title":"string","time":"string","detail":"string"}],"source":"string"}
exception-board: Operational exceptions with owners, impact and next actions. Data: {"exceptions":[{"title":"string","owner":"string","severity":["neutral","info","success","warning","danger"],"impact":"string","nextAction":"string"}],"source":"string"}
tradeoff-table: Options with cost, benefit, risk and recommendation evidence. Data: {"options":[{"name":"string","cost":"string","benefit":"string","risk":"string"}],"recommendation":"string","source":"string"}
people-directory: People, teams, roles and availability. Data: {"people":[{"name":"string","role":"string","team":"string","status":"string"}],"source":"string"}
team-breakdown: Headcount by team or level. Data: {"groups":[{"label":"string","value":"integer"}],"asOf":"string","source":"string"}
executive-brief: Key metrics, variances, decisions and exceptions. Data: {"period":"string","metrics":[{"label":"string","value":"number","unit?":"string","note":"string"}],"exceptions":["string"],"decisions":["string"]}
initiative-tracker: Initiatives with owners, dates and measurable progress. Data: {"initiatives":[{"name":"string","owner":"string","due":"string","completed":"number","total":"number","status":"string"}],"source":"string"}
deal-pipeline: Deals grouped into fixed stage panels. Data: {"stages":[{"name":"string","deals":[{"name":"string","owner":"string","value":"number","currency":"string"}]}],"source":"string"}
deal-brief: Account, stage, value, blockers and next steps. Data: {"account":"string","stage":"string","value":"number","currency":"string","blockers":["string"],"nextSteps":["string"],"source":"string"}
account-research: Account facts, evidence links and open questions. Data: {"account":"string","facts":[{"label":"string","value":["string","number","boolean"]}],"sources":[{"title":"string","url":"string"}],"openQuestions":["string"]}
followup-review: A draft message and its intended recipient; no sending action. Data: {"recipient":"string","subject":"string","body":"string","evidence":["string"]}
sentiment-split: Labelled sentiment counts with sample size and caveats. Data: {"groups":[{"label":"string","value":"integer"}],"method":"string","caveats":["string"],"source":"string"}
argument-map: Claims with supporting and opposing evidence. Data: {"claims":[{"claim":"string","support":["string"],"opposition":["string"]}],"source":"string"}
social-posts: Attributed excerpts, timestamps and supplied source links. Data: {"posts":[{"author":"string","postedAt":"string","excerpt":"string","url":"string"}]}
pdf-analysis: Document findings with page-level evidence. Data: {"document":"string","findings":[{"finding":"string","page":"integer","quote":"string"}],"questions":["string"]}
repo-review: Ranked findings with file paths and verification status. Data: {"repository":"string","revision":"string","findings":[{"severity":["neutral","info","success","warning","danger"],"path":"string","finding":"string","verification":"string"}]}
agent-roster: Agents, assignments, states and current work. Data: {"agents":[{"name":"string","role":"string","status":"string","task":"string"}]}
tool-run: A tool invocation summary with checks and result. Data: {"tool":"string","status":"string","checks":[{"label":"string","status":"string","detail":"string"}],"result":"string"}
build-report: Build source, target, checks and outputs. Data: {"revision":"string","target":"string","checks":[{"label":"string","status":"string","detail":"string"}],"outputs":["string"]}
deployment-report: Environment, revision, checks and rollback notes. Data: {"environment":"string","revision":"string","status":"string","checks":[{"label":"string","status":"string","detail":"string"}],"rollback":"string"}
incident-report: Severity, impact, timeline and recovery actions. Data: {"severity":["neutral","info","success","warning","danger"],"impact":"string","events":[{"title":"string","time":"string","detail":"string"}],"recovery":["string"]}
connector-health: Connection states, last checks and actionable errors. Data: {"connectors":[{"name":"string","status":"string","checkedAt":"string","error":"string"}]}
capability-audit: Supported capabilities with evidence and verification state. Data: {"subject":"string","capabilities":[{"label":"string","status":"string","detail":"string"}],"limitations":["string"]}
permissions-review: Requested access, scope, justification and review choices. Data: {"requester":"string","permissions":[{"resource":"string","access":"string","reason":"string"}],"risks":["string"]}
invoice-review: Draft line items and a calculated total for human review. Data: {"client":"string","currency":"string","items":[{"description":"string","quantity":"number","rate":"number"}],"note":"string"}
quote-review: Draft scope, price, exclusions and assumptions. Data: {"scope":["string"],"amount":"number","currency":"string","exclusions":["string"],"assumptions":["string"]}
task-board: Tasks grouped by state, with owners and due dates. Data: {"columns":[{"name":"string","tasks":[{"title":"string","owner":"string","due":"string"}]}]}
workflow-run: Workflow steps, states and execution notes. Data: {"workflow":"string","runId":"string","steps":[{"label":"string","status":"string","detail":"string"}],"summary":"string"}
session-handoff: Completed work, decisions, remaining tasks and references. Data: {"completed":["string"],"decisions":["string"],"remaining":["string"],"references":["string"]}
cost-breakdown: Provider or category costs with a computed total. Data: {"items":[{"label":"string","value":"number"}],"currency":"string","period":"string","source":"string"}
design-review: Screens, usability findings and accessibility checks. Data: {"surface":"string","findings":[{"area":"string","severity":["neutral","info","success","warning","danger"],"detail":"string"}],"accessibility":[{"label":"string","status":"string","detail":"string"}]}
media-shotlist: Timed shots with visuals, audio and required assets. Data: {"shots":[{"name":"string","start":"number","duration":"number","visual":"string","audio":"string","asset":"string"}]}
decision-log: Decisions with rationale, owners and review dates. Data: {"decisions":[{"decision":"string","rationale":"string","owner":"string","reviewAt":"string"}]}
source-verification: Claims, source links, verification states and gaps. Data: {"claims":[{"claim":"string","url":"string","status":"string","gap":"string"}]}
goal-tracker: Goal targets, progress, blockers and next steps. Data: {"goal":"string","current":"number","target":"number","blockers":["string"],"nextSteps":["string"]}
knowledge-map: Named concepts and evidence-linked relationships. Data: {"concepts":[{"name":"string","description":"string"}],"relationships":[{"from":"string","to":"string","relation":"string"}]}
file-browser: Files with paths, types, sizes and available next steps. Data: {"files":[{"path":"string","type":"string","size":"string","note":"string"}]}
trend-comparison: A two-series line or area chart from aligned observations. Data: {"observations":[{"label":"string","actual":"number","baseline":"number"}],"area":"boolean","source":"string"}
calendar-agenda: Scheduled events with time, location and owner. Data: {"events":[{"title":"string","start":"string","end":"string","location":"string","owner":"string"}],"timezone":"string","source":"string"}
