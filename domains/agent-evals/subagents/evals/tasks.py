"""The 10 golden end-to-end tasks.

Each task runs on a fresh copy of fixture.FILES. `setup` edits the copy before
the run; `hidden` files are written after the run, just before the checks, so
the agent can't fit its change to them. Checks are code-only:
  tests      `python3 -m unittest discover -s tests` exits 0 (only: "hidden"
             runs just the hidden files, for tasks that leave the fixture bug)
  contains   {file, all: [...], any: [...]} (case-insensitive)
  absent     {file, none: [...]} (case-insensitive)
  unchanged  {files: [...]} still match the fixture
  exists     {file}
`skill` names the folded skill (or kept subagent) the task exercises.
"""

TRANSCRIPT = """# Sync call, 2026-10-20

Mara: We move the store from items.json to SQLite. Decided.
Theo: Then someone has to migrate items.json. I'll take that, inventory/store.py first.
Mara: The --csv export flag is still open; nobody owns it yet.
Theo: Currency rounding is undecided: bankers' rounding or half-up. Mara decides by 2026-10-24.
Mara: Also, the shipping zones table lives in inventory/shipping.py now.
(Twenty minutes of chat about lunch omitted.)
"""

REVIEW_DIFF = """--- a/inventory/pricing.py
+++ b/inventory/pricing.py
@@ -8,3 +8,8 @@ def order_total(items):

 def average_price(items):
     return sum(i["price"] for i in items) / len(items)
+
+
+def average_qty(items):
+    from .store import total_quantity
+    return total_quantity(items) / len(items)
"""

APP_LOG = """2026-10-26 09:14:02 INFO  low_stock check started path=items.json threshold=3
2026-10-26 09:14:02 ERROR unhandled exception
Traceback (most recent call last):
  File "inventory/jobs.py", line 12, in nightly
    skus = low_stock("items.json", 3)
  File "inventory/report.py", line 12, in low_stock
    return [i["sku"] for i in items if i["qty"] < threshold]
  File "inventory/report.py", line 12, in <listcomp>
    return [i["sku"] for i in items if i["qty"] < threshold]
KeyError: 'qty'
2026-10-26 09:14:02 INFO  items.json has 1 item without a qty field (new items have no stock yet)
"""

SRC = ["inventory/store.py", "inventory/pricing.py", "inventory/report.py", "inventory/cli.py", "inventory/shipping.py"]

TASKS = [
    {
        "id": "debug-failing-test",
        "skill": "debugging",
        "prompt": "A test in tests/ fails. Find the root cause and fix it. Don't change the tests.",
        "hidden": {"tests/test_hidden_store.py": '''import unittest
from inventory.store import total_quantity
class H(unittest.TestCase):
    def test_one(self): self.assertEqual(total_quantity([{"qty": 4}]), 4)
    def test_empty(self): self.assertEqual(total_quantity([]), 0)
'''},
        "checks": [{"type": "tests"}, {"type": "unchanged", "files": ["tests/test_store.py"]}],
    },
    {
        "id": "debug-from-log",
        "skill": "debugging",
        "setup": {"files": {"app.log": APP_LOG}},
        "prompt": "The nightly job crashed. app.log has the trace. Find the cause and fix it so items without a qty count as 0 in stock.",
        "hidden": {"tests/test_hidden_low.py": '''import json, tempfile, unittest
from inventory.report import low_stock
class H(unittest.TestCase):
    def test_missing_qty(self):
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            json.dump([{"sku": "a", "qty": 5, "price": 1}, {"sku": "n", "price": 2}], f)
        self.assertEqual(low_stock(f.name, 3), ["n"])
'''},
        "checks": [{"type": "tests", "only": "hidden"}],
    },
    {
        "id": "plan-only",
        "skill": "planning",
        "prompt": "Plan (don't build) adding a `--csv` flag that writes the summary as CSV. Write the plan to PLAN.md as a numbered list: what changes, which files, how to test it. Don't edit any source file.",
        "checks": [
            {"type": "exists", "file": "PLAN.md"},
            {"type": "contains", "file": "PLAN.md", "all": ["1.", "2.", "inventory/cli.py", "test"]},
            {"type": "unchanged", "files": SRC},
        ],
    },
    {
        "id": "plan-then-build",
        "skill": "planning",
        "prompt": "Add an optional discount: `order_total(items, discount=0.0)` where discount is a fraction from 0 to 1 (reject anything outside that with ValueError), `summary(path, discount=0.0)`, and a `--discount` CLI flag that passes it through. Add tests.",
        "hidden": {"tests/test_hidden_discount.py": '''import io, json, unittest, contextlib
from inventory.pricing import order_total
from inventory.cli import main
ITEMS = [{"sku": "a", "qty": 2, "price": 1.0}, {"sku": "b", "qty": 3, "price": 2.0}]
class H(unittest.TestCase):
    def test_total(self): self.assertAlmostEqual(order_total(ITEMS, discount=0.25), 6.0)
    def test_default(self): self.assertAlmostEqual(order_total(ITEMS), 8.0)
    def test_bad(self):
        with self.assertRaises(ValueError): order_total(ITEMS, discount=1.5)
    def test_cli(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out): main(["items.json", "--json", "--discount", "0.5"])
        self.assertAlmostEqual(json.loads(out.getvalue())["total"], 9.0)
'''},
        "checks": [{"type": "tests", "only": "hidden"}],
    },
    {
        "id": "docs-cli-flag",
        "skill": "docs-writing",
        "prompt": "The CLI has a --json flag that the README doesn't mention. Update README.md so a user can find and use it, with an example. Don't change source code.",
        "checks": [
            {"type": "contains", "file": "README.md", "all": ["--json", "python3 -m inventory"]},
            {"type": "contains", "file": "README.md", "any": ["\"count\"", "count", "json object", "JSON"]},
            {"type": "unchanged", "files": SRC},
        ],
    },
    {
        "id": "summarize-handoff",
        "skill": "summarizing",
        "setup": {"files": {"transcript.md": TRANSCRIPT}},
        "prompt": "Write a handoff in HANDOFF.md from transcript.md: decisions, every open item with its owner (or 'no owner'), and the files involved.",
        "checks": [
            {"type": "contains", "file": "HANDOFF.md", "all": ["sqlite", "items.json", "--csv", "rounding", "theo", "mara", "inventory/store.py", "inventory/shipping.py", "2026-10-24"]},
            {"type": "absent", "file": "HANDOFF.md", "none": ["lunch"]},
        ],
    },
    {
        "id": "search-callers",
        "skill": "explorer",
        "prompt": "List every function in the inventory package that calls load_items. Write one function name per line to CALLERS.txt, nothing else.",
        "checks": [
            {"type": "contains", "file": "CALLERS.txt", "all": ["summary", "low_stock"]},
            {"type": "absent", "file": "CALLERS.txt", "none": ["total_quantity", "order_total", "main"]},
        ],
    },
    {
        "id": "parallel-two-modules",
        "skill": "builder",
        "prompt": "Two independent jobs: (1) implement shipping_cost in inventory/shipping.py exactly as its docstring says; (2) add `tax(amount, rate)` to inventory/pricing.py that returns amount*rate rounded to 2 decimals. Add tests for both.",
        "hidden": {"tests/test_hidden_ship.py": '''import unittest
from inventory.shipping import shipping_cost
from inventory.pricing import tax
class H(unittest.TestCase):
    def test_local(self):
        self.assertEqual(shipping_cost(2, "local"), 5.0)
        self.assertEqual(shipping_cost(2.1, "local"), 6.5)
        self.assertEqual(shipping_cost(4, "local"), 8.0)
    def test_national(self): self.assertEqual(shipping_cost(3.5, "national"), 14.0)
    def test_bad(self):
        with self.assertRaises(ValueError): shipping_cost(1, "moon")
        with self.assertRaises(ValueError): shipping_cost(0, "local")
    def test_tax(self): self.assertEqual(tax(19.99, 0.0825), 1.65)
'''},
        "checks": [{"type": "tests", "only": "hidden"}],
    },
    {
        "id": "review-diff",
        "skill": "code-reviewer",
        "setup": {"files": {"change.diff": REVIEW_DIFF}},
        "prompt": "Review change.diff for bugs. Write your findings to REVIEW.md, most severe first, each with the function name and what breaks. Don't apply the diff.",
        "checks": [
            {"type": "contains", "file": "REVIEW.md", "all": ["average_qty"]},
            {"type": "contains", "file": "REVIEW.md", "any": ["empty", "zero", "ZeroDivisionError"]},
            {"type": "contains", "file": "REVIEW.md", "any": ["total_quantity", "off-by-one", "off by one", "last item"]},
            {"type": "unchanged", "files": SRC},
        ],
    },
    {
        "id": "verify-and-fix",
        "skill": "verifier",
        "setup": {"replace": [{"file": "inventory/pricing.py", "old": 'item["price"] * item["qty"]', "new": 'item["price"] + item["qty"]'}]},
        "prompt": "Run the test suite, find every failure, and fix the code (not the tests) until it passes.",
        "checks": [{"type": "tests"}, {"type": "unchanged", "files": ["tests/test_store.py", "tests/test_pricing.py"]}],
    },
]

# The four types folded into skills on 2026-10-27, as they were as subagents.
# The `split` arm adds them back through config so the evals can compare.
FOLDED_AGENTS = {
    "planner": {
        "description": "Designs an implementation plan for a non-trivial change: steps, critical files, risks and trade-offs. Read-only.",
        "model": "deep",
        "prompt": "You are Planner, a software architect. You read; you do not edit.\nUnderstand the existing code first, then design the smallest correct change that fits its patterns.\nOutput: a numbered step plan, the files each step touches, trade-offs you rejected and why, risks, and how to verify.\nFlag anything that needs a human decision instead of guessing.",
        "permission": {"edit": "deny", "write": "deny", "bash": {"*": "deny", "ls*": "allow", "cat *": "allow", "grep *": "allow", "find *": "allow"}},
    },
    "debugger": {
        "description": "Finds the root cause of a bug, error or failing log from evidence. Read-only; returns cause + suggested fix.",
        "model": "small",
        "prompt": "You are Debugger. Work from evidence: the error, stack trace or logs first, then the code paths they point to.\nForm hypotheses, check each against the code, and discard the ones the evidence rules out.\nReturn the root cause (file:line), why it happens, the minimal fix, and how to confirm it. Say how confident you are.\nNever modify files.",
        "permission": {"edit": "deny", "write": "deny", "bash": {"*": "deny", "ls*": "allow", "cat *": "allow", "grep *": "allow", "python3 -m unittest*": "allow"}},
    },
    "docs-writer": {
        "description": "Writes or updates documentation (README, guides, .md/.mdx). Can only edit docs files.",
        "model": "small",
        "prompt": "You are Docs Writer. Read the code and existing docs first; document what the code actually does.\nPlain, direct language. Match the existing docs' structure, tone and navigation. Keep examples runnable.\nYou may edit only docs/** and *.md / *.mdx files. Never change source code.",
        "permission": {"edit": {"*": "deny", "*.md": "allow"}, "bash": "deny"},
    },
    "scribe": {
        "description": "Summarizes transcripts, logs, diffs or docs into tight summaries, notes and handoffs. Cheap, read-only.",
        "model": "small",
        "prompt": "You are Scribe. Read the material and write the shortest summary that keeps every decision, open item, owner and file path.\nFor handoffs: state, what is done, what is open (all of it), next steps, and where things live.\nNever invent facts that are not in the source.",
        "permission": {"edit": "deny", "write": "deny", "bash": "deny"},
    },
}

SPLIT_HINT = (
    "Delegate search (explorer), tests/typecheck/lint (verifier), log and error digging (debugger), "
    "docs (docs-writer), plans (planner) and summaries (scribe) to these cheap subagents instead of doing them in your own context."
)
