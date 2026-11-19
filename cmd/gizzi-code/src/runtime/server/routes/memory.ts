import { Hono } from "hono"
import { validator } from "@/runtime/server/openapi"
import z from "zod/v4"
import { MemoryService } from "@/runtime/memory/memory-service"
import { FeedbackNotes } from "@/runtime/primitives/feedback-notes"
import { errors } from "@/runtime/server/error"

export function MemoryRoutes() {
  return new Hono()
    // GET /memory/feedback-notes — local feedback notes (device only), newest first
    .get("/feedback-notes", async (c) => c.json(await FeedbackNotes.list()))

    // DELETE /memory/feedback-notes/:id — delete one local feedback note
    .delete("/feedback-notes/:id", async (c) => {
      const ok = await FeedbackNotes.remove(c.req.param("id"))
      if (!ok) return c.json({ name: "NotFound", message: "Note not found", data: {} }, 404)
      return c.json(true)
    })

    // GET /memory — list all memory entries
    .get("/", async (c) => {
      const entries = await MemoryService.list()
      return c.json(
        entries.map((e) => ({
          filename: e.filename,
          name: e.name,
          description: e.description,
          type: e.type,
          filepath: e.filepath,
        })),
      )
    })

    // GET /memory/search?q=<query> — search memories
    .get("/search", validator("query", z.object({ q: z.string().optional() })), async (c) => {
      const { q } = c.req.valid("query")
      const entries = q ? await MemoryService.search(q) : await MemoryService.list()
      return c.json(entries)
    })

    // GET /memory/:filename — read a specific memory
    .get("/:filename", async (c) => {
      const { filename } = c.req.param()
      const entry = await MemoryService.get(filename.endsWith(".md") ? filename : filename + ".md")
      if (!entry) return c.json({ name: "NotFound", message: `Memory not found: ${filename}`, data: {} }, 404)
      return c.json(entry)
    })

    // PUT /memory/:filename — save / update a memory
    .put(
      "/:filename",
      validator(
        "json",
        z.object({
          name: z.string(),
          description: z.string(),
          type: z.enum(["user", "feedback", "project", "reference"]),
          body: z.string(),
        }),
      ),
      async (c) => {
        const body = c.req.valid("json")
        const entry = await MemoryService.save(
          { name: body.name, description: body.description, type: body.type },
          body.body,
        )
        return c.json(entry)
      },
    )

    // DELETE /memory/:filename — delete a memory
    .delete("/:filename", async (c) => {
      const { filename } = c.req.param()
      const ok = await MemoryService.remove(filename.endsWith(".md") ? filename : filename + ".md")
      if (!ok) return c.json({ name: "NotFound", message: `Memory not found: ${filename}`, data: {} }, 404)
      return c.json(true)
    })
}
