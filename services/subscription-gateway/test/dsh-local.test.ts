import { describe, expect, it } from 'vitest';
import { DshLocalTransport } from '../adapters/_shared/dsh-local.js';
import { runWithCallScope } from '../src/aai/call-scope.js';
const token='a'.repeat(43);
function fixture(foreign=false) { const calls: RequestInit[]=[];
 const fetcher:typeof fetch=(async(url: any,init:any)=>{calls.push(init);if(new URL(String(url)).pathname==='/')return new Response(null,{status:303,headers:{location:'./','set-cookie':'dsh-auth-authority=signed.cookie; HttpOnly; Path=/'}});const body=JSON.parse(init.body);return Response.json({type:'server-response',rpcId:foreign?'foreign':body.rpcId,result:{ok:true,value:{accepted:true}}});}) as typeof fetch;
 return {transport:new DshLocalTransport({baseUrl:'http://127.0.0.1:8899',token,fetch:fetcher}),calls}; }
describe('DSH authenticated RPC transport',()=>{
 it('exchanges the launch token without following redirects, keeps the cookie scoped and matches RPC replies',async()=>{const {transport,calls}=fixture();expect(await transport.rpc('session/prompt',{request:{requestId:'request-1',sessionId:'session-1'}})).toMatchObject({ok:true,value:{accepted:true}});expect(calls[0].redirect).toBe('manual');expect(new Headers(calls[1].headers).get('cookie')).toBe('dsh-auth-authority=signed.cookie');expect(new Headers(calls[1].headers).get('authorization')).toBeNull();expect(await transport.rpc('session/page',{request:{}})).toMatchObject({ok:true});expect(calls).toHaveLength(3);});
 it('refuses foreign RPC results and unauthorized operations',async()=>{expect(await fixture(true).transport.rpc('session/prompt',{request:{}})).toMatchObject({ok:false,error:{code:'ADAPTER_DRIFT'}});expect(await fixture().transport.rpc('session/delete',{})).toMatchObject({ok:false,error:{code:'POLICY_DENIED'}});expect(()=>new DshLocalTransport({baseUrl:'https://remote.example',token})).toThrow(/loopback/);});
 it('never shares a cookie between account bindings',async()=>{const {transport,calls}=fixture();for(const id of ['one','two'])await runWithCallScope({binding:{id},credential:{apiKey:token}},()=>transport.rpc('session/page',{request:{}}));expect(calls).toHaveLength(4);});
});
