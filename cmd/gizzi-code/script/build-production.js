#!/usr/bin/env bun
/**
 * Production Build Script for Gizzi Code
 *
 * Cross-platform build pipeline supporting:
 * - macOS (arm64, x64)
 * - Linux (arm64, x64)
 * - Windows (x64)
 *
 * Usage:
 *   bun run build:production              # Build for current platform
 *   bun run build:production --all        # Build for all platforms
 *   bun run build:production --target=darwin-x64  # Build for specific target
 *   bun run build:production --bundle-only   # Stop after the JS bundle (no compile)
 *
 * Compile-time feature() flags come from script/features.mjs
 * (GIZZI_FEATURES_EXTRA=FOO,BAR adds more for an experiment).
 *
 * Two-step approach:
 * 1. Bundle with plugin to single JS file
 * 2. Compile with `bun build --compile --target=$target`
 */
import { $ } from "bun";
import { createHash } from "crypto";
import { mkdir, rename, copyFile, readdir } from "fs/promises";
import { existsSync, renameSync } from "fs";
import { devNull } from "os";
import { dirname, resolve } from "path";
import { copyNativeAssets } from "./native-assets.mjs";
import { patchEsmAsyncWrappers } from "./patch-esm-async.js";
import { resolveFeatures } from "./features.mjs";
const TARGETS = [
    { platform: "darwin", arch: "arm64", suffix: "", target: "bun-darwin-arm64" },
    { platform: "darwin", arch: "x64", suffix: "", target: "bun-darwin-x64" },
    { platform: "linux", arch: "arm64", suffix: "", target: "bun-linux-arm64" },
    { platform: "linux", arch: "x64", suffix: "", target: "bun-linux-x64" },
    { platform: "win32", arch: "x64", suffix: ".exe", target: "bun-windows-x64" },
];
// Parse CLI arguments
const args = {
    all: process.argv.includes("--all"),
    target: process.argv.find((a) => a.startsWith("--target="))?.split("=")[1],
    outfile: process.argv.find((a) => a.startsWith("--outfile="))?.split("=")[1],
    bundleOnly: process.argv.includes("--bundle-only"),
};
// Determine which targets to build
function getTargetsToBuild() {
    if (args.all) {
        return TARGETS;
    }
    if (args.target) {
        const target = TARGETS.find((t) => t.target === args.target || `${t.platform}-${t.arch}` === args.target);
        if (!target) {
            console.error(`Unknown target: ${args.target}`);
            console.error(`Available targets: ${TARGETS.map((t) => `${t.platform}-${t.arch}`).join(", ")}`);
            process.exit(1);
        }
        return [target];
    }
    // Default: current platform only
    const currentPlatform = process.platform;
    const currentArch = process.arch === "arm64" ? "arm64" : "x64";
    const target = TARGETS.find((t) => t.platform === currentPlatform && t.arch === currentArch);
    return target ? [target] : [TARGETS[0]];
}
const targetsToBuild = getTargetsToBuild();
const OUTDIR = "./dist";
const BUNDLE_FILE = "./.build/gizzi-code-bundle.js";
const BINARY_NAME = "gizzi-code";
console.log("╔══════════════════════════════════════════════════════════╗");
console.log("║        Gizzi Code - Production Build Pipeline            ║");
console.log("╚══════════════════════════════════════════════════════════╝");
console.log("");
console.log(`Targets: ${targetsToBuild.map((t) => `${t.platform}-${t.arch}`).join(", ")}`);
console.log("");
// Read version from package.json
const packageJson = await Bun.file("./package.json").json();
const VERSION = packageJson.version || "1.0.0";
console.log(`Version: ${VERSION}`);
console.log("");
// Load and bundle migrations
console.log("📦 Loading migrations...");
const migrations = [];
const migrationDir = "./migration";
function parseTime(tag) {
    const match = /^(\d{4})(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})/.exec(tag);
    if (!match)
        return 0;
    return Date.UTC(Number(match[1]), Number(match[2]) - 1, Number(match[3]), Number(match[4]), Number(match[5]), Number(match[6]));
}
try {
    const entries = existsSync(migrationDir) ? await readdir(migrationDir) : [];
    for (const dir of entries) {
        const migrationPath = `${migrationDir}/${dir}/migration.sql`;
        const file = Bun.file(migrationPath);
        if (await file.exists()) {
            const sql = await file.text();
            const timestamp = parseTime(dir);
            if (timestamp > 0) {
                const hash = createHash("sha256").update(sql).digest("hex");
                migrations.push({ sql, timestamp, hash });
            }
        }
    }
    migrations.sort((a, b) => a.timestamp - b.timestamp);
    console.log(`   ✓ Loaded ${migrations.length} migrations`);
}
catch (e) {
    console.log("   ℹ No migrations found");
}
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
// Embed .md and .txt files as string constants at bundle time
const textEmbedPlugin = {
    name: "text-embed",
    setup(build) {
        build.onLoad({ filter: /\.(md|txt)$/ }, async (args) => {
            try {
                const text = await Bun.file(args.path).text();
                return {
                    contents: `export default ${JSON.stringify(text)};`,
                    loader: "js",
                };
            } catch {
                return {
                    contents: `export default "";`,
                    loader: "js",
                };
            }
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
            } else if (head === "memdir") {
                // src/shared/memdir/* were empty auto-generated shims that
                // shadowed the real src/memdir/* implementations via the
                // fallback below (wrong memory paths in the binary; broke the
                // build once getAutoMemPathFor was imported). Pin memdir to
                // the real tree.
                candidates = [resolve("src/memdir", rel)];
            } else if (head === "shared") {
                candidates = [resolve("src/shared", rel)];
            } else if (head === "runtime") {
                candidates = [resolve("src/runtime", rel)];
            } else if (head === "cli") {
                candidates = [resolve("src/cli", rel)];
            } else if (head === "ink") {
                candidates = [
                    resolve("src/cli/ui/ink-app/ink", rel),
                ];
            } else if (head === "utils") {
                candidates = [
                    resolve("src/shared/utils", rel),
                    resolve("src/cli/ui/ink-app/utils", rel),
                    resolve("src/runtime/utils", rel),
                    resolve("src/utils", rel),
                ];
            } else {
                candidates = [
                    resolve("src/shared", relativePath),
                    resolve("src", relativePath),
                    resolve("src/runtime", relativePath),
                    resolve("src/cli/ui/ink-app", relativePath),
                ];
            }
            const found = probe(candidates);
            return { path: found ?? candidates[0] };
        });
        build.onResolve({ filter: /\.(md|txt)$/ }, (args) => {
            const abs = resolve(dirname(args.importer || process.cwd()), args.path);
            return { path: abs, namespace: "text-embed" };
        });
        build.onLoad({ filter: /.*/, namespace: "text-embed" }, async (args) => {
            try {
                const text = await Bun.file(args.path).text();
                return {
                    contents: `export default ${JSON.stringify(text)};`,
                    loader: "js",
                };
            } catch {
                return {
                    contents: `export default "";`,
                    loader: "js",
                };
            }
        });
        build.onResolve({ filter: /^\.\.?\/.*\.js$/ }, (args) => {
            const abs = resolve(dirname(args.importer || process.cwd()), args.path);
            if (Bun.file(abs).size > 0) return { path: abs };
            const noExt = abs.replace(/\.js$/, "");
            for (const ext of [".tsx", ".ts", "/index.tsx", "/index.ts"]) {
                const candidate = noExt + ext;
                if (Bun.file(candidate).size > 0) return { path: candidate };
            }
            return { path: abs, namespace: "empty-stub" };
        });
        build.onLoad({ filter: /.*/, namespace: "empty-stub" }, () => ({
            contents: `export default {}; export const SnapshotUpdateDialog = () => null;`,
            loader: "js",
        }));

        // MCP SDK v2 (@modelcontextprotocol/client|server): the bundle is built with the
        // "browser" condition, which would pick the SDK's browser shims (CORS-aware fetch
        // errors, cfworker JSON-schema validator). gizzi runs on Bun, so pin the Node shims.
        build.onResolve({ filter: /^@modelcontextprotocol\/(client|server)\/_shims$/ }, (args) => ({
            path: resolve(`node_modules/@modelcontextprotocol/${args.path.split("/")[1]}/dist/shimsNode.mjs`),
        }));
        build.onResolve({ filter: /^@allternit\/orchestrator$/ }, () => ({
            path: resolve("../../platform/packages/orchestrator/src/index.ts"),
        }));
        // OTel metrics/trace/exporters are optional; not all are in
        // package.json. Inline a no-op module so Bun.build does not fail.
        const otelNs = "optional-otel";
        build.onResolve({
            filter: /^@opentelemetry\/(sdk-metrics|sdk-trace-base|exporter-.*)$/,
        }, () => ({ path: otelNs, namespace: otelNs }));
        build.onLoad({ filter: /.*/, namespace: otelNs }, () => ({
            contents: [
                "const Dummy = function Dummy() {};",
                "Dummy.prototype = Dummy;",
                "const dummy = new Proxy(Dummy, { get: () => Dummy, construct: () => Dummy, apply: () => Dummy });",
                "export default dummy;",
                "export const MeterProvider = dummy;",
                "export const PeriodicExportingMetricReader = dummy;",
                "export const ConsoleMetricExporter = dummy;",
                "export const BasicTracerProvider = dummy;",
                "export const BatchSpanProcessor = dummy;",
                "export const SimpleSpanProcessor = dummy;",
                "export const ConsoleSpanExporter = dummy;",
                "export const AlwaysOnSampler = dummy;",
                "export const PrometheusExporter = dummy;",
                "export const OTLPMetricExporter = dummy;",
                "export const OTLPTraceExporter = dummy;",
                "export const OTLPLogExporter = dummy;",
                "export const AggregationTemporality = dummy;",
                "export const InMemorySpanExporter = dummy;",
                "export const NodeTracerProvider = dummy;",
                "export const Resource = dummy;",
                "export const OTLPGrpcTraceExporter = dummy;",
                "export const OTLPHttpTraceExporter = dummy;",
                "export const OTLPProtoTraceExporter = dummy;",
                "export const OTLPGrpcMetricExporter = dummy;",
                "export const OTLPHttpMetricExporter = dummy;",
                "export const OTLPProtoMetricExporter = dummy;",
                "export const OTLPGrpcLogExporter = dummy;",
                "export const OTLPHttpLogExporter = dummy;",
                "export const OTLPProtoLogExporter = dummy;",
            ].join("\n"),
            loader: "js",
        }));
        // @allternit/gizzi-sdk lives at packages/sdk (not packages/gizzi-sdk).
        // Without this alias Bun.build cannot resolve provider subpaths and
        // the production binary ships without Bot Mode / provider adapters.
        build.onResolve({ filter: /^@allternit\/gizzi-sdk/ }, (args) => {
            const rest = args.path.replace(/^@allternit\/gizzi-sdk\/?/, "");
            const pkgDir = resolve("packages/sdk");
            if (!rest) {
                return { path: resolve(pkgDir, "dist/index.js") };
            }
            const jsPath = resolve(pkgDir, "dist", rest + ".js");
            if (Bun.file(jsPath).size > 0) return { path: jsPath };
            const indexPath = resolve(pkgDir, "dist", rest, "index.js");
            if (Bun.file(indexPath).size > 0) return { path: indexPath };
            return { path: jsPath };
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
        // Let Bun handle .tsx natively with React JSX
        /* build.onLoad({ filter: /\.tsx$/ }, async (args) => { ... }); */
    },
};
console.log("");
console.log("🔨 Step 1: Bundling...");
// Ensure build directory exists
await mkdir("./.build", { recursive: true });
// Temporarily move bunfig.toml for the bundle step too
const BUNFIG_BACKUP = "./.build/bunfig.toml.bak";
const BUNFIG_ORIG = "./bunfig.toml";
let bunfigWasMoved = false;
if (await Bun.file(BUNFIG_ORIG).exists()) {
    await rename(BUNFIG_ORIG, BUNFIG_BACKUP);
    bunfigWasMoved = true;
}
// A failed or interrupted build must not leave bunfig.toml moved aside (it
// shows up as a deleted tracked file and breaks `bun test` aliases). The exit
// handler covers every process.exit() path; signals route through it.
process.on("exit", () => {
    if (bunfigWasMoved && existsSync(BUNFIG_BACKUP)) {
        renameSync(BUNFIG_BACKUP, BUNFIG_ORIG);
    }
});
for (const sig of ["SIGINT", "SIGTERM", "SIGHUP"]) {
    process.on(sig, () => process.exit(130));
}
// Compile-time `feature()` flags (see script/features.mjs).
const FEATURES = resolveFeatures();
console.log(`   Features (${FEATURES.length}): ${FEATURES.join(", ")}`);
// Shared defines for the bundler
const define = {
    "process.env.NODE_ENV": '"production"',
};
// Global code to inject at the top of each bundle
// PACKAGE_URL must be an npm package spec: the autoUpdater/localInstaller/doctor
// consumers run "npm view/install <PACKAGE_URL>". The machine-readable install
// manifest for the website lives at https://install.gizziio.com/version.json.
const GIZZI_CHANNEL = process.env.GIZZI_CHANNEL || "production";
// Source commit, embedded verbatim so scripts/build-state.sh can tell which
// commit any copy of the binary (dist/, desktop resources/bin, Homebrew) was
// built from by grepping for `GIZZI_BUILD_SHA = "<sha>"`. "-dirty" marks a
// build from uncommitted changes.
const BUILD_SHA = await (async () => {
    try {
        const sha = (await $`git rev-parse HEAD`.quiet().text()).trim();
        // Tracked source only: CI's install/SDK steps leave generated and
        // untracked files behind that don't change what the binary contains.
        const dirty = (await $`git status --porcelain --untracked-files=no -- src package.json`.quiet().text()).trim() !== "";
        return dirty ? `${sha}-dirty` : sha;
    } catch {
        return process.env.GITHUB_SHA || "unknown";
    }
})();
let injectionCode = `
var GIZZI_BUILD_SHA = "${BUILD_SHA}";
var GIZZI_VERSION = "${VERSION}";
var GIZZI_CHANNEL = "${GIZZI_CHANNEL}";
var MACRO = ${JSON.stringify({ VERSION, CHANNEL: GIZZI_CHANNEL, PACKAGE_URL: "@allternit/gizzi-code", BUILD_TIME: new Date().toISOString(), BUILD_SHA })};
`;
if (migrations.length > 0) {
    injectionCode += `var GIZZI_MIGRATIONS = ${JSON.stringify(migrations)};\n`;
}
// Step 1: Bundle to single JS file
console.log("🔨 Step 1a: Bundling worker...");
const workerBundleResult = await Bun.build({
    entrypoints: ["./src/cli/ui/ink-app/worker.ts"],
    target: "bun",
    sourcemap: "none",
    minify: { whitespace: true, syntax: false, identifiers: false },
    define,
    features: FEATURES,
    conditions: ["browser"],
    external: ["electron", "chromium-bidi/*", "playwright-core/*"],
    plugins: [wasmEmbedPlugin, textEmbedPlugin, bundlePlugin],
});
if (!workerBundleResult.success) {
    console.error("Worker bundle failed:");
    for (const log of workerBundleResult.logs)
        console.error(log);
    process.exit(1);
}
const workerCode = injectionCode + (await workerBundleResult.outputs[0].text());
console.log(`   ✓ Worker bundled (${Math.round(workerCode.length / 1024)} KB)`);
console.log("🔨 Step 1b: Bundling main application...");
const bundleResult = await Bun.build({
    entrypoints: ["./src/cli/main.ts"],
    target: "bun",
    sourcemap: "none",
    minify: { whitespace: true, syntax: false, identifiers: false },
    define: {
        ...define,
        "GIZZI_WORKER_CODE": JSON.stringify(workerCode),
    },
    features: FEATURES,
    conditions: ["browser"],
    external: ["electron", "chromium-bidi/*", "playwright-core/*"],
    plugins: [wasmEmbedPlugin, textEmbedPlugin, bundlePlugin],
});
if (!bundleResult.success) {
    console.error("Bundle failed:");
    for (const log of bundleResult.logs) {
        console.error(log);
    }
    // Restore bunfig.toml before exiting
    if (bunfigWasMoved && await Bun.file(BUNFIG_BACKUP).exists()) {
        await rename(BUNFIG_BACKUP, BUNFIG_ORIG);
    }
    process.exit(1);
}
// Write bundle output to file with embedded migrations and version
let bundleCode = injectionCode + (await bundleResult.outputs[0].text());
console.log(`   ✓ Embedded version: ${VERSION}`);
if (migrations.length > 0) {
    console.log(`   ✓ Embedded ${migrations.length} migrations into bundle`);
}
await Bun.write(BUNDLE_FILE, bundleCode);
console.log(`   ✓ Bundle written: ${BUNDLE_FILE} (${Math.round(bundleCode.length / 1024)} KB)`);
// The MCP SDK v2 client must be inlined (npm/brew ship only the compiled binary; there is
// no node_modules next to it). "EraNegotiationFailed" is an SDK v2 error code literal
// (dual-era negotiation) that gizzi's own source never spells.
if (!bundleCode.includes("EraNegotiationFailed")) {
    console.error("✗ MCP SDK v2 is not inlined in the bundle (dual-era negotiation code missing)");
    process.exit(1);
}
console.log("   ✓ MCP SDK v2 inlined (2026-07-28 + legacy fallback)");
const { code: patchedBundleCode, patched } = patchEsmAsyncWrappers(bundleCode);
if (patched > 0) {
    await Bun.write(BUNDLE_FILE, patchedBundleCode);
    console.log(`   ✓ Patched ${patched} async ESM wrappers (Bun circular-dep workaround)`);
}
if (args.bundleOnly) {
    if (bunfigWasMoved && await Bun.file(BUNFIG_BACKUP).exists()) {
        await rename(BUNFIG_BACKUP, BUNFIG_ORIG);
    }
    process.exit(0);
}
console.log("");
console.log("🔨 Step 2: Compiling binaries...");
// Ensure dist directory exists
await mkdir(OUTDIR, { recursive: true });
// Build each target
const results = [];
for (const target of targetsToBuild) {
    const outfile = args.outfile || `${OUTDIR}/${BINARY_NAME}-${target.platform}-${target.arch}${target.suffix}`;
    process.stdout.write(`   Building ${target.platform}-${target.arch}... `);
    try {
        // Cross-platform minimal-env compile (no `env -i`, which is unix-only)
        const proc = Bun.spawnSync({
            cmd: [process.execPath, "build", "--compile", BUNDLE_FILE, "--outfile", outfile, "--target", target.target],
            env: { PATH: process.env.PATH, HOME: process.env.HOME, BUNFIG_PATH: devNull },
            stdout: "inherit",
            stderr: "inherit",
        });
        if (proc.exitCode !== 0)
            throw new Error(`bun build failed with exit code ${proc.exitCode}`);
        // Get file size
        const stat = await Bun.file(outfile).stat();
        const sizeMB = stat ? Math.round((stat.size / 1024 / 1024) * 10) / 10 : 0;
        console.log(`✓ (${sizeMB} MB) -> ${outfile}`);
        if (process.env.GIZZI_DISABLE_NATIVE_SIDECAR !== "1") {
            const assets = await copyNativeAssets({
                outDir: OUTDIR,
                target: `${target.platform}-${target.arch}`,
            });
            console.log(`   ✓ ${assets.packageName} sidecar (${assets.files} files)`);
        }
        results.push({ target, success: true, path: outfile, size: sizeMB });
    }
    catch (err) {
        const errorMsg = err?.message || String(err);
        console.log(`✗ ${errorMsg}`);
        results.push({ target, success: false, path: outfile, error: errorMsg });
    }
}
// Restore bunfig.toml
if (bunfigWasMoved && await Bun.file(BUNFIG_BACKUP).exists()) {
    await rename(BUNFIG_BACKUP, BUNFIG_ORIG);
}
console.log("");
console.log("╔══════════════════════════════════════════════════════════╗");
console.log("║                    Build Summary                         ║");
console.log("╚══════════════════════════════════════════════════════════╝");
console.log("");
const successful = results.filter((r) => r.success);
const failed = results.filter((r) => !r.success);
if (successful.length > 0) {
    console.log("✓ Successful builds:");
    for (const r of successful) {
        console.log(`  ${r.target.platform}-${r.target.arch}: ${r.path} (${r.size} MB)`);
    }
}
if (failed.length > 0) {
    console.log("");
    console.log("✗ Failed builds:");
    for (const r of failed) {
        console.log(`  ${r.target.platform}-${r.target.arch}: ${r.error}`);
    }
}
console.log("");
console.log(`Total: ${successful.length} successful, ${failed.length} failed`);
console.log("");
// Create a simple named symlink/copy for the current platform
if (successful.length === 1) {
    const simpleName = `${OUTDIR}/${BINARY_NAME}${successful[0].target.suffix}`;
    const targetName = `${BINARY_NAME}-${successful[0].target.platform}-${successful[0].target.arch}${successful[0].target.suffix}`;
    try {
        // Symlink for the current platform; fall back to a copy (Windows
        // symlink creation often needs elevated privileges).
        const { symlink } = await import("fs/promises");
        await symlink(targetName, simpleName).catch(() => copyFile(successful[0].path, simpleName));
    }
    catch {
        // Ignore symlink errors
    }
    console.log("");
    console.log("To run the binary:");
    console.log(`  ${simpleName} --help`);
}

// Vendor allternit-mux and ripgrep next to the built binaries (gizzi-code
// layout): dist/vendor/{allternit-mux,ripgrep}/... so the runtime resolves
// them from the executable's directory.
for (const r of successful) {
    try {
        const pa = `${r.target.platform}-${r.target.arch}`;
        const ap = `${r.target.arch}-${r.target.platform}`;
        const suffix = r.target.suffix || "";
        const vendoredMux = `vendor/allternit-mux/${pa}/allternit-mux${suffix}`;
        if (await Bun.file(vendoredMux).exists()) {
            const dest = `${OUTDIR}/vendor/allternit-mux/${pa}`;
            await mkdir(dest, { recursive: true });
            await copyFile(vendoredMux, `${dest}/allternit-mux${suffix}`);
            console.log(`✓ vendored allternit-mux -> ${dest}/allternit-mux${suffix}`);
        }
        else {
            console.log(`ℹ no vendored allternit-mux for ${pa} (run script/vendor-mux.sh)`);
        }
        const vendoredRg = `vendor/ripgrep/${ap}/rg${suffix}`;
        if (await Bun.file(vendoredRg).exists()) {
            const dest = `${OUTDIR}/vendor/ripgrep/${ap}`;
            await mkdir(dest, { recursive: true });
            await copyFile(vendoredRg, `${dest}/rg${suffix}`);
            console.log(`✓ vendored ripgrep -> ${dest}/rg${suffix}`);
        }
        else {
            console.log(`ℹ no vendored ripgrep for ${ap} (run script/vendor-ripgrep.sh)`);
        }
    }
    catch (e) {
        console.log(`⚠ vendoring skipped: ${e?.message || e}`);
    }
}
console.log("");
console.log("To run in dev mode:");
console.log("  bun run dev");
console.log("");
// Exit with error if any builds failed
if (failed.length > 0) {
    process.exit(1);
}
