# Making artifacts

An artifact is something you make that the user would show someone: a doc, deck, sheet, design, dashboard, motion piece, web page, card, diagram, image or code file. It is saved to the user's Artifacts (one account-level store, private until they share it), shows in the chat as a card, and opens in its own window in Allternit, where they can edit, version, export and share it.

Keep in sync with the app's manuals in allternit-ai `src/lib/artifacts/` (output-choice.ts, editors/design-instructions.ts, motion/instructions.ts, dashboard/instructions.ts). When the app is the client it sends these per turn; this skill covers sessions without them (terminal, bots, channels).

## When to make one

- Make one with `artifact_create` when the content is substantial (about 15 lines or more), self-contained and likely to be edited or reused, or when the user asks ("make this an artifact", `/docs`, `/slides`, `/sheets`, `/design`, `/dashboard`, `/motion`).
- Otherwise answer inline. Short snippets and quick answers stay in the reply. Interactive OpenUI cards stay inline; never wrap them in an artifact.
- To change an artifact, call `artifact_update` with its id, the complete new body and the `base_version` you started from. Never create a second artifact for a revision. If the user may have edited it since, call `artifact_read` first and apply your change to the latest body.
- Don't repeat the body in your reply. Say in one sentence what you made.
- Give it a short title the user would recognise ("Q3 launch plan") and, optionally, one generic `icon` word (chart, calendar, code).

## Finding and managing artifacts

- `artifact_list` finds artifacts by title words, kind or scope (mine / shared with me). Use it when the user names one ("update the launch brief").
- `artifact_share` with only an id shows who can open it; with visibility (private, people, org, link) and people it changes sharing. Only when asked; the user approves.
- `artifact_delete` deletes permanently (no trash). Only when the user asked to delete that artifact; the user approves.
- `artifact_comment` reads threads, or posts a comment or reply (parent_id). Answer comments that mention you there.
- `artifact_storage` reads or writes a page artifact's saved data (personal or shared scope).

## Kinds and bodies

### doc — `text/markdown`
Plain Markdown: headings, lists, tables, links. The editor turns it into its block format on first edit (`application/vnd.allternit.doc+json`); you can always write Markdown.

### sheet — `application/vnd.allternit.sheet+json`
`{"columns":["Region","Q3","Q4"],"rows":[["EMEA",120,140],["APAC",90,110]]}`. A bare 2-D array or CSV text also loads. Numbers as numbers, not strings.

### slides — `application/vnd.allternit.slides+json`
`{"title":"Board update","slides":[{"title":"Where we are","bullets":["Revenue up 12%","Two launches shipped"]},{"title":"Next quarter","body":"One paragraph of text"}]}`. One idea per slide, at most about 6 bullets of under 12 words each.

### design — `application/vnd.allternit.design+json`
A canvas document, never an HTML page (an HTML design opens read-only until the user converts it).

```json
{ "v": 2, "kind": "canvas", "designSystem": null,
  "artboards": [{ "id": "home", "name": "Home", "x": 0, "y": 0, "w": 1440, "h": 900, "fill": "#ffffff" }],
  "layers": [{ "id": "title", "type": "text", "name": "Title", "artboard": "home", "x": 96, "y": 120, "w": 800, "h": 80,
               "props": { "text": "Plan your week in minutes", "size": "xl", "color": "black", "w": 800 } }] }
```

- Artboards are screens or pages: phone 390x844, desktop 1440x900, social 1080x1080 or 1200x675.
- Layers sit on an artboard with x/y relative to it. Array order is stacking order, first at the bottom. Keep layouts on an 8px grid. Every layer also carries w and h in props.
- Layer types and props:
  - text: text, size (s | m | l | xl), color (black | grey | light-violet | violet | blue | light-blue | yellow | orange | green | light-green | light-red | red | white), w (wraps when set)
  - geo: geo (rectangle | ellipse | triangle | diamond | star | hexagon | cloud | arrow-right | check-box), w, h, text, color, fill (none | semi | solid | pattern)
  - design-component: w, h, label, componentType (custom | button | input | card | nav | modal | badge), fill, stroke (CSS colours), radius
  - design-uiblock: w, h, variant (button-primary | button-secondary | input | card | nav-bar | badge | avatar | divider)
  - design-html: w, h, label, html. Only for one block the shapes above can't express (a chart, an embedded table). Never a whole page.

### dashboard — `application/vnd.allternit.dashboard+json`
Live: every tile runs its own query against the viewer's connectors when opened and on a timer. Never put made-up numbers in a dashboard. If the user has no connector tools, say so and offer to build it once they connect one.

```json
{"version":1,"refreshSeconds":300,
 "filters":[{"id":"region","label":"Region","type":"select","options":[{"value":"emea","label":"EMEA"}],"default":"emea"}],
 "tiles":[{"id":"revenue","title":"Revenue","type":"kpi","layout":{"x":0,"y":0,"w":3,"h":2},
           "query":{"tool":"<connector>__<tool>","args":{}},
           "encoding":{"y":["total"],"compare":"previous"},"format":{"style":"currency","currency":"USD"}}]}
```

- Tile types: kpi (one number; `encoding.y[0]` is the column, `encoding.compare` an optional previous-period column), line, area, bar (`encoding.x` the x column, `encoding.y` the series columns), table (`encoding.columns` optional).
- 12-column grid; layout x, y, w (1–12), h (rows of 72px). KPIs w 3 h 2, charts w 6 h 4, tables w 12 h 5.
- `query.tool` is exactly the name of a connector tool you have (`<connector>__<tool>`), with `query.args`. For a warehouse SQL tool put the statement in `query.sql` and the tool's argument name in `query.sqlArg` (default "query"); only one read-only SELECT, WITH, SHOW or DESCRIBE runs. Rows come back as an array of objects or `{columns, rows}`; set `query.rowsPath` (like "data.rows") if they sit elsewhere. Name result columns to match encoding.
- Filters: select, text or number. Use `{{filters.<id>}}` in `query.args` or `query.sql` (in SQL it becomes a quoted literal: `WHERE region = {{filters.region}}`). `format.style`: number, compact, currency, percent, text or date (optional decimals, prefix, suffix).
- On changes ("add a region filter", "switch to weekly") keep tile ids stable so cached values and layout survive.
- Dashboards are on paid plans (Team on by default, Enterprise when an admin turns them on).

### motion — `application/vnd.allternit.motion+json`
A short animated explainer, data story or title card. The body is JSON, never code: every word, number, colour and duration is data.

```json
{"version":1,"width":1920,"height":1080,"fps":30,
 "theme":{"bg":"#0b0b0f","fg":"#f5f5f7","accent":"#6d7cff","muted":"#8e8ea0","font":"sans"},
 "scenes":[{"id":"s1","type":"title","title":"Intro","duration":3,"transition":{"type":"cut","duration":0.5},"props":{"text":"Q3 in 20 seconds","subtitle":"Revenue, deals, what's next","align":"center"}}]}
```

- Scene types and props: title {text, subtitle, align: left | center}; counter {label, from, to, prefix, suffix, decimals, caption}; chart {chart: bar | line, title, labels, values, unit}; logo {text, caption, src: https or data:image URL, optional}; list {title, items}.
- Colours are #rrggbb; font is sans, serif or mono. Scenes last 0.5–60 s; at most 40 scenes and 180 s in all, 24 chart points, 12 list items. The first scene's transition is "cut"; later ones fade, wipe, slide or cut.
- Keep text short (a headline is about 8 words). Use real figures from the conversation or the source artifact; never invent numbers. No generated footage or people.
- Edits change only what was asked: "slow scene 2" raises that scene's duration; everything else stays.
- Motion is on Super, Ultra and Team plans.

### page — `text/html` or `text/markdown`
A complete, self-contained page: inline CSS and JS, works offline, readable on a phone (no fixed widths, 16px side padding), light and dark.

### card — `application/vnd.allternit.openui`
OpenUI Lang. Only when the user wants a saved interactive card; cards in a reply stay inline.

### diagram — `text/vnd.mermaid` or `image/svg+xml`
Mermaid source, or a standalone SVG with a viewBox.

### image — `text/uri-list`
The image's URL, not its data.

### code — `text/plain`
With `meta.language` ("python") and optionally `meta.filename`.

## Errors

- "Artifact … has changed since version N": someone edited it. Nothing was saved. `artifact_read`, re-apply your change to the latest body, retry with the new `base_version`.
- The store refuses a save: say why in one sentence (the message says what to change). Don't switch to a different kind silently.
- Invalid body: fix the body to the shape above and retry once.
