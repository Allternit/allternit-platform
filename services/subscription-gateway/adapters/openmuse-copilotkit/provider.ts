import { HttpAgent } from '@ag-ui/client';
import { randomUUID } from 'node:crypto';
import { BaseAaiProvider, fail, ok, type AaiResult, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { PairedLocalTransport, isObject } from '../_shared/paired-local.js';

export const CAPABILITIES = agentCapabilityManifestSchema.parse({
 vendor:'openmuse-copilotkit', adapterId:'openmuse-copilotkit', lane:'local', guarantee:'best_effort',
 context:{supported:true,resume:false,parallel:false,maxParallel:1,isolation:'isolated'},
 messaging:{send:true,stream:false,steer:false,interrupt:false,cancel:false},
 memory:{read:false,write:false,snapshot:false,opaque:true},tools:{tools:false,mcp:false,plugins:false,connectors:false},
 tasks:{list:false,schedule:false,cancel:false,background:false},approvals:{read:false,respond:false,exact:false},
 computer:{view:false,control:false,takeover:false},artifacts:{read:false,write:false,export:false},
 events:{native:false,polling:true,transcriptDerived:false,replay:true},runtime:{alwaysOn:false,localRequired:true,cloud:false}
});

type Reply = AaiResult<{messageId:string;correlationId:string;reply:string;guarantee:'best_effort'}>;
type Context = {id:string;owner:string;threadId:string;closed:boolean;busy:boolean;messages:{id:string;role:'user'|'assistant'|'system';content:string}[];sends:Map<string,Promise<Reply>>;sendTexts:Map<string,string>;replies:{requestId:string;correlationId:string;reply:string}[]};

/** Native OpenMuse CopilotKit runtime over its pinned AG-UI HTTP/SSE transport. */
export class OpenMuseCopilotkitProvider extends BaseAaiProvider {
 readonly adapterId='openmuse-copilotkit';
 private transport:PairedLocalTransport;
 private contexts=new Map<string,Context>();
 private timeoutMs:number;

 constructor(options:{baseUrl?:string;token?:string;fetch?:typeof fetch;replyTimeoutMs?:number}={}) {
  super();
  this.transport=new PairedLocalTransport('OpenMuse',{...options,baseUrl:options.baseUrl??'http://127.0.0.1:8787'});
  this.timeoutMs=options.replyTimeoutMs??120000;
 }

 private async session():Promise<AaiResult<string>> {
  const key=this.transport.credential();
  if(!key||key.length<24)return fail('AUTH_REQUIRED','Supply the OpenMuse access key (24+ characters) through the sealed account credential.');
  try {
   const response=await (this.transport.options.fetch??fetch)(this.transport.base+'/api/session',{method:'POST',redirect:'error',signal:AbortSignal.timeout(15000),headers:{'content-type':'application/json',accept:'application/json'},body:JSON.stringify({accessKey:key})});
   if(!response.ok){void response.body?.cancel();if(response.status===401||response.status===403)return fail('AUTH_REVOKED','OpenMuse refused the access key.');if(response.status===429)return fail('RATE_LIMITED','OpenMuse is rate limiting sign-in.');return fail('VENDOR_UNAVAILABLE',`OpenMuse session exchange returned HTTP ${response.status}.`);}
   const value:unknown=await response.json();
   if(!isObject(value)||typeof value.token!=='string'||value.mode!=='live')return fail('POLICY_DENIED','OpenMuse must be in live mode to provide its real agent.');
   return ok(value.token);
  } catch {return fail('VENDOR_UNAVAILABLE','OpenMuse is unreachable or did not complete the session exchange.');}
 }

 async list() {
  const session=await this.session();if(!session.ok)return session;
  try {
   const response=await (this.transport.options.fetch??fetch)(this.transport.base+'/api/health',{redirect:'error',signal:AbortSignal.timeout(15000),headers:{authorization:`Bearer ${session.value}`,accept:'application/json'}});
   if(!response.ok){void response.body?.cancel();return fail(response.status===401?'AUTH_REVOKED':'VENDOR_UNAVAILABLE',`OpenMuse health check returned HTTP ${response.status}.`);}
   const value:unknown=await response.json();
   if(!isObject(value)||value.mode!=='live'||value.agentConfigured!==true)return fail('POLICY_DENIED','OpenMuse has no configured live agent. Configure a model/provider key or AGENT_BACKEND=agui.');
   return ok([{agentId:'openmuse-copilotkit:default',displayName:'OpenMuse',vendor:'openmuse-copilotkit',state:'ready'}]);
  } catch {return fail('VENDOR_UNAVAILABLE','OpenMuse is unreachable.');}
 }
 async get(agentId:string) {const roster=await this.list();if(!roster.ok)return roster;const a=roster.value.find(x=>x.agentId===agentId);return a?ok({...a,remoteIds:{agentId:'default'},capabilities:CAPABILITIES}):fail('CONTEXT_NOT_FOUND','OpenMuse exposes only its configured default agent.');}
 async capabilities(agentId:string) {const a=await this.get(agentId);return a.ok?ok(CAPABILITIES):a;}
 async identity(agentId:string) {const a=await this.get(agentId);return a.ok?ok({agentId,displayName:a.value.displayName,vendor:'openmuse-copilotkit'}):a;}
 private live(id:string):AaiResult<Context> {const c=this.contexts.get(id);return c&&!c.closed&&c.owner===this.transport.owner()?ok(c):fail('CONTEXT_NOT_FOUND','This OpenMuse context is not owned by the current account.');}
 async contextOpen(input:OpenContextInput) {
  const agent=await this.get(input.agentId);if(!agent.ok)return agent;
  if(input.adoptContextId)return fail('UNSUPPORTED','OpenMuse contexts cannot resume after they are detached or the gateway restarts.',{retryable:false});
  const id=randomUUID(),threadId=randomUUID();
  this.contexts.set(id,{id,owner:this.transport.owner(),threadId,closed:false,busy:false,messages:[],sends:new Map(),sendTexts:new Map(),replies:[]});
  return ok({contextId:id,isolation:'isolated' as const,guarantee:'best_effort' as const,resumed:false});
 }
 async contextMessage(input:MessageInput) {
  const c=this.live(input.contextId);if(!c.ok)return c;
  if(!input.text.trim()||!input.correlationId)return fail('LANE_BLOCKED','A nonempty message and stable send ID are required.');
  const prior=c.value.sends.get(input.correlationId);
  if(prior)return c.value.sendTexts.get(input.correlationId)===input.text?prior:fail('POLICY_DENIED','This send ID already belongs to a different message.');
  if(c.value.busy)return fail('CONTEXT_BUSY','This OpenMuse conversation already has a pending Allternit request.');
  c.value.busy=true;c.value.sendTexts.set(input.correlationId,input.text);
  const promise=this.send(c.value,input).finally(()=>{c.value.busy=false;});c.value.sends.set(input.correlationId,promise);return promise;
 }
 private async send(c:Context,input:MessageInput):Promise<Reply> {
  const session=await this.session();if(!session.ok)return session;
  const allowed=`${this.transport.base}/api/copilotkit/agent/default/run`,fetchImpl=this.transport.options.fetch??fetch,runId=randomUUID();
  let transportFailure:AaiResult<never>|undefined;
  const agent=new HttpAgent({agentId:'default',threadId:c.threadId,url:allowed,headers:{authorization:`Bearer ${session.value}`},fetch:async(url,init)=>{
   if(new URL(url).href!==allowed||init.method!=='POST')throw Error('Unapproved OpenMuse runtime request');
   let body:unknown;try{body=JSON.parse(String(init.body));}catch{throw Error('Invalid AG-UI request body');}
   if(!isObject(body)||body.threadId!==c.threadId||body.runId!==runId)throw Error('OpenMuse run identity mismatch');
   const response=await fetchImpl(url,{...init,redirect:'error',signal:AbortSignal.any([init.signal??new AbortController().signal,AbortSignal.timeout(this.timeoutMs)])});
   if(!response.ok)transportFailure=response.status===401||response.status===403?fail('AUTH_REVOKED','OpenMuse rejected its session credential.'):
     response.status===429?fail('RATE_LIMITED','OpenMuse is rate limiting this run.'):
     fail('VENDOR_UNAVAILABLE',`OpenMuse AG-UI run returned HTTP ${response.status}.`,{retryable:false});
   return response;
  }});
  const userMessage={id:randomUUID(),role:'user' as const,content:input.text};
  agent.setMessages([...c.messages,userMessage]);
  let runError=false,started=false,finished=false,runIdentityMismatch=false;
  const identityMatches=(event:{threadId?:string;runId?:string})=>event.threadId===c.threadId&&event.runId===runId;
  const subscription=agent.subscribe({
   onRunStartedEvent:({event})=>{started=true;if(!identityMatches(event))runIdentityMismatch=true;},
   onRunFinishedEvent:({event,outcome})=>{finished=outcome==='success';if(!identityMatches(event))runIdentityMismatch=true;},
   onRunErrorEvent:()=>{runError=true;}
  });
  try {
   const result=await agent.runAgent({runId,tools:[],context:[],forwardedProps:{}});
   const additions=result.newMessages??[];
   if(runError)return fail('VENDOR_UNAVAILABLE','OpenMuse reported a failed AG-UI run.',{retryable:false});
   if(runIdentityMismatch)return fail('ADAPTER_DRIFT','OpenMuse returned run events for a different thread or run.',{retryable:false});
   if(!started||!finished)return transportFailure??fail('ADAPTER_DRIFT','OpenMuse did not confirm completion for the submitted thread and run.',{retryable:false});
   const replies=additions.filter(m=>m.role==='assistant'&&typeof m.content==='string'&&m.content.trim());
   const reply=replies.map(m=>m.content as string).join('\n\n');
   if(!reply.trim())return transportFailure??fail('ADAPTER_DRIFT','OpenMuse finished without an assistant reply attributable to this run.',{retryable:false});
   c.messages=[...c.messages,userMessage,...replies.map(m=>({id:m.id,role:'assistant' as const,content:m.content as string}))];
   c.replies.push({requestId:userMessage.id,correlationId:input.correlationId,reply});
   return ok({messageId:userMessage.id,correlationId:input.correlationId,reply,guarantee:'best_effort'});
  } catch {return transportFailure??fail('VENDOR_UNAVAILABLE','OpenMuse did not confirm this run. Check the vendor conversation before sending again.',{retryable:false});}
  finally {subscription.unsubscribe();await agent.detachActiveRun().catch(()=>undefined);}
 }
 async events(input:EventsInput):Promise<AaiResult<EventsResult>> {
  const c=this.live(input.contextId);if(!c.ok)return c;const after=Number(input.cursor??0);
  if(!Number.isSafeInteger(after)||after<0||after>c.value.replies.length)return fail('CONTEXT_NOT_FOUND','Invalid OpenMuse event cursor.');
  const rows=c.value.replies.slice(after,after+Math.max(1,Math.min(input.limit??100,500)));
  return ok({events:rows.map((r,i)=>({cursor:String(after+i+1),event:{type:'agent.message.completed' as const,botId:`openmuse-copilotkit:default`,threadId:c.value.id,generationId:'1',source:'vendor' as const,vendor:'openmuse-copilotkit',adapter:'openmuse-copilotkit',lane:'local' as const,remoteContextId:c.value.threadId,remoteEventId:r.requestId,correlationId:r.correlationId,causationId:r.requestId,guarantee:'best_effort' as const,at:new Date().toISOString(),payload:{reply:r.reply,messageId:r.requestId}}})),nextCursor:String(after+rows.length)});
 }
 async contextClose(input:{contextId:string}) {const c=this.live(input.contextId);if(!c.ok)return c;if(c.value.busy)return fail('CONTEXT_BUSY','Wait for the current OpenMuse turn before detaching.');c.value.closed=true;return ok({closed:true});}
 async health() {const result=await this.list();return ok({status:result.ok?'healthy' as const:'down' as const,lane:'local',detail:result.ok?'Authenticated OpenMuse AG-UI runtime':result.error.humanMessage});}
}
