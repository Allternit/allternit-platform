# Planning

Use this before a non-trivial change: one that touches several files, has more than one reasonable design, or would be costly to redo. You plan in your own context so the plan, and everything you read to make it, stays with you for the build. (This replaces the old `planner` subagent.)

1. Read the code the change touches first: the entry points, the callers, and one existing example of the same pattern. Read only the parts you need.
2. Find the smallest correct change that fits the existing patterns. Reuse helpers that already exist before writing new ones.
3. Write the plan as a numbered list. For each step: what changes, which files, and how you will check it.
4. Name the options you rejected and why, in one line each.
5. List the risks: callers that could break, data migrations, other surfaces (desktop, web phone layout, PWA) and docs that must change with it.
6. Stop and ask the user when a step needs a decision only they can make: money, deploys, deletes, product direction. Don't guess.
7. Then build it. Revise the plan when results show it is wrong. Don't keep following a stale plan.

Use a subagent only for the parts of planning that are parallel searches (for example, `explorer` on two unrelated areas at once). The plan itself stays with you.
