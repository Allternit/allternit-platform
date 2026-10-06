//! Offline disposable TS/Vitest repository. No model, download, global git
//! configuration, installed product data, or persistent fixture repository.
use std::{path::{Path, PathBuf}, process::{Command, Output}, time::{Duration, Instant}};
use anyhow::{bail, Context, Result};
use tempfile::TempDir;

pub(super) const SEEDED: &str = "export function add(a: number, b: number): number { return a - b; }\n";
pub(super) const IMPERFECT: &str = "export function add(a: number, b: number): number { return a + b + 1; }\n";
pub(super) const FIXED: &str = "export function add(a: number, b: number): number { return a + b; }\n";

pub(super) struct Fixture {
    pub dir: TempDir,
    pub commands: usize,
}

impl Fixture {
    pub fn new() -> Result<Self> {
        let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.allternit/wp10-fixtures");
        std::fs::create_dir_all(&parent)?;
        let dir = tempfile::Builder::new().prefix("bug-fix-").tempdir_in(parent)?;
        let deps = std::env::var_os("WP10_NODE_MODULES").map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../node_modules"));
        let deps = deps.canonicalize().context("WP10 needs existing root node_modules (pnpm install); never downloads during tests")?;
        for tool in ["vitest/vitest.mjs", "typescript/lib/typescript.js"] {
            if !deps.join(tool).exists() { bail!("WP10 missing offline dependency: {tool}"); }
        }
        #[cfg(unix)] std::os::unix::fs::symlink(&deps, dir.path().join("node_modules"))?;
        #[cfg(windows)] std::os::windows::fs::symlink_dir(&deps, dir.path().join("node_modules"))?;
        let mut me = Self { dir, commands: 0 };
        me.write("src/math.ts", SEEDED)?;
        me.write("src/caller.ts", "import { add } from './math';\nexport const doubledSum = (a: number, b: number) => 2 * add(a, b);\n")?;
        me.write("package.json", r#"{"name":"wp10-disposable-fixture","private":true,"type":"module","devDependencies":{"vitest":"1.6.1","typescript":"^5.0.0"},"scripts":{"test":"vitest run"}}"#)?;
        me.write("tsconfig.json", r#"{"compilerOptions":{"target":"ES2022","module":"ESNext","moduleResolution":"Bundler","strict":true,"noEmit":true,"skipLibCheck":true},"include":["src/**/*.ts"]}"#)?;
        me.write("math.test.ts", "import { test, expect } from 'vitest';\nimport { add } from './src/math';\ntest('acceptance: adds both operands', () => { expect(add(2, 3)).toBe(5); expect(add(-2, 3)).toBe(1); });\n")?;
        me.write("caller.test.ts", "import { test, expect } from 'vitest';\nimport { doubledSum } from './src/caller';\ntest('affected caller', () => { expect(doubledSum(2, 3)).toBe(10); });\n")?;
        me.write("parse.cjs", "const ts = require('typescript'); const fs = require('fs'); const source = ts.createSourceFile('math.ts', fs.readFileSync('src/math.ts','utf8'), ts.ScriptTarget.Latest, true); const diagnostics = source.parseDiagnostics; console.log(JSON.stringify(diagnostics)); process.exit(diagnostics.length ? 1 : 0);\n")?;
        me.write(".gitignore", "node_modules/\n")?;
        me.checked(&["git", "init", "-q"])?;
        me.checked(&["git", "add", "."])?;
        me.checked(&["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "commit", "-qm", "seeded bug"])?;
        Ok(me)
    }
    pub fn path(&self) -> &Path { self.dir.path() }
    pub fn write(&self, path: &str, text: &str) -> Result<()> {
        let path = self.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap())?;
        Ok(std::fs::write(path, text)?)
    }
    /// Kill only this test's child on timeout, and always reap it. The output
    /// goes to temp files so a failed test's diagnostic stream cannot fill a pipe.
    pub fn command(&mut self, args: &[&str]) -> Result<Output> {
        self.commands += 1;
        let stdout = tempfile::tempfile()?;
        let stderr = tempfile::tempfile()?;
        let mut child = Command::new(args[0]).args(&args[1..]).current_dir(self.path())
            .env("CI", "1").env("NO_COLOR", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
            .stdout(stdout.try_clone()?).stderr(stderr.try_clone()?).spawn()?;
        let started = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait()? { break s; }
            if started.elapsed() > Duration::from_secs(30) { let _ = child.kill(); let _ = child.wait(); bail!("fixture command timed out: {args:?}"); }
            std::thread::sleep(Duration::from_millis(10));
        };
        use std::io::{Read, Seek};
        let mut out = stdout; let mut err = stderr;
        out.rewind()?; err.rewind()?;
        let mut a = vec![]; let mut b = vec![]; out.read_to_end(&mut a)?; err.read_to_end(&mut b)?;
        Ok(Output { status, stdout: a, stderr: b })
    }
    pub fn checked(&mut self, args: &[&str]) -> Result<Output> {
        let o = self.command(args)?;
        if !o.status.success() { bail!("fixture command {args:?}: {} {}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)); }
        Ok(o)
    }
    pub fn tests(&mut self, target: Option<&str>) -> Result<Output> {
        let mut args = vec!["node", "node_modules/vitest/vitest.mjs", "run", "--pool=forks", "--poolOptions.forks.singleFork", "--reporter=json"];
        if let Some(target) = target { args.push(target); }
        self.command(&args)
    }
}
