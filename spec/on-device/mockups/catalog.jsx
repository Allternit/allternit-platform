const HAVE={mac:['Qwen3.5-4B-GGUF'],win:['Qwen3.5-4B-GGUF'],linux:['Qwen3.5-27B-GGUF'],iphone:['Qwen3.5-2B-GGUF'],android:['Qwen3.5-2B-GGUF'],web:['Qwen3-0.6B-GGUF']};
function Action({d,f,r,env}){const have=(HAVE[env]||[]).includes(r.n);
 if(have)return <Btn pri small>Chat</Btn>;
 if(f.k==='no'||f.k==='na')return <Btn small>{d.kind==='phone'||d.kind==='browser'?'Run on my computer':'Send to a bigger device'}</Btn>;
 return <Btn pri small>Run here</Btn>;}
function Variant({f,r,env}){const have=(HAVE[env]||[]).includes(r.n);return(<span className="mono" style={{fontSize:11,color:'var(--ink2)'}}>{f.v.f} {f.v.q} · {f.v.s} GB · {f.v.e}{have?' · downloaded':''}</span>);}
function ListView({d,env,sel,setSel,rows}){return(<div className="view-list">
<div className="list-h" style={{gridTemplateColumns:'minmax(0,1fr) 70px 60px 104px 200px'}}><span>Subject</span><span className="r">File</span><span className="r">Params</span><span>License</span><span>On {d.kind==='browser'?'this browser':'this device'}</span></div>
{rows.map(r=>{const f=fit(d,r);return(<div key={r.id} className="msg" onClick={()=>setSel(r.id)} style={{gridTemplateColumns:'minmax(0,1fr) 70px 60px 104px 200px',cursor:'pointer',background:sel===r.id?'var(--fill)':undefined}}>
<span className="s"><span className="msg-org msg-mono" dangerouslySetInnerHTML={{__html:r.logo?'<img class="org" src="'+r.logo+'" width="22" height="22">':FW.word(r.mono,9,'mono-svg')}}/><span className="s-t"><span className="s-n">{r.n}</span><small><span className="hw hw-host">Hosted (signed)</span> <b>From:</b> {r.org} <b>in</b> {r.sec}</small></span></span>
<span className="z mono">{f.v.s} GB</span><span className="pp mono">{FW.params(r.params)}</span><span className="lcc"><span className={'lc lc-'+r.lc}>{r.lc==='permissive'?'Permissive':r.lc}</span></span>
<span style={{display:'flex',alignItems:'center',gap:8,justifyContent:'space-between'}}><Pill f={f} small/><Action d={d} f={f} r={r} env={env}/></span></div>);})}</div>);}
function GridView({d,env,rows}){return(<div className="cards tiles">{rows.map(r=>{const f=fit(d,r);return(<div key={r.id} style={{display:'flex',flexDirection:'column',gap:8}}>
<H html={FW.tile({...r,size:f.v.s*1e9})}/>
<div style={{border:'1px solid var(--line)',borderRadius:12,padding:'8px 10px',display:'grid',gap:6}}><div style={{display:'flex',justifyContent:'space-between',alignItems:'center',gap:6}}><Pill f={f} small/><Action d={d} f={f} r={r} env={env}/></div><Variant f={f} r={r} env={env}/></div></div>);})}</div>);}
function RunPanel({d,r,env}){const f=fit(d,r);const [vi,setVi]=React.useState(-1);const opts=r.v.filter(v=>d.kind==='browser'?v.e==='WebGPU':d.apple?true:v.e!=='MLX'&&v.e!=='WebGPU');
 return(<div style={{borderTop:'1px solid var(--line)',padding:'16px 20px',display:'grid',gap:10}}>
<div className="mono" style={{fontSize:11,color:'var(--ink3)',textTransform:'uppercase',letterSpacing:'.08em'}}>Run on {d.name}</div>
<div style={{display:'flex',gap:8,alignItems:'center',flexWrap:'wrap'}}><Pill f={f}/><span style={{fontSize:12.5,color:'var(--ink2)'}}>{f.why}</span></div>
{opts.length?opts.map((v,i)=>{const on=vi===-1?v===f.v:vi===i;return(<label key={i} onClick={()=>setVi(i)} style={{display:'flex',alignItems:'center',gap:10,border:'1px solid '+(on?'var(--ink)':'var(--line)'),borderRadius:10,padding:'8px 12px',cursor:'pointer'}}><span style={{width:14,height:14,borderRadius:7,border:'2px solid var(--ink)',background:on?'var(--ink)':'transparent'}}/><span style={{flex:1,fontSize:13}}><b>{v.f} {v.q}</b> <span style={{color:'var(--ink2)'}}>with {v.e}</span>{v===f.v?<span className="mono" style={{fontSize:10.5,color:'var(--ok)',marginLeft:8}}>best for this device</span>:null}</span><span className="mono" style={{fontSize:12}}>{v.s} GB</span></label>);}):<div style={{fontSize:13,color:'var(--ink2)'}}>No build of this model runs in a browser tab.</div>}
<div style={{display:'flex',gap:8,flexWrap:'wrap'}}><Action d={d} f={f} r={r} env={env}/><Btn small>Send to my iPhone</Btn><Btn small>Open on Freeweights</Btn></div>
<div style={{fontSize:11.5,color:'var(--ink3)'}}>Files are checked against the signed Freeweights manifest (sha256) before the model loads.</div></div>);}
function DeckView({d,env}){const [i,setI]=React.useState(0);const r0=ROWS[i];const r={...r0,size:fit(d,r0).v.s*1e9};return(<div className="view-deck" style={{maxWidth:760}}>
<div className="deck-nav"><button className="iconbtn" onClick={()=>setI((i+ROWS.length-1)%ROWS.length)}>‹</button><span className="mono deck-count">{i+1} of {ROWS.length}</span><button className="iconbtn" onClick={()=>setI((i+1)%ROWS.length)}>›</button></div>
<article className="fwk"><H cls="fwk-rec" html={FW.record(r,'full')}/><div className="fwk-mid"><H cls="fwk-id" html={FW.identity(r)}/><H cls="fwk-facts" html={FW.facts(r)}/></div><RunPanel d={d} r={r0} env={env} key={env+i}/></article></div>);}
export default function App(){const [env,setEnv]=React.useState('mac');const [view,setView]=React.useState('grid');const [sel,setSel]=React.useState(ROWS[0].id);const [fits,setFits]=React.useState(false);const d=DEV[env];
 const shown=fits?ROWS.filter(r=>['ok','slow'].includes(fit(d,r).k)):ROWS;
 return(<div style={{background:'var(--bg)'}}><EnvSwitch env={env} setEnv={setEnv}/>
<Chrome tab="Catalog" d={d}>
<div style={{display:'grid',gridTemplateColumns:'190px minmax(0,1fr)',gap:18}}>
<aside style={{fontSize:13}}><div className="mono" style={{fontSize:11,color:'var(--ink3)',textTransform:'uppercase',letterSpacing:'.08em',marginBottom:6}}>On this device</div>
{[['Everything',false],['Fits this device',true]].map(([l,v])=>(<div key={l} onClick={()=>setFits(v)} style={{padding:'5px 8px',borderRadius:8,cursor:'pointer',background:fits===v?'var(--fill2)':'transparent',fontWeight:fits===v?600:400}}>{l}</div>))}
<div className="mono" style={{fontSize:11,color:'var(--ink3)',textTransform:'uppercase',letterSpacing:'.08em',margin:'14px 0 6px'}}>Collections</div>
{['For this device','Phone','Laptop','Workstation','Browser','GGUF','MLX'].map(c=><div key={c} style={{padding:'4px 8px',color:'var(--ink2)'}}>{c}</div>)}
<div className="mono" style={{fontSize:11,color:'var(--ink3)',textTransform:'uppercase',letterSpacing:'.08em',margin:'14px 0 6px'}}>Sections</div>
{[['qwen',405],['gemma',366],['smollm',35],['phi',19],['llama',239],['lfm',104]].map(([s,n])=><div key={s} className="mono" style={{display:'flex',justifyContent:'space-between',padding:'3px 8px',fontSize:12.5,color:'var(--ink2)'}}><span>{s}</span><span style={{color:'var(--ink3)'}}>{n}</span></div>)}</aside>
<div style={{minWidth:0}}>
<div className="toolbar" style={{display:'flex',gap:8,alignItems:'center',flexWrap:'wrap'}}><label className="q" style={{flex:1,minWidth:180}}><input type="search" placeholder="Search 8,900 models" style={{width:'100%'}}/></label>
<select className="sort" defaultValue="fit" style={{padding:'7px 10px',borderRadius:10,border:'1px solid var(--line)',background:'var(--bg)',color:'var(--ink)'}}><option value="fit">Best for this device</option><option>Best first</option><option>Newest</option><option>Size: smallest</option></select>
<div className="seg" role="group">{[['list','List'],['grid','Grid'],['deck','Deck']].map(([v,l])=><button key={v} className="segb" aria-pressed={view===v} onClick={()=>setView(v)}>{l}</button>)}</div></div>
<div className="results-h" style={{margin:'10px 0 8px',fontSize:13,color:'var(--ink2)'}}>{shown.length} models · sizes are the file you download, not the whole repo</div>
{view==='list'?<ListView d={d} env={env} sel={sel} setSel={setSel} rows={shown}/>:view==='grid'?<GridView d={d} env={env} rows={shown}/>:<DeckView d={d} env={env}/>}
</div></div></Chrome></div>);}
