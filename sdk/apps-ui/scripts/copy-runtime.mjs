// Ship the dependency-free runtime script next to the compiled output.
import { copyFileSync, mkdirSync } from "node:fs";
mkdirSync("dist", { recursive: true });
copyFileSync("runtime/allternit-ui.js", "dist/allternit-ui.js");
