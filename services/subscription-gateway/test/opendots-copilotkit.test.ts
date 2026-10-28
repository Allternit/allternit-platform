import { describe, expect, it } from 'vitest';
import { OpenDotsCopilotkitProvider } from '../adapters/opendots-copilotkit/provider.js';
import { runWithCallScope } from '../src/aai/call-scope.js';
import { fakePhoenix, replyEvents, type PhoenixScript } from './fake-phoenix.js';
function fixture(mode:'ok'|'unauthenticated'|'foreign'|'error'|'silent'='ok') {
 const calls:{path:string;body:any;auth:string|null}[]=[];
 const fetcher:typeof fetch=async(url,init)=>{const path=new URL(String(url)).pathname;const auth=new Headers(init?.headers).get('authorization');const body=init?.body?JSON.parse(String(init.body)):undefined;calls.push({path,body,auth});
 if(!auth&&mode!=='unauthenticated')return Response.json({}, {status:401});
 if(path==='/api/workspace')return Response.json({dots:[{id:'dot-1',name:'Research Dot'}]});
 if(path==='/api/conversations')return Response.json({id:'thread-1',dotId:'dot-1',ownerId:'owner'});
 if(path==='/api/copilotkit/info')return Response.json({mode:'intelligence',agents:{'dot-1':{}},intelligence:{wsUrl:'wss://realtime.intelligence.copilotkit.ai/socket'}});
 return Response.json({threadId:mode==='foreign'?'other':'thread-1',runId:'run-1',joinToken:'scoped-token',realtime:{clientUrl:'wss://realtime.intelligence.copilotkit.ai/socket',topic:'thread:thread-1'}});};
 const script:PhoenixScript=mode==='error'?{events:(_t,runId)=>[{type:'RUN_STARTED',threadId:'thread-1',runId},{type:'RUN_FINISHED',threadId:'thread-1',runId}]}:mode==='silent'?{events:()=>[]}:{events:replyEvents('thread-1','Realtime reply')};
 const phoenix=fakePhoenix(script);
 const provider=new OpenDotsCopilotkitProvider({token:'owner-secret',fetch:fetcher,socket:phoenix.factory,replyTimeoutMs:100});
 return {provider,phoenix,calls};
}
async function open(p:OpenDotsCopilotkitProvider){const r=await p.contextOpen({agentId:'opendots-copilotkit:dot-1'});if(!r.ok)throw Error(r.error.humanMessage);return r.value.contextId;}
describe('CopilotKit OpenDots Intelligence transport',()=>{
 it('creates an owned isolated thread and uses only the current run reply',async()=>{const {provider,phoenix,calls}=fixture();const contextId=await open(provider);const input={contextId,correlationId:'send-1',text:'Hello'};expect(await provider.contextMessage(input)).toMatchObject({ok:true,value:{reply:'Realtime reply'}});expect(await provider.contextMessage(input)).toMatchObject({ok:true});expect(phoenix.urls).toHaveLength(1);expect(await provider.contextMessage({...input,text:'Other'})).toMatchObject({ok:false,error:{code:'POLICY_DENIED'}});expect(calls.find(c=>c.path==='/api/conversations')?.body).toEqual({dotId:'dot-1',title:'Allternit'});const run=calls.find(c=>c.path==='/api/copilotkit/agent/dot-1/run');expect(run?.auth).toBe('Bearer owner-secret');expect(run?.body).toMatchObject({threadId:'thread-1',tools:[],context:[],messages:[{role:'user',content:'Hello'}]});expect(phoenix.urls[0]).toBe('wss://realtime.intelligence.copilotkit.ai/socket/websocket?join_token=scoped-token&vsn=2.0.0');expect(phoenix.sent[0]).toEqual(['1','1','thread:thread-1','phx_join',{stream_mode:'run',run_id:'run-1'}]);expect(phoenix.closed()).toBe(1);expect(await provider.contextClose({contextId})).toMatchObject({ok:true});});
 it('refuses a runtime without enforced owner authentication and foreign realtime receipts',async()=>{expect(await fixture('unauthenticated').provider.list()).toMatchObject({ok:false,error:{code:'POLICY_DENIED'}});const {provider,phoenix}=fixture('foreign');const contextId=await open(provider);expect(await provider.contextMessage({contextId,correlationId:'send-1',text:'Hello'})).toMatchObject({ok:false,error:{code:'ADAPTER_DRIFT'}});expect(phoenix.urls).toHaveLength(0);});
 it('never substitutes historical replies or another account context',async()=>{const {provider}=fixture('error');const contextId=await runWithCallScope({binding:{id:'a'},credential:{apiKey:'a'}},()=>open(provider));expect(await runWithCallScope({binding:{id:'b'},credential:{apiKey:'b'}},()=>provider.contextMessage({contextId,correlationId:'send-1',text:'Hello'}))).toMatchObject({ok:false,error:{code:'CONTEXT_NOT_FOUND'}});expect(await runWithCallScope({binding:{id:'a'},credential:{apiKey:'a'}},()=>provider.contextMessage({contextId,correlationId:'send-1',text:'Hello'}))).toMatchObject({ok:false,error:{code:'ADAPTER_DRIFT'}});});
 it('reports an unfinished run as unconfirmed instead of retrying',async()=>{const {provider,phoenix,calls}=fixture('silent');const contextId=await open(provider);expect(await provider.contextMessage({contextId,correlationId:'send-1',text:'Hello'})).toMatchObject({ok:false,error:{code:'VENDOR_UNAVAILABLE',retryable:false}});expect(calls.filter(c=>c.path.endsWith('/run'))).toHaveLength(1);expect(phoenix.closed()).toBe(1);});
 it('runs the REST admission path and returns revoked authentication without opening a realtime connection',async()=>{
 const calls:string[]=[];const fetcher=(async(url: any,init:any)=>{const path=new URL(String(url)).pathname;calls.push(path);const auth=new Headers(init?.headers).get('authorization');
 if(!auth)return Response.json({}, {status:401});
 if(path==='/api/workspace')return Response.json({dots:[{id:'dot-1',name:'Dot'}]});
 if(path==='/api/conversations')return Response.json({id:'thread-1',dotId:'dot-1'});
 if(path==='/api/copilotkit/info')return Response.json({mode:'intelligence',agents:{'dot-1':{}},intelligence:{wsUrl:'wss://realtime.intelligence.copilotkit.ai/socket'}});
 return Response.json({}, {status:401});}) as typeof fetch;
 const phoenix=fakePhoenix();const provider=new OpenDotsCopilotkitProvider({token:'owner-secret',fetch:fetcher,socket:phoenix.factory,replyTimeoutMs:1000});const contextId=await open(provider);
 expect(await provider.contextMessage({contextId,correlationId:'send-1',text:'Hello'})).toMatchObject({ok:false,error:{code:'AUTH_REVOKED'}});
 expect(calls).toContain('/api/copilotkit/agent/dot-1/run');expect(phoenix.urls).toHaveLength(0);
 });
});
