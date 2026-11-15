/**
 * MCP Apps in the terminal: a tool whose `_meta.ui.resourceUri` names a
 * `ui://` resource gets one line under its result, "MCP App · View in
 * browser". The line opens a local page that hosts the app the way Desktop's
 * MCP App frame does (sandboxed iframe, `ui/initialize`, then the tool's input
 * and result), so the app shows its real data. Best effort: a server that
 * can't serve the resource just gets no line.
 */
import { createHash } from 'crypto'
import { mkdir, writeFile } from 'fs/promises'
import { tmpdir } from 'os'
import { join } from 'path'
import { pathToFileURL } from 'url'

const MAX_HTML_BYTES = 2 * 1024 * 1024
const views = new Map<string, { file: string; title: string }>()

function keyOf(output: unknown): string {
  return createHash('sha1').update(JSON.stringify(output ?? null)).digest('hex')
}

/** The page that hosts the app: the same JSON-RPC handshake Desktop's frame speaks. */
export function hostPage(args: { title: string; html: string; input: unknown; result: unknown }): string {
  const json = (v: unknown) => JSON.stringify(v ?? null).replace(/</g, '\\u003c')
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>${args.title.replace(/[<&]/g, '')} · MCP App</title>
<style>html,body{margin:0;height:100%;background:#fff;font:13px system-ui}header{padding:8px 12px;color:#555;border-bottom:1px solid #eee}iframe{border:0;width:100%;height:calc(100% - 36px)}@media(prefers-color-scheme:dark){html,body{background:#1d1d1f}header{color:#aaa;border-color:#333}}</style></head>
<body><header>${args.title.replace(/[<&]/g, '')} · MCP App from Gizzi Code</header>
<iframe id="app" sandbox="allow-scripts allow-forms allow-popups"></iframe>
<script>
const APP=${json(args.html)},INPUT=${json(args.input)},RESULT=${json(args.result)};
const f=document.getElementById('app');
const send=m=>f.contentWindow.postMessage(Object.assign({jsonrpc:'2.0'},m),'*');
const dark=matchMedia('(prefers-color-scheme: dark)').matches;
addEventListener('message',e=>{if(e.source!==f.contentWindow)return;const m=e.data||{};
 if(m.method==='ui/initialize'){send({id:m.id,result:{protocolVersion:(m.params&&m.params.protocolVersion)||'2025-06-18',hostInfo:{name:'gizzi-code',version:'1'},hostCapabilities:{openLinks:{}},hostContext:{theme:dark?'dark':'light',displayMode:'inline'}}});return}
 if(m.method==='ui/notifications/initialized'){send({method:'ui/notifications/tool-input',params:{arguments:INPUT}});send({method:'ui/notifications/tool-result',params:RESULT});return}
 if(m.method==='ui/open-link'&&m.params&&/^https?:/.test(m.params.url)){open(m.params.url,'_blank','noopener');if(m.id!=null)send({id:m.id,result:{}});return}
 if(m.id!=null&&m.method)send({id:m.id,error:{code:-32601,message:'Not available in the browser view. Open the chat in Allternit Desktop for the full app.'}});
});
f.srcdoc=APP;
</script></body></html>`
}

/**
 * Read the tool's ui:// resource and write its host page. Keyed by the
 * result content, which is what the result renderer is handed.
 */
export async function prepareAppView(args: {
  readResource: (uri: string) => Promise<{ contents?: Array<{ text?: string; blob?: string }> }>
  resourceUri: string
  title: string
  input: unknown
  result: { content?: unknown; structuredContent?: unknown; _meta?: unknown }
}): Promise<void> {
  try {
    const res = await args.readResource(args.resourceUri)
    const first = res.contents?.[0]
    const html = first?.text ?? (first?.blob ? Buffer.from(first.blob, 'base64').toString('utf8') : undefined)
    if (!html || html.length > MAX_HTML_BYTES) return
    const key = keyOf(args.result.content)
    const dir = join(tmpdir(), 'gizzi-mcp-apps')
    await mkdir(dir, { recursive: true })
    const file = join(dir, `${key.slice(0, 16)}.html`)
    await writeFile(file, hostPage({ title: args.title, html, input: args.input, result: args.result }))
    views.set(key, { file, title: args.title })
  } catch {
    // No app view; the text result still shows.
  }
}

/** The app view for a rendered result, if its tool served one. */
export function appViewFor(output: unknown): { url: string; file: string; title: string } | undefined {
  const view = views.get(keyOf(output))
  return view ? { ...view, url: pathToFileURL(view.file).href } : undefined
}
