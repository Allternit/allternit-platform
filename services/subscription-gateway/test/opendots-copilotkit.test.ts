import { describe, expect, it, vi } from 'vitest';
import { OpenDotsCopilotkitProvider } from '../adapters/opendots-copilotkit/provider.js';
import { IntelligenceAgent } from '@copilotkit/core';
import { runWithCallScope } from '../src/aai/call-scope.js';
function fixture(mode:'ok'|'unauthenticated'|'foreign'|'error'='ok') {
 const calls:{path:string;body:any;auth:string|null}[]=[];let configured:any;
 const fetcher:typeof fetch=async(url,init)=>{const path=new URL(String(url)).pathname;const auth=new Headers(init?.headers).get('authorization');const body=init?.body?JSON.parse(String(init.body)):undefined;calls.push({path,body,auth});
 if(!auth&&mode!=='unauthenticated')return Response.json({}, {status:401});
 if(path==='/api/workspace')return Response.json({dots:[{id:'dot-1',name:'Research Dot'}]});
 if(path==='/api/conversations')return Response.json({id:'thread-1',dotId:'dot-1',ownerId:'owner'});
 if(path==='/api/copilotkit/info')return Response.json({mode:'intelligence',agents:{'dot-1':{}},intelligence:{wsUrl:'wss://realtime.intelligence.copilotkit.ai/socket'}});
 return Response.json({threadId:mode==='foreign'?'other':'thread-1',runId:'run-1',joinToken:'scoped-token',realtime:{clientUrl:'wss://realtime.intelligence.copilotkit.ai/socket',topic:'thread:thread-1'}});};
 const client={threadId:'',addMessage:vi.fn(),subscribe:vi.fn(()=>({unsubscribe:vi.fn()})),detachActiveRun:vi.fn().mockResolvedValue(undefined),runAgent:vi.fn(async()=>{await configured.fetch('http://127.0.0.1:4310/api/copilotkit/agent/dot-1/run',{method:'POST',body:JSON.stringify({threadId:client.threadId,runId:'run-1',messages:client.addMessage.mock.calls.map(c=>c[0])})});return {newMessages:mode==='error'?[]:[{id:'answer',role:'assistant',content:'SDK reply'}]};})};
 const provider=new OpenDotsCopilotkitProvider({token:'owner-secret',fetch:fetcher,agentFactory:((config:any)=>{configured=config;return client;}) as any,replyTimeoutMs:50});
 return {provider,client,calls};
}
async function open(p:OpenDotsCopilotkitProvider){const r=await p.contextOpen({agentId:'opendots-copilotkit:dot-1'});if(!r.ok)throw Error(r.error.humanMessage);return r.value.contextId;}
describe('CopilotKit OpenDots Intelligence transport',()=>{
 it('loads the pinned Node SDK and exposes its documented client methods',()=>{const agent=new IntelligenceAgent({agentId:'dot-1',runtimeUrl:'http://127.0.0.1:4310/api/copilotkit',url:'wss://realtime.intelligence.copilotkit.ai/socket',headers:{}});expect(typeof agent.runAgent).toBe('function');expect(typeof agent.detachActiveRun).toBe('function');});
 it('creates an owned isolated thread and uses only the current SDK run reply',async()=>{const {provider,client,calls}=fixture();const contextId=await open(provider);const input={contextId,correlationId:'send-1',text:'Hello'};expect(await provider.contextMessage(input)).toMatchObject({ok:true,value:{reply:'SDK reply'}});expect(await provider.contextMessage(input)).toMatchObject({ok:true});expect(client.runAgent).toHaveBeenCalledTimes(1);expect(await provider.contextMessage({...input,text:'Other'})).toMatchObject({ok:false,error:{code:'POLICY_DENIED'}});expect(calls.find(c=>c.path==='/api/conversations')?.body).toEqual({dotId:'dot-1',title:'Allternit'});expect(client.addMessage.mock.calls[0][0]).toMatchObject({role:'user',content:'Hello'});expect(client.detachActiveRun).toHaveBeenCalledOnce();expect(await provider.contextClose({contextId})).toMatchObject({ok:true});});
 it('refuses a runtime without enforced owner authentication and foreign realtime receipts',async()=>{expect(await fixture('unauthenticated').provider.list()).toMatchObject({ok:false,error:{code:'POLICY_DENIED'}});const {provider}=fixture('foreign');const contextId=await open(provider);expect(await provider.contextMessage({contextId,correlationId:'send-1',text:'Hello'})).toMatchObject({ok:false,error:{code:'ADAPTER_DRIFT'}});});
 it('never substitutes historical replies or another account context',async()=>{const {provider}=fixture('error');const contextId=await runWithCallScope({binding:{id:'a'},credential:{apiKey:'a'}},()=>open(provider));expect(await runWithCallScope({binding:{id:'b'},credential:{apiKey:'b'}},()=>provider.contextMessage({contextId,correlationId:'send-1',text:'Hello'}))).toMatchObject({ok:false,error:{code:'CONTEXT_NOT_FOUND'}});expect(await runWithCallScope({binding:{id:'a'},credential:{apiKey:'a'}},()=>provider.contextMessage({contextId,correlationId:'send-1',text:'Hello'}))).toMatchObject({ok:false,error:{code:'ADAPTER_DRIFT'}});});
 it('runs the real SDK REST admission path and returns revoked authentication without opening a realtime connection',async()=>{
 const calls:string[]=[];const fetcher=(async(url: any,init:any)=>{const path=new URL(String(url)).pathname;calls.push(path);const auth=new Headers(init?.headers).get('authorization');
 if(!auth)return Response.json({}, {status:401});
 if(path==='/api/workspace')return Response.json({dots:[{id:'dot-1',name:'Dot'}]});
 if(path==='/api/conversations')return Response.json({id:'thread-1',dotId:'dot-1'});
 if(path==='/api/copilotkit/info')return Response.json({mode:'intelligence',agents:{'dot-1':{}},intelligence:{wsUrl:'wss://realtime.intelligence.copilotkit.ai/socket'}});
 return Response.json({}, {status:401});}) as typeof fetch;
 const provider=new OpenDotsCopilotkitProvider({token:'owner-secret',fetch:fetcher,replyTimeoutMs:1000});const contextId=await open(provider);
 expect(await provider.contextMessage({contextId,correlationId:'send-1',text:'Hello'})).toMatchObject({ok:false,error:{code:'AUTH_REVOKED'}});
 expect(calls).toContain('/api/copilotkit/agent/dot-1/run');
 });

});
