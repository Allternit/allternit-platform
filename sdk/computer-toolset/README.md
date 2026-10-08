# @allternit/computer-toolset

Typed bindings for the Allternit computer and browser toolset contract
(`allternit.computer.v1`, `allternit.browser.v1`). Member names and input
fields match Anthropic's `computer_toolset_20260801` (17 members) and
`browser_toolset_20260801` (31 members), so a Claude tool call maps onto the
contract with no translation.

`src/generated.ts` is generated. Edit `contracts/computer-toolset/*.json`, then:

```bash
node contracts/computer-toolset/generate.mjs          # write
node contracts/computer-toolset/generate.mjs --check  # CI / pre-commit drift check
```

The executor is `POST /api/v1/computers/:id/toolset` in allternit-api. See
`surfaces/docs/guides/computer-toolset.mdx`.
