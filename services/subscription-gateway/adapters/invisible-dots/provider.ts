import { randomUUID } from 'node:crypto';
import { BaseAaiProvider, fail, ok, type AaiResult, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { PairedLocalTransport, isId, isObject } from '../_shared/paired-local.js';
export const CAPABILITIES = agentCapabilityManifestSchema.parse({
 vendor:'invisible-dots', adapterId:'invisible-dots', lane:'local', guarantee:'best_effort',
 context:{supported:true,resume:true,parallel:false,maxParallel:1,isolation:'shared'},
 messaging:{send:true,stream:false,steer:false,interrupt:false,cancel:false},
 memory:{read:false,write:false,snapshot:false,opaque:true},tools:{tools:false,mcp:false,plugins:false,connectors:false},
 tasks:{list:false,schedule:false,cancel:false,background:false},approvals:{read:false,respond:false,exact:false},
 computer:{view:false,control:false,takeover:false},artifacts:{read:false,write:false,export:false},
 events:{native:false,polling:true,transcriptDerived:false,replay:true},runtime:{alwaysOn:false,localRequired:true,cloud:false}
});
type Reply = AaiResult<{messageId:string;correlationId:string;reply:string;guarantee:'best_effort'}>;
type Context = {id:string;owner:string;bot:string;remote:string;closed:boolean;busy:boolean;sends:Map<string,Promise<Reply>>;replies:{requestId:string;correlationId:string;reply:string}[]};
export class InvisibleDotsProvider extends BaseAaiProvider {
 readonly adapterId='invisible-dots'; private transport:PairedLocalTransport; private contexts=new Map<string,Context>(); private sendTexts=new Map<string,string>(); private running=new Set<string>();
 private pollMs:number; private timeoutMs:number;
 constructor(options:{baseUrl?:string;token?:string;fetch?:typeof fetch;pollMs?:number;replyTimeoutMs?:number}={}) { super();this.transport=new PairedLocalTransport('Invisible Dots',{...options,baseUrl:options.baseUrl??'http://127.0.0.1:8787'});this.pollMs=options.pollMs??500;this.timeoutMs=options.replyTimeoutMs??120000; }
 async list() { const result=await this.transport.json('/api/dots');if(!result.ok)return result;
 if(!Array.isArray(result.value.dots)||!result.value.dots.every(a=>isObject(a)&&isId(a.id)&&typeof a.name==='string'))return fail('ADAPTER_DRIFT','Invisible Dots returned an invalid agent roster.');
 return ok(result.value.dots.map(a=>({agentId:`invisible-dots:${a.id}`,displayName:a.name as string,vendor:'invisible-dots',state:typeof a.status==='string'?a.status:'unknown'}))); }
 async get(agentId:string) {const roster=await this.list();if(!roster.ok)return roster;const agent=roster.value.find(a=>a.agentId===agentId);return agent?ok({...agent,remoteIds:{botId:agentId.slice(15)},capabilities:CAPABILITIES}):fail('CONTEXT_NOT_FOUND','This Invisible Dots agent is not visible to the connected API client.');}
 async capabilities(agentId:string) {const agent=await this.get(agentId);return agent.ok?ok(CAPABILITIES):agent;}
 async identity(agentId:string) {const agent=await this.get(agentId);return agent.ok?ok({agentId,displayName:agent.value.displayName,vendor:'invisible-dots'}):agent;}
 private live(id:string):AaiResult<Context> {const c=this.contexts.get(id);return c&&!c.closed&&c.owner===this.transport.owner()?ok(c):fail('CONTEXT_NOT_FOUND','This Invisible Dots context is not owned by the current account.');}
 async contextOpen(input:OpenContextInput) {const agent=await this.get(input.agentId);if(!agent.ok)return agent;
 if(input.adoptContextId){const c=this.live(input.adoptContextId);return c.ok&&c.value.bot===agent.value.remoteIds.botId?ok({contextId:c.value.id,isolation:'shared' as const,guarantee:'best_effort' as const,resumed:true}):fail('CONTEXT_NOT_FOUND','Cannot adopt another account or agent conversation.');}
 const id=randomUUID();this.contexts.set(id,{id,owner:this.transport.owner(),bot:agent.value.remoteIds.botId,remote:agent.value.remoteIds.botId,closed:false,busy:false,sends:new Map(),replies:[]});
 return ok({contextId:id,isolation:'shared' as const,guarantee:'best_effort' as const,resumed:false}); }
 async contextMessage(input:MessageInput) {const c=this.live(input.contextId);if(!c.ok)return c;if(!isId(input.correlationId)||!input.text.trim())return fail('LANE_BLOCKED','A nonempty message and stable send ID are required.');
 const sendKey=`${c.value.id}:${input.correlationId}`;const prior=c.value.sends.get(input.correlationId);if(prior)return this.sendTexts.get(sendKey)===input.text?prior:fail('POLICY_DENIED','This send ID already belongs to a different message.');const key=`${c.value.owner}:${c.value.bot}`;if(this.running.has(key))return fail('CONTEXT_BUSY','This Invisible Dots conversation has a pending Allternit request.');
 this.running.add(key);c.value.busy=true;const promise=this.send(c.value,input).finally(()=>{c.value.busy=false;this.running.delete(key);});this.sendTexts.set(sendKey,input.text);c.value.sends.set(input.correlationId,promise);return promise; }
 private async send(c:Context,input:MessageInput):Promise<Reply> {
 const receipt=await this.transport.json(`/api/dots/${c.bot}/messages`,{text:input.text});if(!receipt.ok)return receipt;
 const requestId=receipt.value.message_id;
 if(!isId(requestId)||!Number.isSafeInteger(receipt.value.event_id)||!['queued','delivered'].includes(String(receipt.value.delivery)))return fail('ADAPTER_DRIFT','Invisible Dots did not confirm its message receipt.');
 const deadline=Date.now()+this.timeoutMs;
 while(!c.closed&&Date.now()<deadline){
 const result=await this.transport.json(`/api/dots/${c.bot}/messages?limit=100&order=desc`);if(!result.ok)return result;
 if(!Array.isArray(result.value.messages)||!result.value.messages.every(m=>isObject(m)&&Number.isSafeInteger(m.event_id)&&['user','assistant'].includes(String(m.role))&&typeof m.text==='string'&&(m.in_reply_to===null||typeof m.in_reply_to==='string')))return fail('ADAPTER_DRIFT','Invisible Dots returned an invalid conversation.');
 const replies=result.value.messages.filter(m=>m.role==='assistant'&&m.in_reply_to===requestId).sort((a,b)=>Number(a.event_id)-Number(b.event_id));
 if(replies.length){const reply=replies.map(m=>m.text).join('\n\n');c.replies.push({requestId,correlationId:input.correlationId,reply});return ok({messageId:requestId,correlationId:input.correlationId,reply,guarantee:'best_effort'});}
 await new Promise(resolve=>setTimeout(resolve,this.pollMs));
 }return fail('VENDOR_UNAVAILABLE','Invisible Dots accepted the message but has not returned its matched reply. Check the vendor conversation before sending again.',{retryable:false}); }
 async events(input:EventsInput):Promise<AaiResult<EventsResult>> {const c=this.live(input.contextId);if(!c.ok)return c;const after=Number(input.cursor??0);if(!Number.isSafeInteger(after)||after<0||after>c.value.replies.length)return fail('CONTEXT_NOT_FOUND','Invalid Invisible Dots event cursor.');
 const rows=c.value.replies.slice(after,after+Math.max(1,Math.min(input.limit??100,500)));return ok({events:rows.map((r,i)=>({cursor:String(after+i+1),event:{type:'agent.message.completed' as const,botId:`invisible-dots:${c.value.bot}`,threadId:c.value.id,generationId:'1',source:'vendor' as const,vendor:'invisible-dots',adapter:'invisible-dots',lane:'local' as const,remoteContextId:c.value.remote,remoteEventId:r.requestId,correlationId:r.correlationId,causationId:r.requestId,guarantee:'best_effort' as const,at:new Date().toISOString(),payload:{reply:r.reply,messageId:r.requestId}}})),nextCursor:String(after+rows.length)}); }
 async contextClose(input:{contextId:string}){const c=this.live(input.contextId);if(!c.ok)return c;c.value.closed=true;return ok({closed:true});}
 async health(){const result=await this.list();return ok({status:result.ok?'healthy' as const:'down' as const,lane:'local',detail:result.ok?'Authenticated Invisible Dots agent discovery':result.error.humanMessage});}
}
