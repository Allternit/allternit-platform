// Ship the dependency-free runtime scripts next to the compiled output.
import { copyFileSync, mkdirSync } from "node:fs";
mkdirSync("dist", { recursive: true });
for (const f of ["allternit-ext.js", "openai-compat.js"]) copyFileSync(`runtime/${f}`, `dist/${f}`);
