//! The page a connector sign-in lands on (`/mcp/oauth/callback`,
//! `/connectors/oauth/callback`): the A://TERNIT wordmark, a status mark, a
//! title and one line of next steps, in light and dark. Every string is
//! HTML-escaped (error text can come from the remote authorization server).
//! A success closes its window after a moment; a failure stays open so the
//! reason can be read.

use axum::response::Html;

const WORDMARK: &str = include_str!("../assets/a-ternit-wordmark.svg");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Connected,
    Failed,
}

/// `&`, `<`, `>`, `"` and `'` escaped for HTML text and attribute values.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

pub fn render(outcome: Outcome, title: &str, message: &str) -> Html<String> {
    let (title, message) = (escape(title), escape(message));
    let (mark, tone, close) = match outcome {
        Outcome::Connected => (
            r#"<svg viewBox="0 0 24 24" width="28" height="28" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/></svg>"#,
            "ok",
            r#"<p class="hint">This window closes by itself. You can go back to Allternit.</p><script>setTimeout(function(){window.close()},2500)</script>"#,
        ),
        Outcome::Failed => (
            r#"<svg viewBox="0 0 24 24" width="28" height="28" aria-hidden="true"><path d="M12 7.5v6M12 16.8v.2" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"/></svg>"#,
            "err",
            r#"<p class="hint">Close this window, go back to Allternit and try connecting again.</p>"#,
        ),
    };
    Html(format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>{title} · Allternit</title>
<style>
:root{{--bg:#ffffff;--card:#ffffff;--fg:#141413;--muted:#5e5d59;--line:#e8e6dc;--accent:#d97757;--ok:#2f7d4f;--ok-bg:#e7f3ec;--err:#b3261e;--err-bg:#fbe9e7}}
@media (prefers-color-scheme: dark){{:root{{--bg:#141413;--card:#1c1c1a;--fg:#f0eee6;--muted:#a8a69c;--line:#2e2d2a;--ok:#7fcf9c;--ok-bg:#1d3326;--err:#f2b8b5;--err-bg:#3a1e1c}}}}
*{{box-sizing:border-box}}
body{{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;padding:24px 16px;background:var(--bg);color:var(--fg);font-family:'Allternit Sans',Inter,ui-sans-serif,system-ui,-apple-system,sans-serif;-webkit-font-smoothing:antialiased}}
main{{width:100%;max-width:420px;background:var(--card);border:1px solid var(--line);border-radius:16px;padding:32px 28px;text-align:center}}
.brand{{color:var(--fg);display:flex;justify-content:center;margin-bottom:28px}}
.brand svg{{width:168px;height:auto}}
.mark{{width:52px;height:52px;border-radius:50%;display:flex;align-items:center;justify-content:center;margin:0 auto 16px}}
.ok .mark{{background:var(--ok-bg);color:var(--ok)}}
.err .mark{{background:var(--err-bg);color:var(--err)}}
h1{{font-size:19px;font-weight:600;letter-spacing:-.01em;margin:0 0 8px}}
p{{margin:0;font-size:14px;line-height:1.55;color:var(--muted);overflow-wrap:anywhere}}
.hint{{margin-top:20px;padding-top:16px;border-top:1px solid var(--line);font-size:13px}}
</style>
</head>
<body>
<main class="{tone}">
<div class="brand">{WORDMARK}</div>
<div class="mark">{mark}</div>
<h1>{title}</h1>
<p>{message}</p>
{close}
</main>
</body>
</html>"#
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_remote_text_and_brands_the_page() {
        let Html(page) = render(Outcome::Failed, "Couldn't connect", r#"<script>alert(1)</script> & "x""#);
        assert!(!page.contains("<script>alert(1)"), "remote text is escaped");
        assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;x&quot;"));
        assert!(page.contains(r#"aria-label="Allternit""#), "wordmark is inline");
        assert!(!page.contains("window.close"), "a failure stays open");
        let Html(ok) = render(Outcome::Connected, "Connected", "Gmail is connected.");
        assert!(ok.contains("window.close") && ok.contains(r#"class="ok""#));
    }
}
