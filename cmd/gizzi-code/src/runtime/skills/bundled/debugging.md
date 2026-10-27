# Debugging

Use this for a bug, an error, a failing test or a strange log. You debug in your own context so the evidence and the fix stay together. (This replaces the old `debugger` subagent.)

1. Start from the evidence: the exact error text, the stack trace, the failing command and its output. Reproduce it once if you can do that cheaply.
2. Follow the stack trace or log lines to the code paths they name. Read those paths, not the whole module.
3. Write down two or three hypotheses. Check each against the code and the evidence. Drop the ones the evidence rules out.
4. Find the root cause (file:line) and why it happens. A symptom you can suppress is not a root cause.
5. Make the smallest fix for the root cause. Look for the same defect in sibling code (other surfaces, other callers) and fix it there too.
6. Prove the fix with the narrowest check: the failing test, the repro command, or the log line that should now change.
7. Say how sure you are. If you could not reproduce it, say so.

When logs or traces are very large, have `explorer` find the relevant lines and return only those excerpts. Do the reasoning yourself.
