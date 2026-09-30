import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { CliError, type Context } from "../context.js";

const here = dirname(fileURLToPath(import.meta.url));

function templateDir(): string {
  for (const dir of [join(here, "..", "..", "templates", "app"), join(here, "..", "templates", "app")]) {
    if (existsSync(dir)) return dir;
  }
  throw new CliError("Template not found (broken install).");
}

export const slugify = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");

function copyTemplate(from: string, to: string, vars: Record<string, string>): string[] {
  const written: string[] = [];
  mkdirSync(to, { recursive: true });
  for (const name of readdirSync(from)) {
    const src = join(from, name);
    // npm drops .gitignore from packages, so the template stores it as `gitignore`.
    const dest = join(to, name === "gitignore" ? ".gitignore" : name.replace(/\.tpl$/, ""));
    if (statSync(src).isDirectory()) {
      written.push(...copyTemplate(src, dest, vars));
      continue;
    }
    let text = readFileSync(src, "utf8");
    for (const [k, v] of Object.entries(vars)) text = text.split(`__${k}__`).join(v);
    writeFileSync(dest, text);
    written.push(dest);
  }
  return written;
}

/** `create-allternit-app <name>`: scaffold the weather app rebuilt on the SDK. */
export function create(ctx: Context, name: string | undefined): string {
  if (!name) throw new CliError("Usage: create-allternit-app <name>");
  const slug = slugify(basename(name));
  if (!slug) throw new CliError(`"${name}" is not a usable app name.`);
  const target = resolve(ctx.cwd, name);
  if (existsSync(target) && readdirSync(target).length > 0) throw new CliError(`${target} already exists and is not empty.`);
  const files = copyTemplate(templateDir(), target, { APP_NAME: slug, APP_TITLE: slug.slice(0, 30) });
  ctx.log(`Created ${slug} (${files.length} files) in ${target}\n`);
  ctx.log(`  cd ${name}\n  npm install\n  npx allternit dev\n`);
  return target;
}


