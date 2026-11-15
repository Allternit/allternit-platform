export default function App(){const [env,setEnv]=React.useState('win');const [open,setOpen]=React.useState(true);const d=DEV[env];const r=env==='web'?ROWS[6]:ROWS[0];const f=fit(d,r);
 const groups=[['On '+d.name+' · works offline',[[r.n,f.v.f+' '+f.v.q+' · '+f.v.e,true],[ROWS[6].n,'downloaded']]],['My other devices',[["Joe's MacBook Pro · Qwen3.5 27B",'through my Allternit account'],["Joe's iPhone · Qwen3.5 2B",'when the app is open']]],['Cloud',[['Allternit Cloud models','metered']]]];
 return(<div style={{background:'var(--bg)'}}><EnvSwitch env={env} setEnv={setEnv} only={['mac','win','linux','web']}/>
<Chrome tab="Playground" d={d}>
<div style={{border:'1px solid var(--line)',borderRadius:14,overflow:'hidden',maxWidth:820}}>
<div style={{display:'flex',alignItems:'center',gap:10,padding:'10px 14px',borderBottom:'1px solid var(--line)',position:'relative'}}>
<button onClick={()=>setOpen(!open)} style={{display:'flex',alignItems:'center',gap:8,border:'1px solid var(--line2)',borderRadius:10,padding:'5px 10px',background:'transparent',color:'var(--ink)',fontFamily:'inherit',fontSize:13,cursor:'pointer'}}><b>{r.n}</b><span style={{fontSize:11,fontWeight:600,color:'var(--ok)',background:'var(--fill)',borderRadius:999,padding:'1px 8px'}}>On this device</span>▾</button>
<span className="mono" style={{fontSize:11.5,color:'var(--ink3)'}}>{f.v.f} {f.v.q} · {f.v.e} · temp 0.7 · ctx 8,192</span>
{open?<div style={{position:'absolute',top:46,left:14,width:380,background:'var(--bg)',border:'1px solid var(--line2)',borderRadius:12,boxShadow:'0 12px 30px rgba(0,0,0,.12)',padding:8,zIndex:2}}>{groups.map(([g,items])=>(<div key={g} style={{marginBottom:6}}><div className="mono" style={{fontSize:10.5,color:'var(--ink3)',textTransform:'uppercase',letterSpacing:'.08em',padding:'6px 8px 2px'}}>{g}</div>{items.map(([n,s,on])=>(<div key={n} style={{display:'flex',justifyContent:'space-between',padding:'6px 8px',borderRadius:8,background:on?'var(--fill)':'transparent'}}><div><div style={{fontSize:13}}>{n}</div><div className="mono" style={{fontSize:11,color:'var(--ink3)'}}>{s}</div></div>{on?'✓':''}</div>))}</div>))}</div>:null}
</div>
<div style={{padding:'18px 16px',display:'grid',gap:14,minHeight:300}}>
<div style={{justifySelf:'end',background:'var(--fill2)',borderRadius:14,padding:'8px 12px',maxWidth:'70%',fontSize:13.5}}>Summarize this contract clause in two lines.</div>
<div style={{maxWidth:'80%',fontSize:13.5,lineHeight:1.55}}>The supplier must fix defects within 30 days at no cost. If it misses that window, you can end the contract and get a pro-rated refund.
<div className="mono" style={{display:'flex',gap:12,flexWrap:'wrap',marginTop:8,fontSize:11,color:'var(--ink3)'}}><span>{env==='win'?88:env==='linux'?95:env==='web'?38:46} tok/s</span><span>first token 0.3 s</span><span>{env==='web'?'0.7':'3.4'} GB</span><span>212 / 8,192 ctx</span><span>{f.v.e}</span><span style={{color:'var(--ok)'}}>offline</span></div></div>
</div>
<div style={{borderTop:'1px solid var(--line)',padding:'10px 14px',color:'var(--ink3)',fontSize:13}}>Message {r.n}…</div></div>
</Chrome></div>);}
