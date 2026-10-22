#!/usr/bin/env python3
from pathlib import Path
import re, json, sys

ROOT = Path.cwd()
TARGETS = ["apps", "src", "components", "pages", "app", "surfaces", "packages", "styles"]
EXTS = {".css", ".scss", ".sass", ".less", ".tsx", ".ts", ".jsx", ".js", ".html", ".mdx"}

APPROVED_FILES = {"DESIGN.md", "typography.css", "typography.json", "validate-typography.py"}

# Paths exempt from the product typography system. Each falls into one of:
#   - internal/dev tooling, not shipped product UI (platform/packages/ix,
#     plugin-sdk's own docs-site/website/adapters, Storybook fixtures)
#   - the AI design/artifact-generation subsystem, whose entire job is to
#     produce or analyze *other*, user-chosen font stacks for generated
#     output (slides, artifacts, extracted site designs) rather than
#     Allternit's own chrome
#   - a vendored/ported third-party component library, deliberately scoped
#     under its own token namespace so it does NOT collide with Allternit's
#     tokens (see surfaces/ai.allternit.com/src/styles/allternit-design/tokens.css)
#   - generated/example/documentation static content that isn't rendered
#     product UI (newsletter editions, demo pages, plugin examples, docs)
# See docs/learnings/CI_ISSUE_6_NOTES.md for the evidence behind each entry.
EXEMPT_PATH_PREFIXES = [
    "platform/packages/ix",
    "platform/packages/plugin-sdk/docs-site",
    "platform/packages/plugin-sdk/website",
    "platform/packages/plugin-sdk/src/adapters",
    # Office suite packages intentionally reference system fonts (Arial,
    # Times New Roman, Calibri, etc.) for document fidelity when rendering or
    # editing user-provided Office files. Their own chrome should still use
    # Allternit tokens, but the document-rendering engine must preserve
    # external font names.
    "platform/packages/allternit-office-suite",
    "platform/packages/office-docs-app",
    "platform/packages/office-docs-editor",
    "platform/packages/office-docx-engine",
    "platform/packages/office-file-parse",
    "platform/packages/office-pdf-app",
    "platform/packages/office-pdf-viewer",
    "platform/packages/office-pptx-engine",
    "platform/packages/office-pptx-render",
    "platform/packages/office-sheets-app",
    "platform/packages/office-sheets-editor",
    "platform/packages/office-slides-app",
    "platform/packages/office-slides-editor",
    "platform/packages/office-xlsx-engine",
    "surfaces/ai.allternit.com/src/lib/design",
    "surfaces/ai.allternit.com/src/lib/openui",
    "surfaces/ai.allternit.com/src/lib/agents/tools/design-extractor.tool.ts",
    "surfaces/ai.allternit.com/src/lib/agents/tools/design-inspiration.tool.ts",
    "surfaces/ai.allternit.com/src/lib/ai/tools/templates/artifact-templates.ts",
    "surfaces/ai.allternit.com/src/views/design",
    "surfaces/ai.allternit.com/src/views/library",
    "surfaces/ai.allternit.com/src/components/design/QuestionFormView.tsx",
    "surfaces/ai.allternit.com/src/plugins/built-in/slides/plugin.ts",
    "surfaces/ai.allternit.com/src/plugins/vendor",
    "surfaces/ai.allternit.com/src/styles/allternit-design",
    "surfaces/ai.allternit.com/src/allternit-design",
    "surfaces/ai.allternit.com/.storybook",
    "surfaces/ai.allternit.com/public/demos",
    "surfaces/ai.allternit.com/public/editions",
    "surfaces/ai.allternit.com/plugins/examples",
    "surfaces/ai.allternit.com/skills",
    "surfaces/ai.allternit.com/tests",
    "surfaces/ai.allternit.com/src/views/swarm/demo.html",
    "surfaces/ai.allternit.com/src/views/swarm/demo-v2.html",
    "surfaces/ai.allternit.com/src/views/swarm/demo-v3.html",
    "surfaces/ai.allternit.com/src/views/swarm/demo-v4.html",
    "surfaces/ai.allternit.com/src/views/swarm/demo-v5.html",
    "surfaces/allternit-docs",
    "surfaces/allternit-extensions",
    "surfaces/allternit-desktop/src/main/mini-app-oauth-broker.ts",
    "surfaces/docs",
    "surfaces/allternit-mobile/docs",
    "surfaces/gizzi-vscode",
]

FORBIDDEN = [
    re.compile(r"font-family\s*:", re.I),
    re.compile(r"fontFamily\s*:", re.I),
    re.compile(r"@import\s+url\(", re.I),
    re.compile(r"fonts\.googleapis\.com", re.I),
    re.compile(r"\bTimes New Roman\b", re.I),
    re.compile(r"\bArial\b", re.I),
    re.compile(r"\bHelvetica\b", re.I),
    re.compile(r"\bGeorgia\b", re.I),
    # Negative lookaround (not just \b) so compound words like
    # "Inter-Process" (see allternit-os/utils/launchProtocol.ts) don't
    # false-positive on the font name "Inter".
    re.compile(r"(?<![\w-])Inter(?![\w-])", re.I),
]

ALLOWED_TOKENS = [
    "--font-allternit-sans",
    "--font-allternit-serif",
    "--font-allternit-mono",
    "--font-ui",
    "--font-research",
    "--font-code",
    # Semantic aliases defined in theme.css/typography.css that resolve to
    # the --font-allternit-* primitives above (e.g. `--font-sans: var(--font-allternit-sans)`).
    "--font-sans",
    "--font-serif",
    "--font-mono",
    "TYPOGRAPHY.fontFamily",
    "Allternit Sans",
    "Allternit Serif",
    "Allternit Mono",
]

def skip(path: Path) -> bool:
    if (
        path.name in APPROVED_FILES
        or "node_modules" in path.parts
        or ".next" in path.parts
        or "dist" in path.parts
        or "build" in path.parts
    ):
        return True
    if path.suffix == ".mdx" and path.name.endswith(".stories.mdx"):
        return True  # Storybook fixtures, dev-only, never shipped
    if path.suffix in (".tsx", ".ts") and ".stories." in path.name:
        return True  # Storybook fixtures, dev-only, never shipped
    rel = path.relative_to(ROOT).as_posix()
    return any(rel == prefix or rel.startswith(prefix + "/") for prefix in EXEMPT_PATH_PREFIXES)

def allowed(line: str) -> bool:
    if any(token in line for token in ALLOWED_TOKENS):
        return True
    stripped = line.strip()
    # Skip TypeScript interface/type property declarations (not actual font usage)
    if re.search(r"^\s*fontFamily\s*:\s*(string|number|boolean|undefined|any)", line):
        return True
    # A bare object-key opener (Tailwind-style theme config nesting) isn't a
    # font value itself — the literal values it contains are still checked
    # line by line.
    if re.search(r"^fontFamily\s*:\s*\{\s*$", stripped):
        return True
    # `inherit`/`unset`/`initial` don't declare a font at all — search anywhere
    # in the line since inline style objects often chain several properties
    # on one line (e.g. `fontFamily: 'inherit', padding: '24px 16px'`).
    if re.search(r"(font-family|fontFamily)\s*[:=]\s*['\"]?(inherit|unset|initial)['\"]?", line, re.I):
        return True
    return False

violations = []
for target in TARGETS:
    p = ROOT / target
    if not p.exists():
        continue
    for f in p.rglob("*"):
        if not f.is_file() or f.suffix not in EXTS or skip(f):
            continue
        text = f.read_text(errors="ignore")
        for i, line in enumerate(text.splitlines(), 1):
            if allowed(line):
                continue
            for pat in FORBIDDEN:
                if pat.search(line):
                    violations.append({
                        "file": str(f.relative_to(ROOT)),
                        "line": i,
                        "pattern": pat.pattern,
                        "content": line.strip()[:200]
                    })

if violations:
    print("TYPOGRAPHY VALIDATION: FAIL")
    print(json.dumps({"violations": violations}, indent=2))
    sys.exit(1)

print("TYPOGRAPHY VALIDATION: PASS")
print("No unauthorized typography patterns found.")
