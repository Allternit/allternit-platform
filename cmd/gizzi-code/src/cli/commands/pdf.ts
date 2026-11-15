// `gizzi pdf render <template>`: print-ready PDFs from the shared document
// templates (invoice, report, ticket, minutes, receipt). Renders locally with
// the native PDF writer, so it needs no server and no browser. Used by /quote
// and /client-report to turn a draft into a letterhead document.
import type { Argv } from "yargs"
import { mkdir, readFile, writeFile } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import {
  PDF_TEMPLATES,
  PDF_TEMPLATE_INFO,
  PDF_THEMES,
  PDF_VARIANTS,
  buildDocument,
  renderHtml,
  renderRequestPdf,
  validateRequest,
  type PdfRenderRequest,
  type PdfTemplateId,
  type PdfThemeId,
  type PdfVariant,
} from "@allternit/pdf-templates"
import { cmd } from "@/cli/commands/cmd"

async function readStdin(): Promise<string> {
  const chunks: Buffer[] = []
  for await (const chunk of process.stdin) chunks.push(chunk as Buffer)
  return Buffer.concat(chunks).toString("utf8")
}

export const PdfRenderCommand = cmd({
  command: "render <template>",
  describe: "render a document template to PDF (or HTML with --html)",
  builder: (yargs: Argv) =>
    yargs
      .positional("template", { type: "string", choices: [...PDF_TEMPLATES], describe: "template id" })
      .option("data", { alias: "d", type: "string", describe: "JSON data file ('-' for stdin)" })
      .option("sample", { type: "boolean", describe: "use the template's built-in sample data" })
      .option("theme", { type: "string", choices: [...PDF_THEMES], default: "professional" })
      .option("variant", { type: "string", choices: [...PDF_VARIANTS], describe: "masthead style" })
      .option("page-size", { type: "string", choices: ["letter", "a4"], default: "letter" })
      .option("html", { type: "boolean", describe: "write print-ready HTML instead of a PDF" })
      .option("output", { alias: "o", type: "string", describe: "output path (default: <template>.pdf)" }),
  handler: async (args: {
    template: string
    data?: string
    sample?: boolean
    theme: string
    variant?: string
    pageSize: string
    html?: boolean
    output?: string
  }) => {
    const template = args.template as PdfTemplateId
    let data: Record<string, unknown>
    if (args.sample) {
      data = PDF_TEMPLATE_INFO[template].sample
    } else if (args.data) {
      const raw = args.data === "-" ? await readStdin() : await readFile(resolve(args.data), "utf8")
      try {
        data = JSON.parse(raw)
      } catch (err) {
        throw new Error(`Data is not valid JSON: ${(err as Error).message}`)
      }
    } else {
      throw new Error("Pass --data <file.json> (or '-' for stdin), or --sample.")
    }
    const req: PdfRenderRequest = {
      template,
      theme: args.theme as PdfThemeId,
      variant: args.variant as PdfVariant | undefined,
      pageSize: args.pageSize === "a4" ? "a4" : "letter",
      data,
    }
    const bad = validateRequest(req)
    if (bad) throw new Error(bad)
    const out = resolve(args.output ?? `${template}.${args.html ? "html" : "pdf"}`)
    await mkdir(dirname(out), { recursive: true })
    if (args.html) {
      await writeFile(out, renderHtml(buildDocument(req)), "utf8")
    } else {
      const { bytes } = await renderRequestPdf(req)
      await writeFile(out, bytes)
    }
    process.stdout.write(`Wrote ${out}\n`)
  },
})

export const PdfTemplatesCommand = cmd({
  command: "templates",
  describe: "list the document templates, themes and masthead styles",
  builder: (yargs: Argv) => yargs,
  handler: async () => {
    for (const id of PDF_TEMPLATES) {
      const t = PDF_TEMPLATE_INFO[id]
      process.stdout.write(`${id.padEnd(8)} ${t.description}  [variants: ${t.variants.join(", ")}]\n`)
    }
    process.stdout.write(`\nThemes: ${PDF_THEMES.join(", ")}\n`)
  },
})

export const PdfCommand = cmd({
  command: "pdf",
  describe: "make PDFs from document templates (invoice, report, ticket, minutes, receipt)",
  builder: (yargs: Argv) => yargs.command(PdfRenderCommand).command(PdfTemplatesCommand).demandCommand(),
  async handler() {},
})
