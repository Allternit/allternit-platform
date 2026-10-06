// Standalone reproduction of the production worker bundle (RAM-light).
import { resolve } from "path";
// Some source files were pre-processed with React Compiler and contain imports from
// "react/compiler-runtime". The build pipeline does not run React Compiler, so we stub
// the runtime to keep those files bundlable. This disables compiler memoization but
// preserves behaviour.
const REACT_COMPILER_RUNTIME_NS = "react-compiler-runtime-stub";
const REACT_COMPILER_RUNTIME_STUB = `
export function c(size) {
  return (fn) => fn();
}
export function useMemoCache(size) {
  return [];
}
`;
// Embed WASM files as Uint8Array constants at bundle time so they work
// in compiled bun binaries (where /$bunfs/ paths are not fs-readable).
const wasmEmbedPlugin = {
    name: "wasm-embed",
    setup(build) {
        build.onLoad({ filter: /\.wasm$/ }, async (args) => {
            const bytes = await Bun.file(args.path).bytes();
            // Encode as base64 and decode at runtime — avoids any file I/O at runtime.
            const b64 = Buffer.from(bytes).toString("base64");
            return {
                contents: `const b = Buffer.from("${b64}", "base64"); export default new Uint8Array(b.buffer, b.byteOffset, b.byteLength);`,
                loader: "js",
            };
        });
    },
};
// Create the bundle plugin
const bundlePlugin = {
    name: "bundle-plugin",
    setup(build) {
        // Resolve @/ and @tui/ aliases manually for the transform.
        // Mirrors the tsconfig path rules: specific subtrees point at the tree
        // that actually contains the modules (ink-app has the complete trees),
        // with existence-checked candidates in priority order.
        const probe = (candidates) => {
            for (const base of candidates) {
                for (const ext of [".ts", ".tsx", "/index.ts", "/index.tsx"]) {
                    const path = base + ext;
                    if (Bun.file(path).size > 0) return path;
                }
            }
            return null;
        };
        const subtreeBases = (subtree, rel) => [
            resolve("src/cli/ui/ink-app", subtree, rel),
            resolve("src/runtime", subtree, rel),
            resolve("src", subtree, rel),
        ];
        build.onResolve({ filter: /^(@\/|@tui\/)/ }, (args) => {
            const isTui = args.path.startsWith("@tui/");
            let relativePath = args.path.substring(isTui ? 5 : 2);
            if (isTui) {
                relativePath = "cli/ui/tui/" + relativePath;
            }
            relativePath = relativePath.replace(/\.(js|jsx|ts|tsx)$/, "");
            const [head, ...rest] = relativePath.split("/");
            const rel = rest.join("/");
            let candidates;
            if (isTui) {
                candidates = [resolve("src", relativePath)];
            } else if (["services", "state", "hooks", "commands"].includes(head)) {
                candidates = subtreeBases(head, rel);
            } else if (head === "shared") {
                candidates = [resolve("src/shared", rel)];
            } else if (head === "runtime") {
                candidates = [resolve("src/runtime", rel)];
            } else if (head === "cli") {
                candidates = [resolve("src/cli", rel)];
            } else {
                candidates = [
                    resolve("src", relativePath),
                    resolve("src/runtime", relativePath),
                    resolve("src/cli/ui/ink-app", relativePath),
                ];
            }
            const found = probe(candidates);
            return { path: found ?? candidates[0] };
        });
        // Resolve @allternit workspace packages to their source or dist
        build.onResolve({ filter: /^@allternit\/(plugin|script|sdk|util|gizzi-util)/ }, (args) => {
            const parts = args.path.split("/");
            const name = parts[1];
            const subpath = parts.slice(2).join("/");
            const pkgDir = resolve("packages", name);
            if (subpath) {
                // 1. Try direct .js file
                const jsPath = resolve(pkgDir, "dist", subpath + ".js");
                if (Bun.file(jsPath).size > 0) return { path: jsPath };
                
                // 2. Try index.js if subpath is a directory
                const indexPath = resolve(pkgDir, "dist", subpath, "index.js");
                if (Bun.file(indexPath).size > 0) return { path: indexPath };

                // 3. Fallback for subpaths that might already include .js but failed direct check
                return { path: resolve(pkgDir, "dist", subpath) };
            }
            return { path: resolve(pkgDir, "dist/index.js") };
        });
        // Redirect react/compiler-runtime to a no-op stub.
        build.onResolve({ filter: /^react\/compiler-runtime$/ }, () => ({
            path: REACT_COMPILER_RUNTIME_NS,
            namespace: REACT_COMPILER_RUNTIME_NS,
        }));
        build.onLoad({ filter: /.*/, namespace: REACT_COMPILER_RUNTIME_NS }, () => ({
            contents: REACT_COMPILER_RUNTIME_STUB,
            loader: "js",
        }));
    },
};

const result = await Bun.build({
    entrypoints: ["./src/cli/ui/ink-app/worker.ts"],
    target: "bun",
    sourcemap: "none",
    minify: { whitespace: true, syntax: true, identifiers: false },
    conditions: ["browser"],
    external: ["electron", "chromium-bidi/*", "playwright-core/*"],
    plugins: [wasmEmbedPlugin, bundlePlugin],
});
for (const log of result.logs) {
  if (log.message) console.error("MSG:", log.message);
  if (log.position) console.error("  at", log.position.file + ":" + log.position.line + ":" + log.position.column);
  if (log.text) console.error("TEXT:", log.text);
}
console.error("raw:", JSON.stringify(result.logs.slice(0,3), null, 1));
console.log("SUCCESS:", result.success);
