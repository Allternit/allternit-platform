import { randomUUID } from 'node:crypto';
import { BaseAaiProvider, fail, ok, type AaiResult, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { PairedLocalTransport, isId, isObject } from '../_shared/paired-local.js';
export const CAPABILITIES = agentCapabilityManifestSchema.parse({
 vendor:'hivekeep', adapterId:'hivekeep', lane:'local', guarantee:'best_effort',
 context:{supported:true,resume:true,parallel:false,maxParallel:1,isolation:'isolated'},
 messaging:{send:true,stream:false,steer:false,interrupt:false,cancel:false},
 memory:{read:false,write:false,snapshot:false,opaque:true},tools:{tools:false,mcp:false,plugins:false,connectors:false},
 tasks:{list:false,schedule:false,cancel:false,background:false},approvals:{read:false,respond:false,exact:false},
 computer:{view:false,control:false,takeover:false},artifacts:{read:false,write:false,export:false},
 events:{native:false,polling:true,transcriptDerived:false,replay:true},runtime:{alwaysOn:false,localRequired:true,cloud:false}
});
type Reply = AaiResult<{messageId:string;correlationId:string;reply:string;guarantee:'best_effort'}>;
type Context = {id:string;owner:string;bot:string;remote:string;closed:boolean;busy:boolean;sends:Map<string,Promise<Reply>>;replies:{requestId:string;correlationId:string;reply:string}[]};
export class HivekeepProvider extends BaseAaiProvider {
 readonly adapterId='hivekeep'; private transport:PairedLocalTransport; private contexts=new Map<string,Context>();
 private pollMs:number; private timeoutMs:number;
 constructor(options:{baseUrl?:string;token?:string;fetch?:typeof fetch;pollMs?:number;replyTimeoutMs?:number}={}) { super();this.transport=new PairedLocalTransport('Hivekeep',{...options,baseUrl:options.baseUrl??'http://127.0.0.1:3000'});this.pollMs=options.pollMs??500;this.timeoutMs=options.replyTimeoutMs??120000; }
 async list() { const result=await this.transport.json('/api/v1/agents');if(!result.ok)return result;
 if(!Array.isArray(result.value.agents)||!result.value.agents.every(a=>isObject(a)&&isId(a.id)&&typeof a.name==='string'))return fail('ADAPTER_DRIFT','Hivekeep returned an invalid agent roster.');
 return ok(result.value.agents.map(a=>({agentId:`hivekeep:${a.id}`,displayName:a.name as string,vendor:'hivekeep',state:'unknown'}))); }
 async get(agentId:string) {const roster=await this.list();if(!roster.ok)return roster;const agent=roster.value.find(a=>a.agentId===agentId);return agent?ok({...agent,remoteIds:{botId:agentId.slice(9)},capabilities:CAPABILITIES}):fail('CONTEXT_NOT_FOUND','This Hivekeep agent is not visible to the connected API client.');}
 async capabilities(agentId:string) {const agent=await this.get(agentId);return agent.ok?ok(CAPABILITIES):agent;}
 async identity(agentId:string) {const agent=await this.get(agentId);return agent.ok?ok({agentId,displayName:agent.value.displayName,vendor:'hivekeep'}):agent;}
 private live(id:string):AaiResult<Context> {const c=this.contexts.get(id);return c&&!c.closed&&c.owner===this.transport.owner()?ok(c):fail('CONTEXT_NOT_FOUND','This Hivekeep context is not owned by the current account.');}
 async contextOpen(input:OpenContextInput) {const agent=await this.get(input.agentId);if(!agent.ok)return agent;
 if(input.adoptContextId){const c=this.live(input.adoptContextId);return c.ok&&c.value.bot===agent.value.remoteIds.botId?ok({contextId:c.value.id,isolation:'isolated' as const,guarantee:'best_effort' as const,resumed:true}):fail('CONTEXT_NOT_FOUND','Cannot adopt another account or agent conversation.');}
 const result=await this.transport.json(`/api/v1/agents/${agent.value.remoteIds.botId}/conversations`,{title:'Allternit'});if(!result.ok)return result;
 if(!isId(result.value.conversationId))return fail('ADAPTER_DRIFT','Hivekeep did not confirm an isolated conversation.');
 const id=randomUUID();this.contexts.set(id,{id,owner:this.transport.owner(),bot:agent.value.remoteIds.botId,remote:result.value.conversationId,closed:false,busy:false,sends:new Map(),replies:[]});
 return ok({contextId:id,isolation:'isolated' as const,guarantee:'best_effort' as const,resumed:false}); }
 async contextMessage(input:MessageInput) {const c=this.live(input.contextId);if(!c.ok)return c;if(!isId(input.correlationId)||!input.text.trim())return fail('LANE_BLOCKED','A nonempty message and stable send ID are required.');
 const prior=c.value.sends.get(input.correlationId);if(prior)return prior;if(c.value.busy)return fail('CONTEXT_BUSY','This Hivekeep conversation has a pending Allternit request.');
 c.value.busy=true;const promise=this.send(c.value,input).finally(()=>{c.value.busy=false;});c.value.sends.set(input.correlationId,promise);return promise; }
 private async send(c:Context,input:MessageInput):Promise<Reply> {let result=await this.transport.json(`/api/v1/agents/${c.bot}/messages`,{content:input.text,conversationId:c.remote,mode:'async'});if(!result.ok)return result;
 const requestId=result.value.requestId;if(!isId(requestId)||result.value.conversationId!==c.remote)return fail('ADAPTER_DRIFT','Hivekeep did not confirm the matching isolated request.');
 const deadline=Date.now()+this.timeoutMs;
 while(!c.closed) {const row=result.value;if(row.requestId!==requestId||row.conversationId!==c.remote)return fail('ADAPTER_DRIFT','Hivekeep returned a foreign request.');
 if(row.status==='done'&&typeof row.reply==='string'){c.replies.push({requestId,correlationId:input.correlationId,reply:row.reply});return ok({messageId:requestId,correlationId:input.correlationId,reply:row.reply,guarantee:'best_effort'});}
 if(row.status==='error')return fail('VENDOR_UNAVAILABLE','Hivekeep reported that this request failed. Review its conversation.',{retryable:false});
 if(!['pending'].includes(String(row.status)))return fail('ADAPTER_DRIFT','Hivekeep returned an unknown request state.');
 if(Date.now()>=deadline)break;await new Promise(resolve=>setTimeout(resolve,this.pollMs));result=await this.transport.json(`/api/v1/requests/${requestId}`);if(!result.ok)return result;
 }return fail('VENDOR_UNAVAILABLE','Hivekeep accepted the request but has not confirmed completion. Check the vendor conversation before sending again.',{retryable:false}); }
 async events(input:EventsInput):Promise<AaiResult<EventsResult>> {const c=this.live(input.contextId);if(!c.ok)return c;const after=Number(input.cursor??0);if(!Number.isSafeInteger(after)||after<0||after>c.value.replies.length)return fail('CONTEXT_NOT_FOUND','Invalid Hivekeep event cursor.');
 const rows=c.value.replies.slice(after,after+Math.max(1,Math.min(input.limit??100,500)));return ok({events:rows.map((r,i)=>({cursor:String(after+i+1),event:{type:'agent.message.completed' as const,botId:`hivekeep:${c.value.bot}`,threadId:c.value.id,generationId:'1',source:'vendor' as const,vendor:'hivekeep',adapter:'hivekeep',lane:'local' as const,remoteContextId:c.value.remote,remoteEventId:r.requestId,correlationId:r.correlationId,causationId:r.requestId,guarantee:'best_effort' as const,at:new Date().toISOString(),payload:{reply:r.reply,messageId:r.requestId}}})),nextCursor:String(after+rows.length)}); }
 async contextClose(input:{contextId:string}) {const c=this.live(input.contextId);if(!c.ok)return c;if(c.value.busy)return fail('CONTEXT_BUSY','Wait for the outstanding Hivekeep request before closing its conversation.');const result=await this.transport.json(`/api/v1/conversations/${c.value.remote}/close`,{});if(!result.ok)return result;if(result.value.ok!==true)return fail('ADAPTER_DRIFT','Hivekeep did not confirm conversation closure.');c.value.closed=true;return ok({closed:true});}
 async health(){const result=await this.list();return ok({status:result.ok?'healthy' as const:'down' as const,lane:'local',detail:result.ok?'Authenticated Hivekeep agent discovery':result.error.humanMessage});}
}
