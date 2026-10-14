import { randomUUID } from 'node:crypto';
import { BaseAaiProvider, fail, ok, type AaiResult, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { PairedLocalTransport, isId, isObject } from '../_shared/paired-local.js';

export const ADAPTER_ID = 'sidemates';
// Sidemates' own default port (OPENBOT_PORT). Only used when the URL is set, so the adapter never reaches an unset origin.
const DEFAULT_LOOPBACK = 'http://127.0.0.1:4311';

export const CAPABILITIES = agentCapabilityManifestSchema.parse({
 vendor:ADAPTER_ID, adapterId:ADAPTER_ID, lane:'local', guarantee:'best_effort',
 context:{supported:true,resume:true,parallel:false,maxParallel:1,isolation:'shared'},
 messaging:{send:true,stream:false,steer:false,interrupt:false,cancel:false},
 memory:{read:false,write:false,snapshot:false,opaque:true},tools:{tools:false,mcp:false,plugins:false,connectors:false},
 tasks:{list:false,schedule:false,cancel:false,background:false},approvals:{read:false,respond:false,exact:false},
 computer:{view:false,control:false,takeover:false},artifacts:{read:false,write:false,export:false},
 events:{native:false,polling:true,transcriptDerived:false,replay:true},runtime:{alwaysOn:false,localRequired:true,cloud:false}
});

type Reply = AaiResult<{messageId:string;correlationId:string;reply:string;guarantee:'best_effort'}>;
type Bot = {botId:string;name:string;threadId:string};
type Context = {id:string;owner:string;bot:string;thread:string;closed:boolean;busy:boolean;sends:Map<string,Promise<Reply>>;replies:{requestId:string;correlationId:string;reply:string}[]};

export class SidematesProvider extends BaseAaiProvider {
 readonly adapterId=ADAPTER_ID;
 private transport:PairedLocalTransport; private configured:boolean;
 private contexts=new Map<string,Context>(); private sendTexts=new Map<string,string>();
 private pollMs:number; private timeoutMs:number;
 constructor(options:{baseUrl?:string;token?:string;fetch?:typeof fetch;pollMs?:number;replyTimeoutMs?:number}={}) {
  super();
  this.configured=Boolean(options.baseUrl);
  this.transport=new PairedLocalTransport('Sidemates',{...options,baseUrl:options.baseUrl||DEFAULT_LOOPBACK});
  this.pollMs=options.pollMs??500; this.timeoutMs=options.replyTimeoutMs??120000;
 }

 // Teammates from GET /api/state (the same roster the app shows). Retired teammates are dropped.
 private async roster():Promise<AaiResult<Bot[]>> {
  if(!this.configured)return fail('AUTH_REQUIRED','Set the Sidemates address on this computer to connect.');
  const state=await this.transport.json('/api/state');if(!state.ok)return state;
  const bots=state.value.bots;
  if(!Array.isArray(bots)||!bots.every(b=>isObject(b)&&isId(b.id)&&typeof b.name==='string'&&isId(b.threadId)))
   return fail('ADAPTER_DRIFT','Sidemates returned an invalid teammate roster.');
  return ok(bots.filter(b=>!b.retiredAt).map(b=>({botId:b.id as string,name:b.name as string,threadId:b.threadId as string})));
 }

 async list() {
  const roster=await this.roster();if(!roster.ok)return roster;
  return ok(roster.value.map(b=>({agentId:`sidemates:${b.botId}`,displayName:b.name,vendor:ADAPTER_ID,state:'unknown'})));
 }

 async get(agentId:string) {
  const roster=await this.roster();if(!roster.ok)return roster;
  const bot=roster.value.find(b=>`sidemates:${b.botId}`===agentId);
  if(!bot)return fail('CONTEXT_NOT_FOUND','This Sidemates teammate is not visible to the connected access key.');
  const agent={agentId,displayName:bot.name,vendor:ADAPTER_ID,state:'unknown'};
  return ok({...agent,remoteIds:{botId:bot.botId,threadId:bot.threadId},capabilities:CAPABILITIES});
 }

 async capabilities(agentId:string) {const agent=await this.get(agentId);return agent.ok?ok(CAPABILITIES):agent;}

 async identity(agentId:string) {const agent=await this.get(agentId);return agent.ok?ok({agentId,displayName:agent.value.displayName,vendor:ADAPTER_ID}):agent;}

 private live(id:string):AaiResult<Context> {const c=this.contexts.get(id);return c&&!c.closed&&c.owner===this.transport.owner()?ok(c):fail('CONTEXT_NOT_FOUND','This Sidemates context is not owned by the current account.');}

 // Sidemates has no per-conversation create for a teammate, so a context is a handle on the teammate's own thread (isolation: shared).
 async contextOpen(input:OpenContextInput) {
  const agent=await this.get(input.agentId);if(!agent.ok)return agent;
  const {botId,threadId}=agent.value.remoteIds;
  if(input.adoptContextId){
   const c=this.live(input.adoptContextId);
   return c.ok&&c.value.bot===botId?ok({contextId:c.value.id,isolation:'shared' as const,guarantee:'best_effort' as const,resumed:true}):fail('CONTEXT_NOT_FOUND','Cannot adopt another account or teammate conversation.');
  }
  const id=randomUUID();
  this.contexts.set(id,{id,owner:this.transport.owner(),bot:botId,thread:threadId,closed:false,busy:false,sends:new Map(),replies:[]});
  return ok({contextId:id,isolation:'shared' as const,guarantee:'best_effort' as const,resumed:false});
 }

 async contextMessage(input:MessageInput) {
  const c=this.live(input.contextId);if(!c.ok)return c;
  if(!isId(input.correlationId)||!input.text.trim())return fail('LANE_BLOCKED','A nonempty message and stable send ID are required.');
  const sendKey=`${c.value.id}:${input.correlationId}`;
  const prior=c.value.sends.get(input.correlationId);
  if(prior)return this.sendTexts.get(sendKey)===input.text?prior:fail('POLICY_DENIED','This send ID already belongs to a different message.');
  if(c.value.busy)return fail('CONTEXT_BUSY','This Sidemates conversation has a pending Allternit request.');
  c.value.busy=true;
  const promise=this.send(c.value,input).finally(()=>{c.value.busy=false;});
  this.sendTexts.set(sendKey,input.text);c.value.sends.set(input.correlationId,promise);
  return promise;
 }

 private async send(c:Context,input:MessageInput):Promise<Reply> {
  // requestId = the caller's correlation ID: Sidemates replays the same receipt on a retry with the same payload.
  const sent=await this.transport.json('/api/messages',{threadId:c.thread,body:input.text,requestId:input.correlationId,targetBotIds:[c.bot]});
  if(!sent.ok)return sent;
  const runIds=sent.value.runIds;
  if(!Array.isArray(runIds)||runIds.length!==1||!isId(runIds[0]))
   return fail('ADAPTER_DRIFT','Sidemates did not confirm exactly one run for this message. Check the teammate conversation before sending again.',{retryable:false});
  const runId=runIds[0] as string;
  const deadline=Date.now()+this.timeoutMs;
  for(;;){
   const run=await this.transport.json(`/api/runs/${runId}`);if(!run.ok)return run;
   if(run.value.status==='completed')return this.reply(c,input,runId,run.value);
   if(run.value.status==='awaiting_approval')return fail('POLICY_DENIED','Sidemates is waiting for your approval in the app. Approve it there, then check the conversation.',{retryable:false});
   if(run.value.status==='failed'||run.value.status==='cancelled')return fail('VENDOR_UNAVAILABLE','Sidemates reported that this request did not finish. Review its conversation.',{retryable:false});
   if(Date.now()>=deadline)break;
   await new Promise(resolve=>setTimeout(resolve,this.pollMs));
  }
  return fail('VENDOR_UNAVAILABLE','Sidemates accepted the request but has not confirmed completion. Check the teammate conversation before sending again.',{retryable:false});
 }

 // The reply is the teammate's bot message tagged with this run ID (the thread messages route returns a bare array).
 private async reply(c:Context,input:MessageInput,runId:string,run:Record<string,unknown>):Promise<Reply> {
  const res=await this.transport.response(`/api/threads/${c.thread}/messages?limit=200`);if(!res.ok)return res;
  let rows:unknown;
  try{rows=await res.value.json();}catch{return fail('ADAPTER_DRIFT','Sidemates returned invalid JSON.',{retryable:false});}
  if(!Array.isArray(rows))return fail('ADAPTER_DRIFT','Sidemates returned an invalid conversation.',{retryable:false});
  const bot=rows.filter(isObject).filter(m=>m.runId===runId&&m.senderType==='bot'&&typeof m.body==='string').at(-1);
  const text=bot&&typeof bot.body==='string'?bot.body:(typeof run.summary==='string'?run.summary:null);
  if(text===null)return fail('ADAPTER_DRIFT','Sidemates finished the request without a reply it can attribute to this send.',{retryable:false});
  c.replies.push({requestId:runId,correlationId:input.correlationId,reply:text});
  return ok({messageId:runId,correlationId:input.correlationId,reply:text,guarantee:'best_effort' as const});
 }

 async events(input:EventsInput):Promise<AaiResult<EventsResult>> {
  const c=this.live(input.contextId);if(!c.ok)return c;
  const after=Number(input.cursor??0);
  if(!Number.isSafeInteger(after)||after<0||after>c.value.replies.length)return fail('CONTEXT_NOT_FOUND','Invalid Sidemates event cursor.');
  const rows=c.value.replies.slice(after,after+Math.max(1,Math.min(input.limit??100,500)));
  return ok({events:rows.map((r,i)=>({cursor:String(after+i+1),event:{type:'agent.message.completed' as const,botId:`sidemates:${c.value.bot}`,threadId:c.value.id,generationId:'1',source:'vendor' as const,vendor:ADAPTER_ID,adapter:ADAPTER_ID,lane:'local' as const,remoteContextId:c.value.thread,remoteEventId:r.requestId,correlationId:r.correlationId,causationId:r.requestId,guarantee:'best_effort' as const,at:new Date().toISOString(),payload:{reply:r.reply,messageId:r.requestId}}})),nextCursor:String(after+rows.length)});
 }

 // Sidemates offers no close for a teammate's thread, so closing only ends the Allternit handle; the conversation stays in the app.
 async contextClose(input:{contextId:string}) {const c=this.live(input.contextId);if(!c.ok)return c;if(c.value.busy)return fail('CONTEXT_BUSY','Wait for the outstanding Sidemates request before closing this context.');c.value.closed=true;return ok({closed:true});}

 async health(){const roster=await this.roster();return ok({status:roster.ok?'healthy' as const:'down' as const,lane:'local',detail:roster.ok?'Authenticated Sidemates teammate discovery':roster.error.humanMessage});}
}
