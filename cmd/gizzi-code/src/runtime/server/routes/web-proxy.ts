import { Hono } from "hono"
import { lazy } from "@/shared/util/lazy"
import { isEgressHostAllowed } from "@/shared/utils/hooks/ssrfGuard"

/**
 * SSRF-safe web proxy that fetches a URL server-side and strips
 * X-Frame-Options / CSP frame-ancestors so the response can be embedded
 * in an <iframe> on the client.
 *
 * Key fixes applied to the HTML body before returning:
 *  1. Injects a <base href="https://target-origin/"> tag so that all relative
 *     URLs (images, scripts, stylesheets) resolve against the real origin,
 *     not the proxy origin.  Without this every relative path 404s.
 *  2. Rewrites <a href="..."> and <form action="..."> links to go through the
 *     proxy so that in-page navigation stays inside the iframe instead of
 *     breaking out to the real site.
 *  3. Injects a small script that intercepts window.location assignments and
 *     history API calls so JS-driven navigation also stays proxied.
 *
 * Route: GET /web-proxy?url=<encoded-url>
 */
export const WebProxyRoutes = lazy(() =>
  new Hono().get("/", async (c) => {
    const targetUrl = c.req.query("url")
    if (!targetUrl) {
      return c.json({ error: "Missing ?url= query parameter" }, 400)
    }

    // ── Scheme check ────────────────────────────────────────────────
    let parsed: URL
    try {
      parsed = new URL(targetUrl)
    } catch {
      return c.json({ error: "Invalid URL" }, 400)
    }

    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      return c.json({ error: "Only http/https URLs are allowed" }, 403)
    }

    // ── SSRF protection: block private / loopback IPs ───────────────
    // Validates the literal hostname AND every DNS-resolved address
    // before fetching; redirects are followed manually so each hop is
    // re-validated the same way (see fetch loop below).
    if (!(await isPublicHostname(parsed.hostname))) {
      return c.json({ error: "Requests to private/loopback addresses are blocked" }, 403)
    }

    // ── Fetch with timeout, following redirects manually ────────────
    // redirect: "manual" so we can re-validate every hop's hostname and
    // resolved IPs — a public hostname that redirects to (or resolves
    // to) 169.254.169.254 etc. must not slip through.
    const MAX_REDIRECTS = 5
    const REDIRECT_STATUSES = [301, 302, 303, 307, 308]
    let upstream: Response | null = null
    try {
      let currentUrl = targetUrl
      for (let hop = 0; hop <= MAX_REDIRECTS; hop++) {
        const hopUrl = new URL(currentUrl)
        if (hopUrl.protocol !== "http:" && hopUrl.protocol !== "https:") {
          return c.json({ error: "Only http/https URLs are allowed" }, 403)
        }
        if (!(await isPublicHostname(hopUrl.hostname))) {
          return c.json({ error: "Requests to private/loopback addresses are blocked" }, 403)
        }
        const res = await fetch(currentUrl, {
          headers: {
            "User-Agent":
              "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
            Accept: "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            "Accept-Language": "en-US,en;q=0.9",
          },
          redirect: "manual",
          signal: AbortSignal.timeout(15_000),
        })
        if (REDIRECT_STATUSES.includes(res.status)) {
          const location = res.headers.get("location")
          if (!location || hop === MAX_REDIRECTS) {
            return c.json({ error: "Too many redirects" }, 502)
          }
          currentUrl = new URL(location, currentUrl).toString()
          continue
        }
        upstream = res
        break
      }
    } catch (err: any) {
      const message = err?.name === "TimeoutError" ? "Upstream request timed out" : "Failed to fetch upstream URL"
      return c.json({ error: message }, 502)
    }
    if (!upstream) {
      return c.json({ error: "Too many redirects" }, 502)
    }

    // ── Decide content type ─────────────────────────────────────────
    const upstreamContentType = upstream.headers.get("content-type") ?? ""
    const isHtml = upstreamContentType.includes("text/html") ||
      upstreamContentType === "" ||
      !upstreamContentType  // treat unknown as HTML

    // ── Non-HTML assets: stream through without modification ─────────
    if (!isHtml) {
      const responseHeaders = new Headers()
      for (const [key, value] of upstream.headers.entries()) {
        const lower = key.toLowerCase()
        if (lower === "x-frame-options") continue
        if (lower === "content-security-policy") continue
        // Never forward the upstream site's own CORS policy — the global
        // cors middleware governs access to this authenticated route.
        if (lower.startsWith("access-control-")) continue
        if (lower === "content-encoding") continue
        if (lower === "transfer-encoding") continue
        if (lower === "connection") continue
        if (lower === "server") continue
        responseHeaders.set(key, value)
      }
      return new Response(upstream.body, {
        status: upstream.status,
        statusText: upstream.statusText,
        headers: responseHeaders,
      })
    }

    // ── Get HTML body ───────────────────────────────────────────────
    let bodyText: string
    try {
      bodyText = await upstream.text()
    } catch (err: any) {
      return c.json({ error: "Failed to read response body" }, 502)
    }

    // ── Track the final URL after redirects ──────────────────────────
    const finalUrl = upstream.url || targetUrl
    const finalParsed = (() => { try { return new URL(finalUrl) } catch { return parsed } })()
    const baseOrigin = `${finalParsed.protocol}//${finalParsed.host}`

    // ── Proxy prefix used for link rewriting ─────────────────────────
    // The client sends requests to /web-proxy?url=<encoded>; we want
    // the host-relative version so it works on any port.
    const proxyPrefix = "/web-proxy?url="

    // ── Helper: make a URL absolute then wrap in proxy prefix ─────────
    function proxifyUrl(rawUrl: string): string {
      if (
        !rawUrl ||
        rawUrl.startsWith("data:") ||
        rawUrl.startsWith("blob:") ||
        rawUrl.startsWith("javascript:") ||
        rawUrl.startsWith("#") ||
        rawUrl.startsWith("mailto:") ||
        rawUrl.startsWith("tel:")
      ) {
        return rawUrl
      }
      // Already proxied
      if (rawUrl.startsWith(proxyPrefix)) return rawUrl
      try {
        const abs = new URL(rawUrl, finalUrl).toString()
        // Don't re-proxy if it's already pointing to the proxy host
        if (abs.includes("/web-proxy?url=")) return rawUrl
        return `${proxyPrefix}${encodeURIComponent(abs)}`
      } catch {
        return rawUrl
      }
    }

    // ── Rewrite navigation links (<a href>, <form action>) ────────────
    // We rewrite navigation targets so that clicking links loads through
    // the proxy.  Static assets (img src, script src, link href for CSS)
    // are left alone so they resolve via the injected <base> tag and load
    // directly from the origin — that avoids double-fetching everything.
    function rewriteNavLinks(html: string): string {
      // <a href="...">
      html = html.replace(
        /(<a\b[^>]+\shref=)(["'])([^"']*)\2/gi,
        (_match, prefix, quote, url) => {
          const rewritten = proxifyUrl(url)
          return `${prefix}${quote}${rewritten}${quote}`
        },
      )
      // <form action="...">
      html = html.replace(
        /(<form\b[^>]+\saction=)(["'])([^"']*)\2/gi,
        (_match, prefix, quote, url) => {
          const rewritten = proxifyUrl(url)
          return `${prefix}${quote}${rewritten}${quote}`
        },
      )
      return html
    }

    // ── Inject <base> tag and proxy navigation script ─────────────────
    // The <base href> tells the browser where to resolve relative paths.
    // The injected script intercepts JS-driven navigations.
    const baseTag = `<base href="${baseOrigin}/">`

    // Tiny script: posts "navigate" messages to the parent so the React
    // shell can update the URL bar and load the new URL through the proxy.
    const navInterceptScript = `<script>
(function(){
  var _proxyPrefix = '/web-proxy?url=';
  function toProxy(url) {
    if (!url || url.charAt(0) === '#') return url;
    try {
      var abs = new URL(url, '${finalUrl}').toString();
      if (abs.indexOf('/web-proxy?url=') !== -1) return abs;
      return _proxyPrefix + encodeURIComponent(abs);
    } catch(e) { return url; }
  }
  // Intercept window.location assignments
  try {
    var _loc = window.location;
    Object.defineProperty(window, 'location', {
      get: function() { return _loc; },
      set: function(url) {
        var proxied = toProxy(String(url));
        window.parent.postMessage({ type: 'gizzi-navigate', url: proxied }, '*');
      },
      configurable: true,
    });
  } catch(e) { /* some browsers block defineProperty on location */ }
  // Intercept history API
  var _push = history.pushState.bind(history);
  var _replace = history.replaceState.bind(history);
  history.pushState = function(state, title, url) {
    if (url) {
      var proxied = toProxy(String(url));
      window.parent.postMessage({ type: 'gizzi-navigate', url: proxied }, '*');
      return;
    }
    return _push(state, title, url);
  };
  history.replaceState = function(state, title, url) {
    if (url) {
      var proxied = toProxy(String(url));
      window.parent.postMessage({ type: 'gizzi-navigate', url: proxied }, '*');
      return;
    }
    return _replace(state, title, url);
  };
})();
</script>`

    // Insert base tag + nav intercept right after <head> (or at the top)
    const headClose = baseTag + navInterceptScript
    if (/<head(\s[^>]*)?>/i.test(bodyText)) {
      bodyText = bodyText.replace(/(<head(\s[^>]*)?>)/i, `$1${headClose}`)
    } else if (/<html(\s[^>]*)?>/i.test(bodyText)) {
      bodyText = bodyText.replace(/(<html(\s[^>]*)?>)/i, `$1<head>${headClose}</head>`)
    } else {
      bodyText = `<head>${headClose}</head>\n` + bodyText
    }

    // Rewrite <a> and <form> links
    bodyText = rewriteNavLinks(bodyText)

    // ── Build response, stripping frame-blocking headers ────────────
    const responseHeaders = new Headers()
    for (const [key, value] of upstream.headers.entries()) {
      const lower = key.toLowerCase()
      // Strip headers that prevent iframe embedding
      if (lower === "x-frame-options") continue
      if (lower === "content-security-policy") continue
      if (lower === "x-content-type-options") continue
      if (lower === "content-encoding") continue  // We've already decoded
      if (lower === "content-length") continue    // Will differ now
      if (lower === "transfer-encoding") continue
      if (lower === "connection") continue
      if (lower === "server") continue
      if (lower === "set-cookie") continue        // Don't pass cookies
      // Never forward the upstream site's own CORS policy — the global
      // cors middleware governs access to this authenticated route.
      if (lower.startsWith("access-control-")) continue
      responseHeaders.set(key, value)
    }

    // Set proper headers for iframe compatibility. No explicit
    // Access-Control-Allow-Origin here: the global cors middleware applies
    // the same origin allowlist as every other (authenticated) route.
    responseHeaders.set("Content-Type", "text/html; charset=utf-8")
    responseHeaders.set("X-Frame-Options", "ALLOWALL")

    return new Response(bodyText, {
      status: upstream.status,
      statusText: upstream.statusText,
      headers: responseHeaders,
    })
  }),
)

// ── Helpers ───────────────────────────────────────────────────────────

/**
 * True only if the hostname is not private by literal check AND every
 * address its DNS A/AAAA records resolve to is public. Fails closed:
 * any resolution error or empty result is treated as non-public.
 */
function isPublicHostname(hostname: string): Promise<boolean> {
  return isEgressHostAllowed(hostname, { allowLoopback: false })
}
