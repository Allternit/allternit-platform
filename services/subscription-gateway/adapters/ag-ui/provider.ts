import { randomUUID } from 'node:crypto';
import { AsyncLocalStorage } from 'node:async_hooks';
import { BaseAaiProvider, fail, ok, type AaiResult, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { runAgUi } from '../_shared/ag-ui.js';
import { endpointFetch, parseCredential, type Endpoint, type Resolve } from './endpoint.js';

export const CAPABILITIES = agentCapabilityManifestSchema.parse({
 vendor:'ag-ui', adapterId:'ag-ui', lane:'local', guarantee:'best_effort',
 context:{supported:true,resume:false,parallel:false,maxParallel:1,isolation:'isolated'},
 messaging:{send:true,stream:false,steer:false,interrupt:false,cancel:false},
 memory:{read:false,write:false,snapshot:false,opaque:true},tools:{tools:false,mcp:false,plugins:false,connectors:false},
 tasks:{list:false,schedule:false,cancel:false,background:false},approvals:{read:false,respond:false,exact:false},
 computer:{view:false,control:false,takeover:false},artifacts:{read:false,write:false,export:false},
 events:{native:false,polling:true,transcriptDerived:false,replay:true},runtime:{alwaysOn:false,localRequired:false,cloud:true}
});

type Scope = { binding?: { id?: string; accountBindingId?: string }; credential?: { apiKey: string } };
type Reply = AaiResult<{messageId:string;correlationId:string;reply:string;guarantee:'best_effort'}>;
type Context = {id:string;owner:string;agentId:string;threadId:string;closed:boolean;busy:boolean;messages:{id:string;role:'user'|'assistant'|'system';content:string}[];sends:Map<string,Promise<Reply>>;sendTexts:Map<string,string>;replies:{requestId:string;correlationId:string;reply:string}[]};
const scope = () => (globalThis as unknown as Record<symbol, AsyncLocalStorage<Scope> | undefined>)[Symbol.for('allternit.aai.callScope')]?.getStore();
const agentIdOf = (e:Endpoint) => `ag-ui:${e.url.host}`;

/** Any AG-UI agent the user names by URL (sealed credential JSON {"url","token"?,"headers"?}); one agent per connection. */
export class AgUiProvider extends BaseAaiProvider {
 readonly adapterId='ag-ui';
 private contexts=new Map<string,Context>();
 private timeoutMs:number;

 constructor(private options:{credential?:string;fetch?:typeof fetch;resolve?:Resolve;replyTimeoutMs?:number}={}) {super();this.timeoutMs=options.replyTimeoutMs??120000;}

 private owner() {const s=scope();return s?.binding?.accountBindingId??s?.binding?.id??'local';}
 private endpoint() {return parseCredential(scope()?.credential?.apiKey??this.options.credential);}
 private headers(e:Endpoint) {return {...e.headers,...(e.token?{authorization:`Bearer ${e.token}`}:{})};}

 /** Validates the URL and address, then checks the agent answers and accepts the credential (a GET; most AG-UI routes answer 405). */
 async list() {
  const e=this.endpoint();if(!e.ok)return e;
  const f=await endpointFetch(e.value,this.options.resolve,this.options.fetch);if(!f.ok)return f;
  try {
   const response=await f.value(e.value.url.href,{method:'GET',redirect:'error',signal:AbortSignal.timeout(15000),headers:{...this.headers(e.value),accept:'text/event-stream'}});
   void response.body?.cancel().catch(()=>undefined);
   if(response.status===401||response.status===403)return fail('AUTH_REVOKED','The AG-UI agent refused this token.');
   if(response.status===429)return fail('RATE_LIMITED','The AG-UI agent is rate limiting this connection.');
   if(response.status>=300&&response.status<400)return fail('POLICY_DENIED','The AG-UI agent URL redirects; enter the final run URL instead.');
   if(response.status>=500)return fail('VENDOR_UNAVAILABLE',`The AG-UI agent returned HTTP ${response.status}.`);
  } catch {return fail('VENDOR_UNAVAILABLE',`The AG-UI agent at ${e.value.url.host} is unreachable.`);}
  return ok([{agentId:agentIdOf(e.value),displayName:e.value.url.hostname,vendor:'ag-ui',state:'ready' as const}]);
 }
 async get(agentId:string) {const roster=await this.list();if(!roster.ok)return roster;const a=roster.value.find(x=>x.agentId===agentId);return a?ok({...a,remoteIds:{agentId:a.agentId.slice(6)},capabilities:CAPABILITIES}):fail('CONTEXT_NOT_FOUND','This connection exposes only the AG-UI agent at its configured URL.');}
 async capabilities(agentId:string) {const a=await this.get(agentId);return a.ok?ok(CAPABILITIES):a;}
 async identity(agentId:string) {const a=await this.get(agentId);return a.ok?ok({agentId,displayName:a.value.displayName,vendor:'ag-ui'}):a;}
 private live(id:string):AaiResult<Context> {const c=this.contexts.get(id);return c&&!c.closed&&c.owner===this.owner()?ok(c):fail('CONTEXT_NOT_FOUND','This AG-UI context is not owned by the current account.');}
 async contextOpen(input:OpenContextInput) {
  const agent=await this.get(input.agentId);if(!agent.ok)return agent;
  if(input.adoptContextId)return fail('UNSUPPORTED','AG-UI contexts cannot resume after they are detached or the gateway restarts.',{retryable:false});
  const id=randomUUID();
  this.contexts.set(id,{id,owner:this.owner(),agentId:input.agentId,threadId:randomUUID(),closed:false,busy:false,messages:[],sends:new Map(),sendTexts:new Map(),replies:[]});
  return ok({contextId:id,isolation:'isolated' as const,guarantee:'best_effort' as const,resumed:false});
 }
 async contextMessage(input:MessageInput) {
  const c=this.live(input.contextId);if(!c.ok)return c;
  if(!input.text.trim()||!input.correlationId)return fail('LANE_BLOCKED','A nonempty message and stable send ID are required.');
  const prior=c.value.sends.get(input.correlationId);
  if(prior)return c.value.sendTexts.get(input.correlationId)===input.text?prior:fail('POLICY_DENIED','This send ID already belongs to a different message.');
  if(c.value.busy)return fail('CONTEXT_BUSY','This AG-UI conversation already has a pending Allternit request.');
  c.value.busy=true;c.value.sendTexts.set(input.correlationId,input.text);
  const promise=this.send(c.value,input).finally(()=>{c.value.busy=false;});c.value.sends.set(input.correlationId,promise);return promise;
 }
 private async send(c:Context,input:MessageInput):Promise<Reply> {
  const e=this.endpoint();if(!e.ok)return e;
  if(agentIdOf(e.value)!==c.agentId)return fail('CONTEXT_NOT_FOUND','This connection now points at a different AG-UI agent; open a new conversation.');
  const f=await endpointFetch(e.value,this.options.resolve,this.options.fetch);if(!f.ok)return f;
  const userMessage={id:randomUUID(),role:'user' as const,content:input.text};
  const run=await runAgUi({url:e.value.url.href,threadId:c.threadId,runId:randomUUID(),messages:[...c.messages,userMessage],headers:this.headers(e.value),fetch:f.value,timeoutMs:this.timeoutMs});
  if(run.status!==undefined){if(run.status===401||run.status===403)return fail('AUTH_REVOKED','The AG-UI agent refused this token.');
   return run.status===429?fail('RATE_LIMITED','The AG-UI agent is rate limiting this run.'):fail('VENDOR_UNAVAILABLE',`The AG-UI agent returned HTTP ${run.status}; check its conversation before sending again.`,{retryable:false});}
  if(run.error)return fail('VENDOR_UNAVAILABLE',`The AG-UI agent reported a failed run: ${run.error.message.slice(0,200)}`,{retryable:false});
  if(run.identityMismatch)return fail('ADAPTER_DRIFT','The AG-UI agent returned run events for a different thread or run.',{retryable:false});
  if(run.transport==='malformed'||run.transport==='not_sse')return fail('ADAPTER_DRIFT','The AG-UI agent did not answer with a valid AG-UI event stream.',{retryable:false});
  if(run.transport==='too_large')return fail('ADAPTER_DRIFT','The AG-UI agent sent a reply larger than Allternit accepts.',{retryable:false});
  if(run.transport)return fail('VENDOR_UNAVAILABLE','The AG-UI agent did not confirm this run. Check its conversation before sending again.',{retryable:false});
  if(run.interrupted)return fail('UNSUPPORTED','The AG-UI agent paused this run for input Allternit cannot provide yet.',{retryable:false});
  if(!run.started||!run.finished)return fail('ADAPTER_DRIFT','The AG-UI agent did not confirm completion for the submitted thread and run.',{retryable:false});
  const replies=run.messages.filter(m=>m.role==='assistant'&&m.content.trim());
  const reply=replies.map(m=>m.content).join('\n\n');
  if(!reply.trim())return fail('ADAPTER_DRIFT','The AG-UI agent finished without an assistant reply for this run.',{retryable:false});
  c.messages=[...c.messages,userMessage,...replies.map(m=>({id:m.id,role:'assistant' as const,content:m.content}))];
  c.replies.push({requestId:userMessage.id,correlationId:input.correlationId,reply});
  return ok({messageId:userMessage.id,correlationId:input.correlationId,reply,guarantee:'best_effort'});
 }
 async events(input:EventsInput):Promise<AaiResult<EventsResult>> {
  const c=this.live(input.contextId);if(!c.ok)return c;const after=Number(input.cursor??0);
  if(!Number.isSafeInteger(after)||after<0||after>c.value.replies.length)return fail('CONTEXT_NOT_FOUND','Invalid AG-UI event cursor.');
  const rows=c.value.replies.slice(after,after+Math.max(1,Math.min(input.limit??100,500)));
  return ok({events:rows.map((r,i)=>({cursor:String(after+i+1),event:{type:'agent.message.completed' as const,botId:c.value.agentId,threadId:c.value.id,generationId:'1',source:'vendor' as const,vendor:'ag-ui',adapter:'ag-ui',lane:'local' as const,remoteContextId:c.value.threadId,remoteEventId:r.requestId,correlationId:r.correlationId,causationId:r.requestId,guarantee:'best_effort' as const,at:new Date().toISOString(),payload:{reply:r.reply,messageId:r.requestId}}})),nextCursor:String(after+rows.length)});
 }
 async contextClose(input:{contextId:string}) {const c=this.live(input.contextId);if(!c.ok)return c;if(c.value.busy)return fail('CONTEXT_BUSY','Wait for the current AG-UI turn before detaching.');c.value.closed=true;return ok({closed:true});}
 async health() {const result=await this.list();return ok({status:result.ok?'healthy' as const:'down' as const,lane:'local',detail:result.ok?'Reachable AG-UI agent':result.error.humanMessage});}
}
