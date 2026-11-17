import { afterEach, describe, expect, test } from "bun:test"
import { clearLookupCache, transport } from "../../src/runtime/primitives/lookup/http"
import { lookupLink, parseMeta } from "../../src/runtime/primitives/lookup/link"
import { lookupImages, toImages } from "../../src/runtime/primitives/lookup/images"

const ctx = { signal: new AbortController().signal }
const realFetch = transport.fetch
afterEach(() => {
  transport.fetch = realFetch
  clearLookupCache()
})

const html = `<html><head><title>Fallback</title>
<meta property="og:title" content="Fish &amp; Chips">
<meta property="og:site_name" content="Food Co">
<meta property="og:description" content="A tasty  page">
<meta property="og:image" content="/img/a.png">
<meta property="og:image:alt" content="A plate">
<meta property="article:published_time" content="2026-01-02"></head></html>`

describe("lookup link", () => {
  test("refuses private and local addresses without fetching", async () => {
    let called = false
    transport.fetch = (async () => ((called = true), new Response("x"))) as typeof fetch
    for (const u of ["http://127.0.0.1/a", "http://localhost:3000", "http://169.254.169.254/latest", "http://10.0.0.5/", "http://192.168.1.1/", "http://[::1]/"]) {
      await expect(lookupLink({ url: u }, ctx)).rejects.toThrow(/private or local/)
    }
    await expect(lookupLink({ url: "file:///etc/passwd" }, ctx)).rejects.toThrow(/http/)
    expect(called).toBe(false)
  })

  test("parses og tags from fixture html", async () => {
    const meta = parseMeta(html, "https://example.com/post")
    expect(meta.title).toBe("Fish & Chips")
    expect(meta.siteName).toBe("Food Co")
    expect(meta.description).toBe("A tasty page")
    expect(meta.image).toBe("https://example.com/img/a.png")
    expect(meta.imageAlt).toBe("A plate")
    expect(meta.published).toBe("2026-01-02")
  })

  test("returns a link-preview card", async () => {
    transport.fetch = (async () =>
      new Response(html, { headers: { "content-type": "text/html" } })) as typeof fetch
    const r = await lookupLink({ url: "https://example.com/post" }, ctx)
    expect(r.card?.template).toBe("link-preview")
    expect(r.credit).toBe("Preview from example.com")
    expect(r.text).toContain("Fish & Chips")
  })
})

const page = (title: string, ext: Record<string, string>, extra: object = {}) => ({
  title,
  imageinfo: [
    {
      url: `https://upload.wikimedia.org/${title}`,
      thumburl: `https://upload.wikimedia.org/thumb/${title}`,
      descriptionurl: `https://commons.wikimedia.org/wiki/${title}`,
      mime: "image/jpeg",
      extmetadata: Object.fromEntries(Object.entries(ext).map(([k, v]) => [k, { value: v }])),
    },
  ],
  ...extra,
})

describe("lookup images", () => {
  const good = { LicenseShortName: "CC BY-SA 4.0", LicenseUrl: "https://creativecommons.org/licenses/by-sa/4.0", Artist: '<a href="x">Jane Doe</a>' }
  const response = {
    query: {
      pages: [
        page("File:Cat_one.jpg", good),
        page("File:No_license.jpg", { Artist: "Bob" }),
        page("File:Naked_statue.jpg", good),
        page("File:Safe_title.jpg", good, { categories: [{ title: "Category:Nude art" }] }),
        page("File:Cat_two.jpg", good),
      ],
    },
  }

  test("keeps licensed images with credit, skips unlicensed and explicit", () => {
    const images = toImages(response)
    expect(images.map((i) => i.caption)).toEqual(["Cat one", "Cat two"])
    for (const i of images) {
      expect(i.credit).toContain("Jane Doe")
      expect(i.license).toContain("CC BY-SA 4.0")
      expect(i.sourceUrl).toStartWith("https://commons.wikimedia.org/wiki/")
    }
  })

  test("returns an image-gallery card and sends the contact User-Agent", async () => {
    let ua = ""
    transport.fetch = (async (_u: unknown, init?: RequestInit) => {
      ua = String((init?.headers as Record<string, string>)["User-Agent"])
      return new Response(JSON.stringify(response), { headers: { "content-type": "application/json" } })
    }) as typeof fetch
    const r = await lookupImages({ query: "cat" }, ctx)
    expect(ua).toContain("+https://allternit.com")
    expect(r.card?.template).toBe("image-gallery")
    expect(r.credit).toContain("Wikimedia Commons")
  })
})
