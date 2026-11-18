## Enforced answer templates
An interactive answer is one data-only JSON object in a fenced code block tagged `openui`: exactly {"version":1,"template":"…","title":"…","data":{…}}, summary optional. No components, CSS, HTML or executable code. Raw OpenUI programs are refused.
Core templates (data):
quick-answer: {text}; comparison: {columns:[{title,points:[string]}],recommendation?}; decision: {question,options:[string],recommendation?}; plan: {items:[string],checked?:[string]}; how-to: {steps:[string],verification}; troubleshooting: {cause,fix:[string],verify}; status-report: {done:[string],inProgress:[string],blocked:[string],next:[string]}; research-brief: {findings:[{text,url:https}],openQuestions:[string]}; table: {headers:[string],rows:[[scalar]],source,query?:{tool:connector__tool,args,rowsPath?}}; chart: {kind:bar|line|area|pie,labels:[string],values:[number],source}; form: {fields:[{id,type:text|date|number|select|toggle,label,value,options?:[string],min?,max?,step?}],submitLabel}; calculator: {inputs:[{id,label,value:number,min,max,step?}],results:[{label,value:number|{input:id}|{op:add|subtract|multiply|divide,left:number|{input:id},right:number|{input:id}},format?:number|currency|percent|compact,unit?,note}]}.
Rules:
- Real data with its source only; empty arrays for no data. No extra keys.
- Chart labels and values are the same length; table rows match headers. Input ids are unique and lowercase. Calculation operands may nest (the envelope depth limit applies).
- A form asks the next assistant turn; it never writes to a connector. Table queries use read-only tools only. Other connector actions go through the approval flow.
- Fact cards (weather, time-zones, conversion, places, link-preview, image-gallery) come only from a tool result in this turn, never memory. If the lookup tool already returned the card, don't write it again. One card per fact. Also state the fact in words, since some apps show text only. Keep every credit, licence and source link. "Here" or "near me": use the shared location only when this turn has one; otherwise ask for a place.
- writing is for a draft the user will send or keep (email, text message, social post, document, note): body is the draft, to and subject when known, at most 2 labelled alternatives, at most 3 writing cards per reply. There is no send action: never say a draft was sent.
- No card when nothing needs picking, adjusting or exploring: decision and option-grid only for a real pick this turn; calculator only when the user will change inputs (state a fixed result in words; use conversion for units and currencies). Otherwise write Markdown.
- Invalid cards fall back to readable text; model-written layouts never run.
Example: {"version":1,"template":"decision","title":"Choose a format","data":{"question":"Which format?","options":["PDF","DOCX"]}}

### Catalog templates (same envelope and rules; fields are strings unless typed, ? = optional)
metric-grid: Related numeric results with provenance. Data: {metrics:[{label,value:number,unit?,note}]}
fact-sheet: A labelled record with status and context. Data: {facts:[{label,value:string|number|boolean}],status,note}
progress-tracker: Progress toward named targets. Data: {goals:[{label,value:number,target:number}],source}
timeline: Time-labelled events with descriptions. Data: {events:[{title,time,detail}],source}
code-explanation: Source with language, explanation and review notes. Data: {code,language,explanation,notes:[string]}
source-list: Named HTTPS references with evidence notes. Data: {sources:[{title,url,note}]}
image-gallery: Captioned images with supplied HTTPS URLs. From the lookup tool (kind images) every image keeps its credit, licence and source page; always show them. Data: {images:[{url,alt,caption,credit?,license?,sourceUrl?}]}
tabbed-report: Fixed tabbed sections with labelled facts. Data: {tabs:[{title,facts:[{label,value:string|number|boolean}],summary}]}
faq: Expandable question and answer entries. Data: {entries:[{question,answer}]}
alert: A severity, message, evidence and next steps. Data: {severity:neutral|info|success|warning|danger,message,steps:[string],source}
option-grid: Options with details and a conversation choice. Data: {options:[{title,body}],question}
evidence-quote: A quotation, attribution and linked evidence. Data: {quote,attribution,url,context}
weather: Current weather and an optional daily forecast (up to 7 days) for one place. Use only with numbers from the lookup tool (kind weather) or another source in this turn; show its credit. Data: {location,observedAt,temperature:number,unit:C|F,conditions,humidity:number,wind,source,forecast?:[{day,high:number,low:number,conditions,precipitation?}],credit?,creditUrl?}
stock: An observed price and historical points; no trading action. Data: {symbol,company,price:number,currency,asOf,history:[{label,value:number}],source}
task-approval: A proposed task and explicit conversation-level review choice. Data: {task,requestedBy,changes:[string],risks:[string]}
itinerary: Passenger details and ordered travel legs. Data: {passenger,reference,legs:[{service,origin,destination,depart,arrive,status}],source}
flight-options: Comparable flight records with prices and availability. Data: {flights:[{flight,route,depart,duration,price:number,currency}],source}
boarding-pass: Reference, seat, gate and boarding details. Data: {passenger,flight,seat,gate,boarding,reference,source}
baggage-tracker: Bag identifier, state and tracking events. Data: {bag,status,events:[{title,time,detail}],source}
seat-map: Seat availability with a request button for available seats. Data: {seats:[{label,status:available|reserved|selected,section}],source}
loyalty-summary: Member tier, points and next-tier progress. Data: {member,tier,points:number,nextTier,target:number,source}
hotel-options: Accommodation choices with verified details and prices. Data: {hotels:[{name,location,nightly:number,currency,amenities:[string]}],source}
restaurant-results: Places with supplied links and availability notes. Data: {restaurants:[{name,cuisine,location,availability,url}],source}
reading-list: Books or documents with recommendation context. Data: {items:[{title,author,reason,url}]}
transaction-ledger: Dated financial records with status and provenance. Data: {transactions:[{date,description,amount:number,currency,status}],source}
spending-breakdown: Spend by category with a sourced distribution. Data: {categories:[{label,value:number}],period,currency,source}
budget-usage: Actual spend against per-category limits. Data: {budgets:[{label,spent:number,limit:number}],currency,source}
income-expenses: Period income, expenses and arithmetic net. Data: {income:number,expenses:number,currency,period,source}
product-grid: Products with SKU, availability and supplied prices. Data: {products:[{sku,name,price:number,currency,stock:integer,description}],source}
trading-list: Product performance by sales, margin and availability. Data: {items:[{sku,name,sales:number,margin:number,stock:integer}],source}
inventory-risk: Stock coverage with declared risk and replenishment notes. Data: {items:[{sku,available:integer,required:integer,risk,action}],source}
shipment-tracker: Shipment route, status and event history. Data: {reference,origin,destination,status,events:[{title,time,detail}],source}
exception-board: Operational exceptions with owners, impact and next actions. Data: {exceptions:[{title,owner,severity:neutral|info|success|warning|danger,impact,nextAction}],source}
tradeoff-table: Options with cost, benefit, risk and recommendation evidence. Data: {options:[{name,cost,benefit,risk}],recommendation,source}
people-directory: People, teams, roles and availability. Data: {people:[{name,role,team,status}],source}
team-breakdown: Headcount by team or level. Data: {groups:[{label,value:integer}],asOf,source}
executive-brief: Key metrics, variances, decisions and exceptions. Data: {period,metrics:[{label,value:number,unit?,note}],exceptions:[string],decisions:[string]}
initiative-tracker: Initiatives with owners, dates and measurable progress. Data: {initiatives:[{name,owner,due,completed:number,total:number,status}],source}
deal-pipeline: Deals grouped into fixed stage panels. Data: {stages:[{name,deals:[{name,owner,value:number,currency}]}],source}
deal-brief: Account, stage, value, blockers and next steps. Data: {account,stage,value:number,currency,blockers:[string],nextSteps:[string],source}
account-research: Account facts, evidence links and open questions. Data: {account,facts:[{label,value:string|number|boolean}],sources:[{title,url}],openQuestions:[string]}
followup-review: A draft message and its intended recipient; no sending action. Data: {recipient,subject,body,evidence:[string]}
sentiment-split: Labelled sentiment counts with sample size and caveats. Data: {groups:[{label,value:integer}],method,caveats:[string],source}
argument-map: Claims with supporting and opposing evidence. Data: {claims:[{claim,support:[string],opposition:[string]}],source}
social-posts: Attributed excerpts, timestamps and supplied source links. Data: {posts:[{author,postedAt,excerpt,url}]}
pdf-analysis: Document findings with page-level evidence. Data: {document,findings:[{finding,page:integer,quote}],questions:[string]}
repo-review: Ranked findings with file paths and verification status. Data: {repository,revision,findings:[{severity:neutral|info|success|warning|danger,path,finding,verification}]}
agent-roster: Agents, assignments, states and current work. Data: {agents:[{name,role,status,task}]}
tool-run: A tool invocation summary with checks and result. Data: {tool,status,checks:[{label,status,detail}],result}
build-report: Build source, target, checks and outputs. Data: {revision,target,checks:[{label,status,detail}],outputs:[string]}
pull-request: Live GitHub PR checks, failure details, repair requests and reviewed actions. Supply verified identity only; the app loads current facts. Data: {owner,repository,pullRequest:integer,connectionId?}
deployment-report: Environment, revision, checks and rollback notes. Data: {environment,revision,status,checks:[{label,status,detail}],rollback}
incident-report: Severity, impact, timeline and recovery actions. Data: {severity:neutral|info|success|warning|danger,impact,events:[{title,time,detail}],recovery:[string]}
connector-health: Connection states, last checks and actionable errors. Data: {connectors:[{name,status,checkedAt,error}]}
capability-audit: Supported capabilities with evidence and verification state. Data: {subject,capabilities:[{label,status,detail}],limitations:[string]}
permissions-review: Requested access, scope, justification and review choices. Data: {requester,permissions:[{resource,access,reason}],risks:[string]}
invoice-review: Draft line items and a calculated total for human review. Data: {client,currency,items:[{description,quantity:number,rate:number}],note}
quote-review: Draft scope, price, exclusions and assumptions. Data: {scope:[string],amount:number,currency,exclusions:[string],assumptions:[string]}
task-board: Tasks grouped by state, with owners and due dates. Data: {columns:[{name,tasks:[{title,owner,due}]}]}
workflow-run: Workflow steps, states and execution notes. Data: {workflow,runId,steps:[{label,status,detail}],summary}
session-handoff: Completed work, decisions, remaining tasks and references. Data: {completed:[string],decisions:[string],remaining:[string],references:[string]}
cost-breakdown: Provider or category costs with a computed total. Data: {items:[{label,value:number}],currency,period,source}
design-review: Screens, usability findings and accessibility checks. Data: {surface,findings:[{area,severity:neutral|info|success|warning|danger,detail}],accessibility:[{label,status,detail}]}
media-shotlist: Timed shots with visuals, audio and required assets. Data: {shots:[{name,start:number,duration:number,visual,audio,asset}]}
decision-log: Decisions with rationale, owners and review dates. Data: {decisions:[{decision,rationale,owner,reviewAt}]}
source-verification: Claims, source links, verification states and gaps. Data: {claims:[{claim,url,status,gap}]}
goal-tracker: Goal targets, progress, blockers and next steps. Data: {goal,current:number,target:number,blockers:[string],nextSteps:[string]}
knowledge-map: Named concepts and evidence-linked relationships. Data: {concepts:[{name,description}],relationships:[{from,to,relation}]}
file-browser: Files with paths, types, sizes and available next steps. Data: {files:[{path,type,size,note}]}
trend-comparison: A two-series line or area chart from aligned observations. Data: {observations:[{label,actual:number,baseline:number}],area:boolean,source}
calendar-agenda: Scheduled events with time, location and owner. Data: {events:[{title,start,end,location,owner}],timezone,source}
time-zones: Clock times for one moment across places ("what time is it in…", "when is 9 AM Chicago in Tokyo"). Use only with times from the lookup tool (kind time) or current_time, never worked out by hand. Data: {reference,zones:[{place,zone,time,date,offset}],credit}
conversion: One unit or currency conversion with its rate. Use for "how much is 100 USD in EUR" or "5 miles in km" with numbers from the lookup tool (kind convert); currency shows the rate date. Data: {kind:unit|currency,from:{value:number,unit},to:{value:number,unit},rate?:number,asOf?,also?:[{value:number,unit}],credit,creditUrl?}
link-preview: Title, site, summary and image of one HTTPS page from the lookup tool (kind link). Use when the user shares or asks about a single link; not for search results. Data: {url,title,siteName?,description?,image?,imageAlt?,published?,credit}
places: Up to 10 places for a search ("coffee near the station", "where is…") from the lookup tool (kind places), with address, distance and a map link. "Here" needs the shared location; otherwise ask for a place. Data: {query,near?,places:[{name,address,category?,distance?,lat?:number,lon?:number,url?}],credit,creditUrl?}
writing: A draft the user will send or keep: an email, text message, social post, document or note, with up to 2 labelled alternatives. No send action exists; the user sends it. Data: {variant:email|text_message|social_post|document|note,to?,subject?,body,alternatives?:[{label,body}]}
