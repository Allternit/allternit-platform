import { describe, expect, it } from 'vitest'
import JSZip from 'jszip'
import { createXlsxSessionRouter, zipUnpackedBytes } from '../src/xlsx-sessions'
import type { XlsxSidecarClient } from '@allternit/office-xlsx-engine'

function fakeSidecar() {
  let next = 0
  const closed: string[] = []
  const client = {
    open: async () => ({ sessionId: `s${(next += 1)}`, sheets: [] }),
    close: async (id: string) => void closed.push(id),
    readRange: async () => ({ cells: [] }),
    inspect: async () => ({ sheets: [] }),
  } as unknown as XlsxSidecarClient
  return { client, closed }
}

async function zipBase64(entries: Record<string, string>): Promise<string> {
  const zip = new JSZip()
  for (const [name, text] of Object.entries(entries)) zip.file(name, text)
  return (await zip.generateAsync({ type: 'nodebuffer', compression: 'DEFLATE' })).toString('base64')
}

const post = (app: ReturnType<typeof createXlsxSessionRouter>, path: string, body: unknown) =>
  app.request(path, { method: 'POST', body: JSON.stringify(body), headers: { 'content-type': 'application/json' } })

describe('zipUnpackedBytes', () => {
  it('reads what a zip claims to unpack to, and spots a bomb it never unpacks', async () => {
    const small = Buffer.from(await zipBase64({ 'a.xml': 'x'.repeat(1000), 'b.xml': 'y'.repeat(500) }), 'base64')
    expect(zipUnpackedBytes(small)).toBe(1500)
    const bomb = Buffer.from(await zipBase64({ 'sheet.xml': '0'.repeat(20 * 1024 * 1024) }), 'base64')
    expect(bomb.length).toBeLessThan(100 * 1024)
    expect(zipUnpackedBytes(bomb)).toBe(20 * 1024 * 1024)
  })

  it('leaves a file that is no zip to the parser, and refuses a broken zip', () => {
    expect(zipUnpackedBytes(Buffer.from('a,b,c\n1,2,3\n'.repeat(10)))).toBe(0)
    const broken = Buffer.concat([Buffer.from([0x50, 0x4b, 0x03, 0x04]), Buffer.alloc(100)])
    expect(zipUnpackedBytes(broken)).toBeNull()
  })
})

describe('xlsx session limits', () => {
  it('refuses a zip bomb and an oversized file before opening anything', async () => {
    const { client } = fakeSidecar()
    const app = createXlsxSessionRouter(() => client, { maxUnpackedBytes: 1024, maxFileBytes: 1024 * 1024 })
    const bomb = await post(app, '/open', { name: 'a.xlsx', bytesBase64: await zipBase64({ 'a.xml': 'x'.repeat(4096) }) })
    expect(bomb.status).toBe(413)
    const huge = await post(app, '/open', { name: 'a.csv', bytesBase64: Buffer.alloc(2 * 1024 * 1024, 65).toString('base64') })
    expect(huge.status).toBe(413)
  })

  it('answers 429 at the session cap, and an unknown session gets 404', async () => {
    const { client } = fakeSidecar()
    const app = createXlsxSessionRouter(() => client, { maxSessions: 0 })
    const full = await post(app, '/open', { name: 'a.csv', bytesBase64: Buffer.from('a').toString('base64') })
    expect(full.status).toBe(429)
    const unknown = await post(app, '/range', { sessionId: 'nope', sheetId: 's', range: { startRow: 0 } })
    expect(unknown.status).toBe(404)
  })
})
