import { describe, expect, it } from 'vitest';
import { HivekeepProvider } from '../adapters/hivekeep/provider.js';
import { runWithCallScope } from '../src/aai/call-scope.js';
function fixture(mode: 'ok'|'foreign'|'lost'|'denied'='ok') {
 const calls:{path:string;body:any;auth:string|null}[]=[];let polls=0;
 const fetcher:typeof fetch=async (url,init)=>{const path=new URL(String(url)).pathname;const body=init?.body?JSON.parse(String(init.body)):undefined;calls.push({path,body,auth:new Headers(init?.headers).get('authorization')});
 if(mode==='denied')return Response.json({}, {status:403});
 if(path==='/api/v1/agents')return Response.json({agents:[{id:'agent-1',name:'Hive'}]});
 if(path.endsWith('/conversations'))return Response.json({conversationId:'conversation-1'},{status:201});
 if(path.endsWith('/messages')){if(mode==='lost')throw Error('secret');return Response.json({requestId:'request-1',status:'pending',conversationId:'conversation-1'},{status:202});}
 if(path.includes('/requests/'))return Response.json({requestId:'request-1',status:++polls>1?'done':'pending',reply:polls>1?'Actual reply':null,conversationId:mode==='foreign'?'foreign':'conversation-1'});
 if(path.endsWith('/close'))return Response.json({ok:true});return Response.json({}, {status:404});};
 return {provider:new HivekeepProvider({token:'private-key',fetch:fetcher,pollMs:1,replyTimeoutMs:100}),calls};
}
async function open(provider:HivekeepProvider){const r=await provider.contextOpen({agentId:'hivekeep:agent-1'});if(!r.ok)throw Error(r.error.humanMessage);return r.value.contextId;}
describe('Hivekeep isolated bearer API',()=>{
 it('creates an isolated context, polls its request and replays the matched completion without duplicate sends',async()=>{const {provider,calls}=fixture();const contextId=await open(provider);const input={contextId,correlationId:'send-1',text:'Hello'};const result=await provider.contextMessage(input);expect(result).toMatchObject({ok:true,value:{reply:'Actual reply'}});expect(await provider.contextMessage(input)).toEqual(result);expect(calls.filter(c=>c.path.endsWith('/messages'))).toHaveLength(1);expect(calls.find(c=>c.path.endsWith('/messages'))?.body).toEqual({content:'Hello',conversationId:'conversation-1',mode:'async'});expect(calls.every(c=>c.auth==='Bearer private-key')).toBe(true);expect(await provider.events({contextId})).toMatchObject({ok:true,value:{events:[{event:{type:'agent.message.completed',correlationId:'send-1'}}]}});expect(await provider.contextClose({contextId})).toEqual({ok:true,value:{closed:true}});});
 it('refuses foreign replies and retains uncertain send outcomes',async()=>{for(const mode of ['foreign','lost'] as const){const {provider,calls}=fixture(mode);const contextId=await open(provider);const input={contextId,correlationId:'send-1',text:'Hello'};const result=await provider.contextMessage(input);expect(result.ok).toBe(false);expect(await provider.contextMessage(input)).toEqual(result);expect(calls.filter(c=>c.path.endsWith('/messages'))).toHaveLength(1);expect(JSON.stringify(result)).not.toContain('private-key');}});
 it('checks authentication and account ownership without enabling unsupported controls',async()=>{expect(()=>new HivekeepProvider({baseUrl:'https://remote.example'})).toThrow(/loopback/);expect((await new HivekeepProvider().list()).ok).toBe(false);expect(await fixture('denied').provider.list()).toMatchObject({ok:false,error:{code:'AUTH_REVOKED'}});const {provider}=fixture();const contextId=await runWithCallScope({binding:{id:'a'},credential:{apiKey:'a'}},()=>open(provider));expect(await runWithCallScope({binding:{id:'b'},credential:{apiKey:'b'}},()=>provider.contextMessage({contextId,correlationId:'send-1',text:'Hello'}))).toMatchObject({ok:false,error:{code:'CONTEXT_NOT_FOUND'}});});
});
