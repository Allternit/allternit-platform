# __APP_TITLE__

An Allternit Mini app built on the Apps SDK: a server (MCP tools), a View (a screen in chat) and a look pack (design tokens).

```bash
npm install
npx allternit dev        # run the server locally and print how to connect it
npx allternit test       # annotation, description and CSP checks, plus a live MCP round trip
npx allternit package --url https://<your-host>/mcp   # build the plugin zip
```

`server.ts` is the whole app. See the SDK docs: https://docs.allternit.com/plugins/sdk/overview
