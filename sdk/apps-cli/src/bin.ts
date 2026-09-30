#!/usr/bin/env node
import { run } from "./cli.js";
import { nodeContext } from "./context.js";

const code = await run(nodeContext(), process.argv.slice(2));
// `dev` leaves the HTTP server running; only exit early on failure or a finished command.
if (code !== 0 || process.argv[2] !== "dev") process.exit(code);
