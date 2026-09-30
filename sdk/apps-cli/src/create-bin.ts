#!/usr/bin/env node
import { run } from "./cli.js";
import { nodeContext } from "./context.js";

process.exit(await run(nodeContext(), ["create", ...process.argv.slice(2)]));
