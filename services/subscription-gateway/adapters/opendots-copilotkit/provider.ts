import { IntelligenceAgent, type IntelligenceAgentConfig } from '@copilotkit/core';
import { randomUUID } from 'node:crypto';
import { BaseAaiProvider, fail, ok, type AaiResult, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { PairedLocalTransport, isId, isObject } from '../_shared/paired-local.js';
export const CAPABILITIES = agentCapabilityManifestSchema.parse({
 vendor:'opendots-copilotkit', adapterId:'opendots-copilotkit', lane:'local', guarantee:'best_effort',
 context:{supported:true,resume:true,parallel:false,maxParallel:1,isolation:'isolated'},
 messaging:{send:true,stream:false,steer:false,interrupt:false,cancel:false},
 memory:{read:false,write:false,snapshot:false,opaque:true},tools:{tools:false,mcp:false,plugins:false,connectors:false},
 tasks:{list:false,schedule:false,cancel:false,background:false},approvals:{read:false,respond:false,exact:false},
 computer:{view:false,control:false,takeover:false},artifacts:{read:false,write:false,export:false},
 events:{native:false,polling:true,transcriptDerived:false,replay:true},runtime:{alwaysOn:false,localRequired:true,cloud:false}
});
type Client = Pick<IntelligenceAgent,'threadId'|'addMessage'|'subscribe'|'runAgent'|'detachActiveRun'>;
type Factory = (config:IntelligenceAgentConfig)=>Client;
type Reply = AaiResult<{messageId:string;correlationId:string;reply:string;guarantee:'best_effort'}>;
type Context = {id:string;owner:string;bot:string;remote:string;closed:boolean;busy:boolean;sends:Map<string,Promise<Reply>>;replies:{requestId:string;correlationId:string;reply:string}[]};
export class OpenDotsCopilotkitProvider extends BaseAaiProvider {
 readonly adapterId='opendots-copilotkit'; private transport:PairedLocalTransport; private contexts=new Map<string,Context>(); private sendTexts=new Map<string,string>();
 private pollMs:number; private timeoutMs:number; private factory:Factory; private realtimeOrigin:string;
 constructor(options:{baseUrl?:string;token?:string;fetch?:typeof fetch;pollMs?:number;replyTimeoutMs?:number;agentFactory?:Factory;realtimeOrigin?:string}={}) { super();this.transport=new PairedLocalTransport('CopilotKit OpenDots',{...options,baseUrl:options.baseUrl??'http://127.0.0.1:4310'});this.pollMs=options.pollMs??500;this.timeoutMs=options.replyTimeoutMs??120000;this.factory=options.agentFactory??(config=>new IntelligenceAgent(config));this.realtimeOrigin=new URL(options.realtimeOrigin??'wss://realtime.intelligence.copilotkit.ai').origin; }
 async list() {
 const token=this.transport.credential();if(!token)return fail('AUTH_REQUIRED','Supply the configured OpenDots owner token.');
 try{const probe=await (this.transport.options.fetch??fetch)(this.transport.base+'/api/workspace',{redirect:'error',signal:AbortSignal.timeout(15000)});void probe.body?.cancel();if(![401,403].includes(probe.status))return fail('POLICY_DENIED','Configure an owner access token in OpenDots before connecting Allternit.');}catch{return fail('VENDOR_UNAVAILABLE','CopilotKit OpenDots is unreachable.');}
 const result=await this.transport.json('/api/workspace');if(!result.ok)return result;
 if(!Array.isArray(result.value.dots)||!result.value.dots.every(a=>isObject(a)&&isId(a.id)&&typeof a.name==='string'))return fail('ADAPTER_DRIFT','CopilotKit OpenDots returned an invalid agent roster.');
 return ok(result.value.dots.map(a=>({agentId:`opendots-copilotkit:${a.id}`,displayName:a.name as string,vendor:'opendots-copilotkit',state:'unknown'}))); }
 async get(agentId:string) {const roster=await this.list();if(!roster.ok)return roster;const agent=roster.value.find(a=>a.agentId===agentId);return agent?ok({...agent,remoteIds:{botId:agentId.slice('opendots-copilotkit:'.length)},capabilities:CAPABILITIES}):fail('CONTEXT_NOT_FOUND','This CopilotKit OpenDots agent is not visible to the connected API client.');}
 async capabilities(agentId:string) {const agent=await this.get(agentId);return agent.ok?ok(CAPABILITIES):agent;}
 async identity(agentId:string) {const agent=await this.get(agentId);return agent.ok?ok({agentId,displayName:agent.value.displayName,vendor:'opendots-copilotkit'}):agent;}
 private live(id:string):AaiResult<Context> {const c=this.contexts.get(id);return c&&!c.closed&&c.owner===this.transport.owner()?ok(c):fail('CONTEXT_NOT_FOUND','This CopilotKit OpenDots context is not owned by the current account.');}
 async contextOpen(input:OpenContextInput) {const agent=await this.get(input.agentId);if(!agent.ok)return agent;
 if(input.adoptContextId){const c=this.live(input.adoptContextId);return c.ok&&c.value.bot===agent.value.remoteIds.botId?ok({contextId:c.value.id,isolation:'isolated' as const,guarantee:'best_effort' as const,resumed:true}):fail('CONTEXT_NOT_FOUND','Cannot adopt another account or agent conversation.');}
 const result=await this.transport.json('/api/conversations',{dotId:agent.value.remoteIds.botId,title:'Allternit'});if(!result.ok)return result;
 if(!isId(result.value.id)||result.value.dotId!==agent.value.remoteIds.botId)return fail('ADAPTER_DRIFT','CopilotKit OpenDots did not confirm an isolated conversation.');
 const id=randomUUID();this.contexts.set(id,{id,owner:this.transport.owner(),bot:agent.value.remoteIds.botId,remote:result.value.id,closed:false,busy:false,sends:new Map(),replies:[]});
 return ok({contextId:id,isolation:'isolated' as const,guarantee:'best_effort' as const,resumed:false}); }
 async contextMessage(input:MessageInput) {const c=this.live(input.contextId);if(!c.ok)return c;if(!isId(input.correlationId)||!input.text.trim())return fail('LANE_BLOCKED','A nonempty message and stable send ID are required.');
 const sendKey=`${c.value.id}:${input.correlationId}`;const prior=c.value.sends.get(input.correlationId);if(prior)return this.sendTexts.get(sendKey)===input.text?prior:fail('POLICY_DENIED','This send ID already belongs to a different message.');if(c.value.busy)return fail('CONTEXT_BUSY','This CopilotKit OpenDots conversation has a pending Allternit request.');
 c.value.busy=true;const promise=this.send(c.value,input).finally(()=>{c.value.busy=false;});this.sendTexts.set(sendKey,input.text);c.value.sends.set(input.correlationId,promise);return promise; }
 private async send(c:Context,input:MessageInput):Promise<Reply> {
 const info=await this.transport.json('/api/copilotkit/info');if(!info.ok)return info;
 if(info.value.mode!=='intelligence'||!isObject(info.value.intelligence)||typeof info.value.intelligence.wsUrl!=='string'||!isObject(info.value.agents)||!Object.hasOwn(info.value.agents,c.bot))return fail('ADAPTER_DRIFT','OpenDots did not expose the selected Intelligence agent.');
 try{const ws=new URL(info.value.intelligence.wsUrl);if(ws.protocol!=='wss:'||ws.origin!==this.realtimeOrigin) return fail('POLICY_DENIED','OpenDots returned an unapproved realtime endpoint.');}catch{return fail('ADAPTER_DRIFT','OpenDots returned an invalid realtime endpoint.');}
 const token=this.transport.credential();if(!token)return fail('AUTH_REQUIRED','The owner token is unavailable.');
 const messageId=randomUUID();let failure:AaiResult<never>|undefined;let runError=false;let timer:ReturnType<typeof setTimeout>|undefined;
 const sdkFetch=(async(url:Parameters<typeof fetch>[0],init?:Parameters<typeof fetch>[1])=>{
  const target=new URL(typeof url==='string'?url:url instanceof URL?url.href:url.url);if(target.origin!==this.transport.base||!['run','connect'].some(mode=>target.pathname===`/api/copilotkit/agent/${encodeURIComponent(c.bot)}/${mode}`)||init?.method!=='POST')throw Error('Unapproved SDK request');
  const body:unknown=JSON.parse(String(init.body));if(!isObject(body)||body.threadId!==c.remote)throw Error('Foreign SDK thread');
  const result=await this.transport.response(target.pathname,body);if(!result.ok){failure=result;throw Error('Vendor refused SDK request');}
  const receipt:unknown=await result.value.clone().json();
  if(!isObject(receipt)||receipt.threadId!==c.remote||!isObject(receipt.realtime)||typeof receipt.realtime.clientUrl!=='string'){failure=fail('ADAPTER_DRIFT','OpenDots returned foreign realtime credentials.');throw Error('Invalid SDK receipt');}
  const ws=new URL(receipt.realtime.clientUrl);if(ws.protocol!=='wss:'||ws.origin!==this.realtimeOrigin){failure=fail('POLICY_DENIED','OpenDots returned an unapproved realtime endpoint.');throw Error('Invalid SDK endpoint');}
  return result.value;
 }) as typeof fetch;
 let agent:Client;try{agent=this.factory({url:info.value.intelligence.wsUrl,runtimeUrl:this.transport.base+'/api/copilotkit',agentId:c.bot,headers:{Authorization:`Bearer ${token}`},fetch:sdkFetch});}catch{return fail('VENDOR_UNAVAILABLE','The OpenDots SDK could not initialize. Check the gateway installation.');}agent.threadId=c.remote;
 const subscription=agent.subscribe({onRunErrorEvent:()=>{runError=true;}});
 try{agent.addMessage({id:messageId,role:'user',content:input.text});
 const result=await Promise.race([agent.runAgent(),new Promise<never>((_,reject)=>{timer=setTimeout(()=>reject(Error('Wait expired')),this.timeoutMs);})]);
 if(runError)return fail('VENDOR_UNAVAILABLE','OpenDots reported a run error. Review the vendor conversation.',{retryable:false});
 const reply=result.newMessages.filter(m=>m.role==='assistant').at(-1)?.content;
 if(typeof reply!=='string'||!reply.trim())return fail('ADAPTER_DRIFT','OpenDots did not return a current-run assistant reply. Review approvals in the vendor app.');
 c.replies.push({requestId:messageId,correlationId:input.correlationId,reply});return ok({messageId,correlationId:input.correlationId,reply,guarantee:'best_effort'});
 }catch{return failure??fail('VENDOR_UNAVAILABLE','OpenDots has not confirmed this run. Check its conversation before sending again.',{retryable:false});}
 finally{if(timer)clearTimeout(timer);subscription.unsubscribe();await agent.detachActiveRun().catch(()=>undefined);}
 }
 async events(input:EventsInput):Promise<AaiResult<EventsResult>> {const c=this.live(input.contextId);if(!c.ok)return c;const after=Number(input.cursor??0);if(!Number.isSafeInteger(after)||after<0||after>c.value.replies.length)return fail('CONTEXT_NOT_FOUND','Invalid CopilotKit OpenDots event cursor.');
 const rows=c.value.replies.slice(after,after+Math.max(1,Math.min(input.limit??100,500)));return ok({events:rows.map((r,i)=>({cursor:String(after+i+1),event:{type:'agent.message.completed' as const,botId:`opendots-copilotkit:${c.value.bot}`,threadId:c.value.id,generationId:'1',source:'vendor' as const,vendor:'opendots-copilotkit',adapter:'opendots-copilotkit',lane:'local' as const,remoteContextId:c.value.remote,remoteEventId:r.requestId,correlationId:r.correlationId,causationId:r.requestId,guarantee:'best_effort' as const,at:new Date().toISOString(),payload:{reply:r.reply,messageId:r.requestId}}})),nextCursor:String(after+rows.length)}); }
 async contextClose(input:{contextId:string}){const c=this.live(input.contextId);if(!c.ok)return c;if(c.value.busy)return fail('CONTEXT_BUSY','Wait for the current OpenDots turn before detaching.');c.value.closed=true;return ok({closed:true});}
 async health(){const result=await this.list();return ok({status:result.ok?'healthy' as const:'down' as const,lane:'local',detail:result.ok?'Authenticated CopilotKit OpenDots agent discovery':result.error.humanMessage});}
}
