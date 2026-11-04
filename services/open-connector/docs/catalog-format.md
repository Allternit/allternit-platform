# Catalog Format

Provider definitions in `src/providers/<service>/definition.ts` are the source of truth.
Catalog JSON in `catalog/apps` is generated local runtime data and used by the server at startup.
Generated registry and catalog files are ignored by git. `npm install`, `npm run dev`, and
`npm start` create them when they are missing or stale.

Provider executors live in `src/providers/<service>/executors.ts` and are loaded only when an action is executed.

Do not hand-edit generated catalog files as source. Update provider definitions and run:

```bash
npm run generate:catalog
```

At runtime, catalog responses add execution status that is not stored in generated catalog JSON:

- `locallyExecutable`: the open-source runtime has a local executor for the action.
- `catalogOnly`: schemas and metadata are available, but no local executor is wired yet.
- `needsCredential`: the provider needs a configured local connection before execution.
- `noAuthRunnable`: the action belongs to a provider that can run without stored credentials.

Action definitions also declare provider-native `requiredScopes` and `providerPermissions`. The
runtime exposes those fields through HTTP and MCP discovery together with the current connection
profile, so agents can see both the capability they are about to use and the account it will run as.

Some actions take an input that the connection already holds, such as a workspace slug, account id,
or developer token entered at connect time or recorded by credential validation. Providers wrap their
actions with `withSavedConnectionInputs` (`src/core/provider-definition.ts`): the input stays in the
schema but is no longer required, and the action records `savedConnectionInputs`, a map from the
input key to the credential value or metadata fields that hold it. When a caller omits the input, the
runtime fills it from the selected connection before running the executor; an explicit input always
wins. If the input is neither passed nor saved, the run fails with `invalid_input`.

For the full contribution workflow, see `.codex/skills/add-provider/SKILL.md`.
