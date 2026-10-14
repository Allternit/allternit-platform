import { AsyncLocalStorage } from 'node:async_hooks';
import { randomUUID } from 'node:crypto';
import { BaseAaiProvider, fail, ok, type AaiResult, type AgentSummary, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult, type CursoredEvent } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { isObject, type Json } from '../_shared/paired-local.js';

export const CAPABILITIES = agentCapabilityManifestSchema.parse({
 vendor:'rakazo',adapterId:'rakazo',lane:'local',guarantee:'best_effort',
 context:{supported:true,resume:false,parallel:false,maxParallel:1,isolation:'shared'},
 messaging:{send:true,stream:false,steer:false,interrupt:false,cancel:false},
 memory:{read:false,write:false,snapshot:false,opaque:true},tools:{tools:false,mcp:false,plugins:false,connectors:false},
 tasks:{list:false,schedule:false,cancel:false,background:false},approvals:{read:false,respond:false,exact:false},
 computer:{view:false,control:false,takeover:false},artifacts:{read:false,write:false,export:false},
 events:{native:true,polling:false,transcriptDerived:false,replay:true},runtime:{alwaysOn:false,localRequired:false,cloud:true}
});

type Scope={binding?:{id?:string;accountBindingId?:string};credential?:{apiKey:string}};
type Credential={token:string;spaceId:string};
type Bot={id:string;spaceId:string;name:string;threadId:string;archivedAt:string|null};
type Context={id:string;owner:string;spaceId:string;bot:Bot;closed:boolean;busy:boolean;sends:Map<string,Promise<AaiResult<{messageId:string;correlationId:string;reply:string;guarantee:'best_effort'}>>>;sendTexts:Map<string,string>;events:CursoredEvent[]};
const scopeStore=()=> (globalThis as unknown as Record<symbol,AsyncLocalStorage<Scope>|undefined>)[Symbol.for('allternit.aai.callScope')]?.getStore();
const idLike=(v:unknown):v is string=>typeof v==='string'&&/^[\w-]{1,200}$/.test(v);
const unwrap=(v:unknown):unknown=>isObject(v)?v.json:undefined;
const textFromBlocks=(v:unknown)=>Array.isArray(v)?v.filter(isObject).filter(b=>b.kind==='text'&&typeof b.text==='string').map(b=>String(b.text)).join('\n\n'):'';

/** Direct native Rakazo oRPC calls; uses the selected bot's existing shared native thread. */
export class RakazoProvider extends BaseAaiProvider {
 readonly adapterId='rakazo';
 private readonly base?:string;
 private readonly fetchImpl:typeof fetch;
 private readonly configuredSpace?:string;
 private readonly contexts=new Map<string,Context>();
 private readonly running=new Set<string>();
 private readonly timeoutMs:number;
 constructor(options:{baseUrl?:string;spaceId?:string;fetch?:typeof fetch;replyTimeoutMs?:number}={}) {
  super();
  if(!options.baseUrl){this.fetchImpl=options.fetch??fetch;this.configuredSpace=options.spaceId;this.timeoutMs=options.replyTimeoutMs??120000;return;}
  const url=new URL(options.baseUrl);
  const loopback=['127.0.0.1','[::1]','localhost'].includes(url.hostname);
  if((url.protocol!=='https:'&&!(url.protocol==='http:'&&loopback))||url.username||url.password||url.search||url.hash||url.pathname!=='/')throw new Error('Rakazo URL must be an HTTPS origin (or loopback HTTP origin).');
  this.base=url.origin;this.fetchImpl=options.fetch??fetch;this.configuredSpace=options.spaceId;this.timeoutMs=options.replyTimeoutMs??120000;
 }
 private credentials():AaiResult<{owner:string;credential:Credential}> {
  const scope=scopeStore(),raw=scope?.credential?.apiKey;
  if(!raw)return fail('AUTH_REQUIRED','Provide a per-account Rakazo BetterAuth bearer session token and selected space ID.');
  let token=raw,spaceId=this.configuredSpace;
  try { const value:unknown=JSON.parse(raw); if(isObject(value)){token=typeof value.token==='string'?value.token:typeof value.sessionToken==='string'?value.sessionToken:'';if(typeof value.spaceId==='string')spaceId=value.spaceId;} } catch { /* A plain bearer token is supported when space ID is configured separately. */ }
  const owner=scope?.binding?.accountBindingId??scope?.binding?.id;
  if(!owner)return fail('AUTH_REQUIRED','A per-account Rakazo credential binding is required.');
  if(!token||!spaceId||!idLike(spaceId))return fail('AUTH_REQUIRED','Rakazo credential must include a bearer session and selected space ID.');
  return ok({owner,credential:{token,spaceId}});
 }
 private async rpc(path:string,input:Json|undefined,credential:Credential,signal?:AbortSignal):Promise<AaiResult<unknown>> {
  if(!this.base)return fail('AUTH_REQUIRED','Configure SUBS_GATEWAY_RAKAZO_URL with the exact HTTPS origin of your Rakazo server.');
  const controller=AbortSignal.timeout(15000),combined=signal?AbortSignal.any([signal,controller]):controller;
  try {
   const response=await this.fetchImpl(`${this.base}/rpc/${path.split('.').map(encodeURIComponent).join('/')}`,{method:'POST',redirect:'error',signal:combined,headers:{authorization:`Bearer ${credential.token}`,'x-rakazo-space-id':credential.spaceId,accept:'application/json','content-type':'application/json'},body:JSON.stringify(input===undefined?{}:{json:input})});
   if(!response.ok){void response.body?.cancel();if(response.status===401||response.status===403)return fail('AUTH_REVOKED','Rakazo rejected the BetterAuth session or selected-space membership.');if(response.status===429)return fail('RATE_LIMITED','Rakazo is rate limiting RPC requests.');return fail(response.status===404?'CONTEXT_NOT_FOUND':'VENDOR_UNAVAILABLE',`Rakazo RPC returned HTTP ${response.status}.`,{retryable:false});}
   const value:unknown=await response.json();if(!isObject(value)||!Object.hasOwn(value,'json'))return fail('ADAPTER_DRIFT','Rakazo returned an invalid oRPC JSON envelope.',{retryable:false});return ok(value.json);
  }catch{return fail('VENDOR_UNAVAILABLE','Rakazo RPC did not complete. Check the native conversation before retrying.',{retryable:false});}
 }
 private async bots():Promise<AaiResult<{owner:string;credential:Credential;bots:Bot[]}>> {
  if(!this.base)return fail('AUTH_REQUIRED','Configure SUBS_GATEWAY_RAKAZO_URL with the exact HTTPS origin of your Rakazo server.');
  const auth=this.credentials();if(!auth.ok)return auth;
  const response=await this.rpc('bots.list',undefined,auth.value.credential);if(!response.ok)return response;
  const value=response.value;
  if(!Array.isArray(value))return fail('ADAPTER_DRIFT','Rakazo bot roster did not match its pinned contract.');
  const bots:Bot[]=[];
  for(const row of value){if(!isObject(row)||!idLike(row.id)||!idLike(row.spaceId)||!idLike(row.threadId)||typeof row.name!=='string'||(row.archivedAt!==null&&typeof row.archivedAt!=='string'))return fail('ADAPTER_DRIFT','Rakazo returned a malformed bot identity.');if(row.spaceId===auth.value.credential.spaceId&&row.archivedAt===null)bots.push({id:row.id,spaceId:row.spaceId,name:row.name,threadId:row.threadId,archivedAt:null});}
  return ok({...auth.value,bots});
 }
 private summary(bot:Bot):AgentSummary{return {agentId:`rakazo:${bot.id}`,displayName:bot.name,vendor:'rakazo',state:'ready'};}
 async list(){const result=await this.bots();return result.ok?ok(result.value.bots.map(bot=>this.summary(bot))):result;}
 async get(agentId:string){const result=await this.bots();if(!result.ok)return result;const bot=result.value.bots.find(row=>`rakazo:${row.id}`===agentId);return bot?ok({...this.summary(bot),remoteIds:{botId:bot.id,threadId:bot.threadId,spaceId:bot.spaceId},capabilities:CAPABILITIES}):fail('CONTEXT_NOT_FOUND','No active Rakazo bot exists in the selected space.');}
 async capabilities(agentId:string){const result=await this.get(agentId);return result.ok?ok(CAPABILITIES):result;}
 async identity(agentId:string){const result=await this.get(agentId);return result.ok?ok({agentId,displayName:result.value.displayName,vendor:'rakazo'}):result;}
 private live(id:string):AaiResult<Context>{const c=this.contexts.get(id),auth=this.credentials();if(!auth.ok)return auth;return c&&!c.closed&&c.owner===auth.value.owner&&c.spaceId===auth.value.credential.spaceId?ok(c):fail('CONTEXT_NOT_FOUND','This Rakazo bot context is not owned by the current account and selected space.');}
 async contextOpen(input:OpenContextInput){
  const result=await this.bots();if(!result.ok)return result;
  const bot=result.value.bots.find(row=>`rakazo:${row.id}`===input.agentId);if(!bot)return fail('CONTEXT_NOT_FOUND','No active Rakazo bot exists in the selected space.');
  if(input.adoptContextId)return fail('UNSUPPORTED','Rakazo contexts cannot be adopted after gateway detachment.',{retryable:false});
  const id=randomUUID();this.contexts.set(id,{id,owner:result.value.owner,spaceId:bot.spaceId,bot,closed:false,busy:false,sends:new Map(),sendTexts:new Map(),events:[]});
  return ok({contextId:id,isolation:'shared' as const,guarantee:'best_effort' as const,resumed:false});
 }
 async contextMessage(input:MessageInput){
  const ctx=this.live(input.contextId);if(!ctx.ok)return ctx;
  if(!input.text.trim()||!idLike(input.correlationId))return fail('LANE_BLOCKED','A nonempty message and stable send ID are required.');
  const prior=ctx.value.sends.get(input.correlationId);if(prior)return ctx.value.sendTexts.get(input.correlationId)===input.text?prior:fail('POLICY_DENIED','This send ID already belongs to a different message.');
  if(ctx.value.busy)return fail('CONTEXT_BUSY','This Rakazo bot already has an active Allternit request.');
  const lock=`${ctx.value.owner}:${ctx.value.spaceId}:${ctx.value.bot.id}`;if(this.running.has(lock))return fail('CONTEXT_BUSY','This Rakazo bot has an active request in its shared native thread.');
  this.running.add(lock);ctx.value.busy=true;ctx.value.sendTexts.set(input.correlationId,input.text);const remoteNonce=randomUUID();const promise=this.send(ctx.value,input,remoteNonce).finally(()=>{ctx.value.busy=false;this.running.delete(lock);});ctx.value.sends.set(input.correlationId,promise);return promise;
 }
 private async send(context:Context,input:MessageInput,remoteNonce:string):Promise<AaiResult<{messageId:string;correlationId:string;reply:string;guarantee:'best_effort'}>> {
  const base=this.base;if(!base)return fail('AUTH_REQUIRED','Configure SUBS_GATEWAY_RAKAZO_URL with the exact HTTPS origin of your Rakazo server.');
  const auth=this.credentials();if(!auth.ok)return auth;
  const target={botId:context.bot.id},head=await this.rpc('threads.head',target,auth.value.credential);if(!head.ok)return head;
  if(!isObject(head.value)||head.value.threadId!==context.bot.threadId||!Number.isSafeInteger(head.value.cursor))return fail('ADAPTER_DRIFT','Rakazo thread head did not match the selected bot.');
  const sent=await this.rpc('threads.send',{...target,text:input.text,clientNonce:remoteNonce},auth.value.credential);if(!sent.ok)return sent;
  if(!isObject(sent.value)||!idLike(sent.value.runId)||!idLike(sent.value.taskId))return fail('ADAPTER_DRIFT','Rakazo did not return the native run ID for this send.');
  const runId=sent.value.runId,deadline=Date.now()+this.timeoutMs;
  const sub=await this.fetchImpl(`${base}/rpc/threads/subscribe`,{method:'POST',redirect:'error',signal:AbortSignal.timeout(this.timeoutMs),headers:{authorization:`Bearer ${auth.value.credential.token}`,'x-rakazo-space-id':context.spaceId,accept:'text/event-stream','content-type':'application/json'},body:JSON.stringify({json:{...target,cursor:head.value.cursor}})}).catch(()=>undefined);
  if(!sub?.ok||!sub.body)return fail(sub?.status===401||sub?.status===403?'AUTH_REVOKED':'VENDOR_UNAVAILABLE','Rakazo native thread event stream could not be opened.',{retryable:false});
  const reader=sub.body.getReader(),decoder=new TextDecoder();let buffer='',bytes=0,replyBlocks:unknown[]=[],messageId='',terminal:string='',lastSequence=head.value.cursor as number;
  try {
   streamLoop: while(Date.now()<deadline&&!terminal){const part=await reader.read();if(part.done)break;bytes+=part.value.byteLength;if(bytes>8*1024*1024)return fail('ADAPTER_DRIFT','Rakazo run event stream exceeded the connector limit.');buffer=(buffer+decoder.decode(part.value,{stream:true})).replace(/\r\n/g,'\n').replace(/\r(?!$)/g,'\n');let pos:number;while((pos=buffer.indexOf('\n\n'))>=0){const frame=buffer.slice(0,pos);buffer=buffer.slice(pos+2);const name=frame.split('\n').find(line=>line.startsWith('event:'))?.slice(6).trim();const data=frame.split('\n').filter(line=>line.startsWith('data:')).map(line=>line.slice(5).trimStart()).join('\n');if(name==='done')break streamLoop;if(name==='error')return fail('VENDOR_UNAVAILABLE','Rakazo ended the native event stream with an error.',{retryable:false});if(!data)continue;let envelope:unknown;try{envelope=JSON.parse(data);}catch{return fail('ADAPTER_DRIFT','Rakazo emitted invalid event JSON.');}const event=unwrap(envelope);if(!isObject(event)||!idLike(event.id)||!Number.isSafeInteger(event.seq)||typeof event.type!=='string')return fail('ADAPTER_DRIFT','Rakazo emitted a malformed native ProductEvent.');const seq=event.seq as number;if(seq===0&&event.type==='heartbeat')continue;if(seq<=lastSequence)return fail('ADAPTER_DRIFT','Rakazo ProductEvent sequence was duplicated or out of order.');lastSequence=seq;if(event.threadId!==context.bot.threadId||event.botId!==context.bot.id||event.spaceId!==context.spaceId||event.runId!==runId)continue;if(event.type==='thread.message.created'&&isObject(event.payload)&&event.payload.role==='bot'&&Array.isArray(event.payload.blocks)){replyBlocks.push(...event.payload.blocks);messageId=typeof event.payload.messageId==='string'?event.payload.messageId:'';}if(event.type==='run.completed')terminal='completed';if(event.type==='run.failed'||event.type==='run.cancelled')terminal=String(event.type);}}
  }catch{return fail('VENDOR_UNAVAILABLE','Rakazo accepted the send but did not confirm its terminal run event. Check the bot thread before retrying.',{retryable:false});}
  finally{await reader.cancel().catch(()=>undefined);reader.releaseLock();}
  if(terminal!=='completed')return fail(terminal?'VENDOR_UNAVAILABLE':'VENDOR_UNAVAILABLE',terminal==='run.failed'?'Rakazo run failed.':terminal==='run.cancelled'?'Rakazo run was cancelled.':'Rakazo accepted the send but completion was not confirmed. Check the bot thread before retrying.',{retryable:false});
  const reply=textFromBlocks(replyBlocks);if(!messageId||!reply.trim())return fail('ADAPTER_DRIFT','Rakazo completed the matching run without an attributable assistant text message.',{retryable:false});
  const result={messageId,correlationId:input.correlationId,reply,guarantee:'best_effort' as const};
  context.events.push({cursor:String(context.events.length+1),event:{type:'agent.message.completed',botId:`rakazo:${context.bot.id}`,threadId:context.id,generationId:'1',source:'vendor',vendor:'rakazo',adapter:'rakazo',lane:'local',remoteContextId:context.bot.threadId,remoteEventId:messageId,correlationId:input.correlationId,causationId:input.correlationId,guarantee:'best_effort',at:new Date().toISOString(),payload:{reply,messageId,runId}}});
  return ok(result);
 }
 async events(input:EventsInput):Promise<AaiResult<EventsResult>>{const ctx=this.live(input.contextId);if(!ctx.ok)return ctx;const after=Number(input.cursor??0);if(!Number.isSafeInteger(after)||after<0||after>ctx.value.events.length)return fail('CONTEXT_NOT_FOUND','Invalid Rakazo event cursor.');const rows=ctx.value.events.slice(after,after+Math.max(1,Math.min(input.limit??100,500)));return ok({events:rows,nextCursor:String(after+rows.length)});}
 async contextClose(input:{contextId:string}){const ctx=this.live(input.contextId);if(!ctx.ok)return ctx;if(ctx.value.busy)return fail('CONTEXT_BUSY','Wait for the Rakazo run to finish before detaching.');ctx.value.closed=true;return ok({closed:true});}
 async health(){const auth=this.credentials();if(!auth.ok)return ok({status:'down' as const,lane:'local',detail:auth.error.humanMessage});const result=await this.rpc('health',undefined,auth.value.credential);return ok({status:result.ok?'healthy' as const:'down' as const,lane:'local',detail:result.ok?'Authenticated Rakazo native RPC':result.error.humanMessage});}
}
