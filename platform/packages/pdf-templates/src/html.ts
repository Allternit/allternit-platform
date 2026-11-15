import { PDF_PAGE_POINTS } from './templates';
import type { PdfBlock, PdfDocModel } from './types';

const esc = (s: string): string =>
  s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);
const multi = (s: string): string => esc(s).replace(/\n/g, '<br>');

function block(b: PdfBlock): string {
  switch (b.t) {
    case 'masthead': {
      const meta = b.meta.map(([k, v]) => `<div><span>${esc(k)}</span>${esc(v)}</div>`).join('');
      return `<header class="mast mast-${b.style}"><div class="mast-l">${b.brand ? `<p class="brand">${esc(b.brand)}</p>` : ''}<h1>${esc(b.title)}</h1>${
        b.subtitle ? `<p class="sub">${esc(b.subtitle)}</p>` : ''
      }</div><div class="mast-r">${meta}</div></header>`;
    }
    case 'cols':
      return b.cols.length
        ? `<section class="cols">${b.cols.map((c) => `<div><h3>${esc(c.label)}</h3>${c.lines.map((l) => `<p>${esc(l)}</p>`).join('')}</div>`).join('')}</section>`
        : '';
    case 'table': {
      const th = b.head.map((h, i) => `<th class="${b.align[i] === 'r' ? 'r' : ''}" style="width:${(b.widths[i] * 100).toFixed(1)}%">${esc(h)}</th>`).join('');
      const tr = b.rows
        .map((r) => `<tr>${r.map((c, i) => `<td class="${b.align[i] === 'r' ? 'r' : ''}">${multi(c)}</td>`).join('')}</tr>`)
        .join('');
      return `<table><thead><tr>${th}</tr></thead><tbody>${tr}</tbody></table>`;
    }
    case 'totals':
      return `<dl class="totals">${b.rows.map((r) => `<div class="${r.strong ? 'strong' : ''}"><dt>${esc(r.label)}</dt><dd>${esc(r.value)}</dd></div>`).join('')}</dl>`;
    case 'stats':
      return `<section class="stats">${b.items.map((s) => `<div><span>${esc(s.label)}</span><b>${esc(s.value)}</b>${s.note ? `<em>${esc(s.note)}</em>` : ''}</div>`).join('')}</section>`;
    case 'heading':
      return `<h2>${esc(b.text)}</h2>`;
    case 'text':
      return `<p class="body">${multi(b.text)}</p>`;
    case 'list': {
      const tag = b.numbered ? 'ol' : 'ul';
      return `<${tag}>${b.items.map((i) => `<li>${esc(i)}</li>`).join('')}</${tag}>`;
    }
    case 'note':
      return `<p class="note">${multi(b.text)}</p>`;
    case 'ticket':
      return `<section class="ticket"><div class="tk-main"><h2>${esc(b.event)}</h2>${b.holder ? `<p class="holder">${esc(b.holder)}</p>` : ''}<dl>${b.lines
        .map((l) => `<div><dt>${esc(l.label)}</dt><dd>${esc(l.value)}</dd></div>`)
        .join('')}</dl></div><div class="tk-stub"><span>Ticket code</span><b>${esc(b.code)}</b></div></section>`;
    case 'rule':
      return '<hr>';
    case 'space':
      return `<div style="height:${b.h}px"></div>`;
  }
}

/** A complete print-ready HTML document. `@page` carries the paper size so Chromium's printToPDF needs no extra options. */
export function renderHtml(doc: PdfDocModel): string {
  const t = doc.theme;
  const [w, h] = PDF_PAGE_POINTS[doc.pageSize];
  const css = `
@page{size:${w}pt ${h}pt;margin:0}
*{box-sizing:border-box}
html{-webkit-print-color-adjust:exact;print-color-adjust:exact}
body{margin:0;background:#fff;color:${t.ink};font:10.5pt/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",Helvetica,Arial,sans-serif}
.page{width:${w}pt;min-height:${h}pt;padding:48pt;margin:0 auto}
h1{font-size:24pt;line-height:1.15;margin:0;letter-spacing:-.01em}
h2{font-size:12.5pt;margin:20pt 0 6pt;break-after:avoid}
h3{font-size:8pt;text-transform:uppercase;letter-spacing:.08em;color:${t.muted};margin:0 0 4pt}
p{margin:0 0 3pt}
.mast{display:flex;justify-content:space-between;gap:24pt;margin-bottom:22pt;padding-bottom:14pt;border-bottom:1.5pt solid ${t.accent}}
.mast .brand{color:${t.muted};font-size:9pt;text-transform:uppercase;letter-spacing:.08em}
.mast .sub{color:${t.muted};margin-top:4pt}
.mast-r{text-align:right;font-size:9.5pt}
.mast-r span{color:${t.muted};margin-right:8pt}
.mast-minimal{border-bottom:.5pt solid ${t.line}}
.mast-minimal h1{font-size:14pt;text-transform:uppercase;letter-spacing:.12em;font-weight:600}
.mast-modern{background:${t.band};color:${t.bandInk};margin:-48pt -48pt 24pt;padding:36pt 48pt 24pt;border:0}
.mast-modern .brand,.mast-modern .sub,.mast-modern .mast-r span{color:${t.bandInk};opacity:.7}
.cols{display:flex;gap:32pt;margin-bottom:18pt}.cols>div{flex:1}
table{width:100%;border-collapse:collapse;margin:6pt 0 10pt}
th{background:${t.fill};font-size:8.5pt;text-transform:uppercase;letter-spacing:.06em;text-align:left;padding:6pt 8pt;color:${t.muted}}
td{padding:7pt 8pt;border-bottom:.5pt solid ${t.line};vertical-align:top}
tr{break-inside:avoid}
.r{text-align:right}
.totals{margin:4pt 0 12pt auto;width:55%}
.totals div{display:flex;justify-content:space-between;padding:4pt 8pt}
.totals .strong{border-top:1.5pt solid ${t.accent};margin-top:4pt;padding-top:8pt;font-weight:700;font-size:12pt}
.totals dt,.totals dd,.ticket dt,.ticket dd{margin:0}
.stats{display:flex;gap:10pt;margin-bottom:12pt}
.stats>div{flex:1;background:${t.fill};border-radius:6pt;padding:10pt 12pt}
.stats span{display:block;font-size:8pt;text-transform:uppercase;letter-spacing:.06em;color:${t.muted}}
.stats b{display:block;font-size:17pt}.stats em{font-style:normal;font-size:8.5pt;color:${t.muted}}
ul,ol{margin:0 0 6pt;padding-left:16pt}li{margin-bottom:2pt}
.note{color:${t.muted};font-size:9pt;margin-top:12pt}
.ticket{display:flex;border:1pt solid ${t.line};border-radius:8pt;overflow:hidden;margin:6pt 0;break-inside:avoid}
.tk-main{flex:1;padding:18pt 20pt}.tk-main h2{margin:0 0 4pt;font-size:18pt}
.holder{color:${t.muted};margin-bottom:10pt}
.tk-main dl{display:grid;grid-template-columns:1fr 1fr;gap:8pt 20pt;margin:0}
.tk-main dt{font-size:8pt;text-transform:uppercase;letter-spacing:.06em;color:${t.muted}}
.tk-stub{width:150pt;border-left:1.5pt dashed ${t.line};background:${t.fill};display:flex;flex-direction:column;justify-content:center;align-items:center;padding:12pt;text-align:center}
.tk-stub span{font-size:8pt;text-transform:uppercase;letter-spacing:.06em;color:${t.muted}}
.tk-stub b{font:600 12pt ui-monospace,Menlo,Consolas,monospace;margin-top:6pt;word-break:break-all}
hr{border:0;border-top:.5pt solid ${t.line};margin:12pt 0}
.foot{margin-top:24pt;padding-top:8pt;border-top:.5pt solid ${t.line};color:${t.muted};font-size:8.5pt}
@media screen{body{background:#ECECEC}.page{background:#fff;margin:16px auto;box-shadow:0 1px 6px rgba(0,0,0,.12)}}`;
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${esc(
    doc.title,
  )}</title><style>${css}</style></head><body><main class="page">${doc.blocks.map(block).join('')}${
    doc.footer ? `<p class="foot">${esc(doc.footer)}</p>` : ''
  }</main></body></html>`;
}
