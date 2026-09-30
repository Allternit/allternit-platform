import { toCss, type LookPack } from "@allternit/look-packs";
import { allternitExtSource, openaiCompatSource } from "@allternit/apps-bridge/sources";
import { kitSource } from "@allternit/apps-ui/sources";
import type { KitFields, KitTag } from "@allternit/apps-ui";
import type { McpAppResourceCsp } from "./lint.js";

export interface KitViewSpec {
  tag: KitTag;
  /** Dot path into the tool result's `structuredContent` where the data lives. Default: the whole object. */
  path?: string;
  fields?: KitFields;
  /** A tool to call with the form values when a <allternit-form> fires `allternit-submit`. */
  submitTool?: string;
}

export interface ViewInput {
  /** `ui://<app>/<name>.html` */
  uri: string;
  name: string;
  description?: string;
  /** Hand-written View HTML. Provide this or `kit`. */
  html?: string;
  /** Compose the View from the view kit. */
  kit?: KitViewSpec;
  /** Domains the View may reach. Defaults to none. */
  csp?: McpAppResourceCsp;
  prefersBorder?: boolean;
  /** Inline the window.openai shim (for Views ported from ChatGPT). */
  openaiCompat?: boolean;
  /** Also install `window.allternit`. Default true. */
  allternitExt?: boolean;
}

export interface ViewDef extends Required<Pick<ViewInput, "uri" | "name">> {
  description?: string;
  csp: Required<Pick<McpAppResourceCsp, "connectDomains" | "resourceDomains">> & McpAppResourceCsp;
  prefersBorder: boolean;
  kit?: KitViewSpec;
  render(ctx: { lookPack: LookPack | null; appName: string; appVersion: string }): string;
}

const URI_RE = /^ui:\/\/[a-z0-9][a-z0-9._-]*\/[A-Za-z0-9._\/-]+$/;

/** Keep JSON and script text from closing their own tag or starting an HTML comment. */
const safeInline = (s: string) => s.replace(/<\/(script)/gi, "<\\/$1").replace(/<!--/g, "<\\!--");
const safeJson = (v: unknown) => JSON.stringify(v).replace(/</g, "\\u003c").replace(/[\u2028\u2029]/g, " ");

// The mount script speaks the standard bridge only; it is also what `allternit dev` shows a View doing.
const MOUNT_SCRIPT = `(function(){
var cfg=JSON.parse(document.getElementById("allternit-view").textContent);
var el=document.getElementById("view");
function post(m){parent.postMessage(Object.assign({jsonrpc:"2.0"},m),"*")}
function pick(o,p){if(!p)return o;var parts=p.split("."),i;for(i=0;i<parts.length;i++){if(o==null||typeof o!=="object")return undefined;o=o[parts[i]]}return o}
function size(){post({method:"ui/notifications/size-changed",params:{height:document.documentElement.scrollHeight}})}
function vars(c){var v=c&&c.styles&&c.styles.variables;if(!v)return;Object.keys(v).forEach(function(k){if(v[k]&&k.indexOf("--")===0)document.documentElement.style.setProperty(k,v[k])})}
el.fields=cfg.fields||{};
window.addEventListener("message",function(e){
var m=e.data;if(e.source!==parent||!m||m.jsonrpc!=="2.0")return;
if(m.id===1&&m.result){vars(m.result.hostContext);post({method:"ui/notifications/initialized",params:{}});size()}
else if(m.method==="ui/notifications/tool-result"){el.data=pick(m.params&&m.params.structuredContent,cfg.path);size()}
else if(m.method==="ui/notifications/host-context-changed"){vars(m.params)}
else if(m.method==="ui/resource-teardown"&&m.id!==undefined){post({id:m.id,result:{}})}
});
if(cfg.submitTool){el.addEventListener("allternit-submit",function(e){post({id:2,method:"tools/call",params:{name:cfg.submitTool,arguments:e.detail.values}})})}
post({id:1,method:"ui/initialize",params:{protocolVersion:"2026-01-26",appInfo:{name:cfg.app,version:cfg.version},appCapabilities:{}}});
})();`;

export function view(input: ViewInput): ViewDef {
  if (!URI_RE.test(input.uri)) throw new Error(`view ${input.uri}: uri must look like ui://<app>/<name>.html`);
  if (!input.name.trim()) throw new Error(`view ${input.uri}: name is required`);
  if ((input.html === undefined) === (input.kit === undefined)) {
    throw new Error(`view ${input.uri}: provide exactly one of html or kit`);
  }
  const csp = { ...input.csp, connectDomains: input.csp?.connectDomains ?? [], resourceDomains: input.csp?.resourceDomains ?? [] };
  return {
    uri: input.uri,
    name: input.name,
    description: input.description,
    csp,
    prefersBorder: input.prefersBorder ?? true,
    kit: input.kit,
    render({ lookPack, appName, appVersion }) {
      const head: string[] = [];
      if (lookPack) head.push(`<style>${toCss(lookPack, { selector: ":root", darkSelector: '[data-theme="dark"]' })}</style>`);
      if (input.allternitExt !== false) head.push(`<script>${safeInline(allternitExtSource())}</script>`);
      if (input.openaiCompat) head.push(`<script>${safeInline(openaiCompatSource())}</script>`);
      if (input.html !== undefined) {
        const inject = head.join("\n");
        return /<head[^>]*>/i.test(input.html) ? input.html.replace(/<head[^>]*>/i, (m) => `${m}\n${inject}`) : `${inject}\n${input.html}`;
      }
      const kit = input.kit!;
      const cfg = { app: appName, version: appVersion, path: kit.path, fields: kit.fields, submitTool: kit.submitTool };
      return [
        "<!doctype html>",
        '<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">',
        "<style>body{margin:0;padding:12px;font-family:var(--vp-font-body,system-ui,sans-serif)}</style>",
        ...head,
        "</head><body>",
        `<${kit.tag} id="view"></${kit.tag}>`,
        `<script type="application/json" id="allternit-view">${safeJson(cfg)}</script>`,
        `<script>${safeInline(kitSource())}</script>`,
        `<script>${MOUNT_SCRIPT}</script>`,
        "</body></html>",
      ].join("\n");
    },
  };
}
