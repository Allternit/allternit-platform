import { randomUUID } from 'node:crypto';
import { BaseAaiProvider, fail, ok, type AaiResult, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { PairedLocalTransport, isId, isObject } from '../_shared/paired-local.js';

export const ADAPTER_ID = 'nanomuse';
// `nanomuse serve` default (nanomuse/server). Only used once a URL is configured.
const DEFAULT_LOOPBACK = 'http://127.0.0.1:8787';
const AGENT_ID = 'nanomuse:agent';

export const CAPABILITIES = agentCapabilityManifestSchema.parse({
 vendor:ADAPTER_ID, adapterId:ADAPTER_ID, lane:'local', guarantee:'best_effort',
 context:{supported:true,resume:true,parallel:false,maxParallel:1,isolation:'isolated'},
 messaging:{send:true,stream:false,steer:false,interrupt:false,cancel:false},
 memory:{read:false,write:false,snapshot:false,opaque:true},tools:{tools:false,mcp:false,plugins:false,connectors:false},
 tasks:{list:false,schedule:false,cancel:false,background:false},approvals:{read:false,respond:false,exact:false},
 computer:{view:false,control:false,takeover:false},artifacts:{read:false,write:false,export:false},
 events:{native:false,polling:true,transcriptDerived:false,replay:true},runtime:{alwaysOn:false,localRequired:true,cloud:false}
});

type Reply = AaiResult<{messageId:string;correlationId:string;reply:string;guarantee:'best_effort'}>;
type Context = {id:string;owner:string;thread:string;closed:boolean;busy:boolean;sends:Map<string,Promise<Reply>>;replies:{requestId:string;correlationId:string;reply:string}[]};

export class NanoMuseProvider extends BaseAaiProvider {
 readonly adapterId=ADAPTER_ID;
 private transport:PairedLocalTransport; private configured:boolean;
 private contexts=new Map<string,Context>(); private sendTexts=new Map<string,string>();
 private pollMs:number; private timeoutMs:number;
 constructor(options:{baseUrl?:string;token?:string;fetch?:typeof fetch;pollMs?:number;replyTimeoutMs?:number}={}) {
  super();
  this.configured=Boolean(options.baseUrl);
  this.transport=new PairedLocalTransport('nanoMuse',{...options,baseUrl:options.baseUrl||DEFAULT_LOOPBACK});
  this.pollMs=options.pollMs??500; this.timeoutMs=options.replyTimeoutMs??120000;
 }

 // One `nanomuse serve` process is one agent. Its name comes from the profile in GET /api/state.
 private async agent():Promise<AaiResult<{displayName:string}>> {
  if(!this.configured)return fail('AUTH_REQUIRED','Set the nanoMuse server address on this computer to connect.');
  const state=await this.transport.json('/api/state');if(!state.ok)return state;
  const profile=state.value.profile;
  if(!isObject(profile)||typeof profile.name!=='string'||!profile.name.trim())return fail('ADAPTER_DRIFT','nanoMuse returned an invalid profile.');
  return ok({displayName:profile.name});
 }

 async list() {
  const agent=await this.agent();if(!agent.ok)return agent;
  return ok([{agentId:AGENT_ID,displayName:agent.value.displayName,vendor:ADAPTER_ID,state:'unknown'}]);
 }

 async get(agentId:string) {
  const agent=await this.agent();if(!agent.ok)return agent;
  if(agentId!==AGENT_ID)return fail('CONTEXT_NOT_FOUND','This nanoMuse agent is not visible to the connected access key.');
  return ok({agentId,displayName:agent.value.displayName,vendor:ADAPTER_ID,state:'unknown',remoteIds:{},capabilities:CAPABILITIES});
 }

 async capabilities(agentId:string) {const agent=await this.get(agentId);return agent.ok?ok(CAPABILITIES):agent;}

 async identity(agentId:string) {const agent=await this.get(agentId);return agent.ok?ok({agentId,displayName:agent.value.displayName,vendor:ADAPTER_ID}):agent;}

 private live(id:string):AaiResult<Context> {const c=this.contexts.get(id);return c&&!c.closed&&c.owner===this.transport.owner()?ok(c):fail('CONTEXT_NOT_FOUND','This nanoMuse context is not owned by the current account.');}

 // Each context is a new conversation thread on the agent (isolated), created with POST /api/threads.
 async contextOpen(input:OpenContextInput) {
  const agent=await this.get(input.agentId);if(!agent.ok)return agent;
  if(input.adoptContextId){
   const c=this.live(input.adoptContextId);
   return c.ok?ok({contextId:c.value.id,isolation:'isolated' as const,guarantee:'best_effort' as const,resumed:true}):fail('CONTEXT_NOT_FOUND','Cannot adopt another account or agent conversation.');
  }
  const created=await this.transport.json('/api/threads',{title:'Allternit'});if(!created.ok)return created;
  if(!isId(created.value.id))return fail('ADAPTER_DRIFT','nanoMuse did not confirm an isolated conversation.',{retryable:false});
  const id=randomUUID();
  this.contexts.set(id,{id,owner:this.transport.owner(),thread:created.value.id as string,closed:false,busy:false,sends:new Map(),replies:[]});
  return ok({contextId:id,isolation:'isolated' as const,guarantee:'best_effort' as const,resumed:false});
 }

 async contextMessage(input:MessageInput) {
  const c=this.live(input.contextId);if(!c.ok)return c;
  if(!isId(input.correlationId)||!input.text.trim())return fail('LANE_BLOCKED','A nonempty message and stable send ID are required.');
  const sendKey=`${c.value.id}:${input.correlationId}`;
  const prior=c.value.sends.get(input.correlationId);
  if(prior)return this.sendTexts.get(sendKey)===input.text?prior:fail('POLICY_DENIED','This send ID already belongs to a different message.');
  if(c.value.busy)return fail('CONTEXT_BUSY','This nanoMuse conversation has a pending Allternit request.');
  c.value.busy=true;
  const promise=this.send(c.value,input).finally(()=>{c.value.busy=false;});
  this.sendTexts.set(sendKey,input.text);c.value.sends.set(input.correlationId,promise);
  return promise;
 }

 // POST /api/threads/:id/send queues the message and returns its timeline event ID. The reply is the first
 // assistant event with final:true after that event in the same thread (events are in arrival order).
 private async send(c:Context,input:MessageInput):Promise<Reply> {
  const sent=await this.transport.json(`/api/threads/${c.thread}/send`,{text:input.text});
  if(!sent.ok)return sent;
  const event=sent.value.event;
  if(!isObject(event)||!isId(event.id))return fail('ADAPTER_DRIFT','nanoMuse did not confirm the queued message.',{retryable:false});
  const userEventId=event.id as string;
  const deadline=Date.now()+this.timeoutMs;
  let sawBusy=false;
  for(;;){
   const timeline=await this.transport.json(`/api/threads/${c.thread}/events?limit=1000`);if(!timeline.ok)return timeline;
   const events=timeline.value.events;
   const thread=isObject(timeline.value.thread)?timeline.value.thread:{};
   if(!Array.isArray(events))return fail('ADAPTER_DRIFT','nanoMuse returned an invalid timeline.',{retryable:false});
   const at=events.findIndex(e=>isObject(e)&&e.id===userEventId);
   if(at<0&&Date.now()>=deadline)break;
   if(at>=0){
    const reply=events.slice(at+1).find(e=>isObject(e)&&e.type==='assistant'&&e.final===true&&typeof e.text==='string');
    if(isObject(reply)){
     const text=reply.text as string;
     c.replies.push({requestId:userEventId,correlationId:input.correlationId,reply:text});
     return ok({messageId:userEventId,correlationId:input.correlationId,reply:text,guarantee:'best_effort' as const});
    }
   }
   if(thread.busy===true)sawBusy=true;
   // The turn ran and has ended without a final reply: a failure notice, not a silent wait.
   if(sawBusy&&thread.busy===false)return fail('VENDOR_UNAVAILABLE','nanoMuse finished this turn without a reply. Review the conversation in the app.',{retryable:false});
   if(Date.now()>=deadline)break;
   await new Promise(resolve=>setTimeout(resolve,this.pollMs));
  }
  return fail('VENDOR_UNAVAILABLE','nanoMuse accepted the message but has not confirmed a reply. Check the conversation before sending again.',{retryable:false});
 }

 async events(input:EventsInput):Promise<AaiResult<EventsResult>> {
  const c=this.live(input.contextId);if(!c.ok)return c;
  const after=Number(input.cursor??0);
  if(!Number.isSafeInteger(after)||after<0||after>c.value.replies.length)return fail('CONTEXT_NOT_FOUND','Invalid nanoMuse event cursor.');
  const rows=c.value.replies.slice(after,after+Math.max(1,Math.min(input.limit??100,500)));
  return ok({events:rows.map((r,i)=>({cursor:String(after+i+1),event:{type:'agent.message.completed' as const,botId:AGENT_ID,threadId:c.value.id,generationId:'1',source:'vendor' as const,vendor:ADAPTER_ID,adapter:ADAPTER_ID,lane:'local' as const,remoteContextId:c.value.thread,remoteEventId:r.requestId,correlationId:r.correlationId,causationId:r.requestId,guarantee:'best_effort' as const,at:new Date().toISOString(),payload:{reply:r.reply,messageId:r.requestId}}})),nextCursor:String(after+rows.length)});
 }

 // Closing ends only the Allternit handle. The thread stays on the computer, where the person can open or delete it.
 async contextClose(input:{contextId:string}) {const c=this.live(input.contextId);if(!c.ok)return c;if(c.value.busy)return fail('CONTEXT_BUSY','Wait for the outstanding nanoMuse request before closing its conversation.');c.value.closed=true;return ok({closed:true});}

 async health(){const agent=await this.agent();return ok({status:agent.ok?'healthy' as const:'down' as const,lane:'local',detail:agent.ok?'Authenticated nanoMuse agent discovery':agent.error.humanMessage});}
}
