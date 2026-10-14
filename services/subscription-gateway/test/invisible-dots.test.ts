import { describe, expect, it } from 'vitest';
import { InvisibleDotsProvider } from '../adapters/invisible-dots/provider.js';
import { runWithCallScope } from '../src/aai/call-scope.js';
function fixture(mode:'ok'|'foreign'|'lost'|'drift'='ok') {const calls:{path:string;body:any;auth:string|null}[]=[];let poll=0;
 const fetcher:typeof fetch=async(url,init)=>{const u=new URL(String(url));const path=u.pathname;const body=init?.body?JSON.parse(String(init.body)):undefined;calls.push({path,body,auth:new Headers(init?.headers).get('authorization')});
 if(path==='/api/dots')return Response.json({dots:[{id:'dot-1',name:'My Dot',status:'READY'}]});
 if(body){if(mode==='lost')throw Error('private-token');return Response.json({message_id:'msg-1',event_id:1,delivery:'queued'},{status:202});}
 if(path.endsWith('/messages'))return Response.json({messages:mode==='drift'?[{}]:++poll>1?[{event_id:5,role:'assistant',text:'Vendor answer',in_reply_to:mode==='foreign'?'other':'msg-1'}]:[{event_id:2,role:'assistant',text:'Unrelated',in_reply_to:'other'}]});return Response.json({}, {status:404});};
 return {provider:new InvisibleDotsProvider({token:'private-token',fetch:fetcher,pollMs:1,replyTimeoutMs:15}),calls}; }
async function open(p:InvisibleDotsProvider){const r=await p.contextOpen({agentId:'invisible-dots:dot-1'});if(!r.ok)throw Error(r.error.humanMessage);return r.value.contextId;}
describe('Invisible Dots authenticated conversations',()=>{
 it('discovers the real Dot and waits for the receipt-matched reply even when admission is queued',async()=>{const {provider,calls}=fixture();const contextId=await open(provider);const input={contextId,correlationId:'send-1',text:'Hello'};const result=await provider.contextMessage(input);expect(result).toMatchObject({ok:true,value:{reply:'Vendor answer',messageId:'msg-1'}});expect(await provider.contextMessage(input)).toEqual(result);expect(await provider.contextMessage({...input,text:'Changed'})).toMatchObject({ok:false,error:{code:'POLICY_DENIED'}});expect(calls.filter(c=>c.body)).toHaveLength(1);expect(calls.find(c=>c.body)?.body).toEqual({text:'Hello'});expect(calls.every(c=>c.auth==='Bearer private-token')).toBe(true);expect(await provider.events({contextId})).toMatchObject({ok:true,value:{events:[{event:{type:'agent.message.completed',correlationId:'send-1'}}]}});expect(await provider.contextClose({contextId})).toMatchObject({ok:true});expect(calls.filter(c=>c.body)).toHaveLength(1);});
 it('rejects unrelated replies, malformed histories and uncertain sends without repeating mutations',async()=>{for(const mode of ['foreign','lost','drift'] as const){const {provider,calls}=fixture(mode);const contextId=await open(provider);const input={contextId,correlationId:'send-1',text:'Hello'};const result=await provider.contextMessage(input);expect(result.ok).toBe(false);expect(await provider.contextMessage(input)).toEqual(result);expect(calls.filter(c=>c.body)).toHaveLength(1);expect(JSON.stringify(result)).not.toContain('private-token');}});
 it('requires authorization and isolates account context ownership',async()=>{expect((await new InvisibleDotsProvider().list()).ok).toBe(false);expect(()=>new InvisibleDotsProvider({baseUrl:'http://remote.example:8787'})).toThrow(/loopback/);const {provider}=fixture();const contextId=await runWithCallScope({binding:{id:'a'},credential:{apiKey:'a'}},()=>open(provider));expect(await runWithCallScope({binding:{id:'b'},credential:{apiKey:'b'}},()=>provider.contextMessage({contextId,correlationId:'send-1',text:'Other account'}))).toMatchObject({ok:false,error:{code:'CONTEXT_NOT_FOUND'}});});
 it('prevents concurrent sends through two contexts attached to the same shared Dot',async()=>{
 const {provider,calls}=fixture('foreign');const first=await open(provider);const second=await open(provider);
 const pending=provider.contextMessage({contextId:first,correlationId:'send-1',text:'First'});
 expect(await provider.contextMessage({contextId:second,correlationId:'send-2',text:'Second'})).toMatchObject({ok:false,error:{code:'CONTEXT_BUSY'}});
 await pending;expect(calls.filter(c=>c.body)).toHaveLength(1);
 });

});
