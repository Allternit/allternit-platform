#!/usr/bin/env node
// Packs a CRX3 (Chrome extension) from a zip, signed with an RSA private key.
// No dependencies. Usage:
//   node pack-crx3.mjs <extension.zip> <private-key.pem> <out.crx>
// Prints the extension id (derived from the key, a-p alphabet) on stdout.
import { createHash, createPrivateKey, createPublicKey, sign } from 'node:crypto'
import { readFileSync, writeFileSync } from 'node:fs'

const [zipPath, keyPath, outPath] = process.argv.slice(2)
if (!zipPath || !keyPath || !outPath) {
  console.error('usage: pack-crx3.mjs <extension.zip> <private-key.pem> <out.crx>')
  process.exit(2)
}

const varint = (n) => {
  const out = []
  while (n > 0x7f) { out.push((n & 0x7f) | 0x80); n = Math.floor(n / 128) }
  out.push(n)
  return Buffer.from(out)
}
// protobuf: length-delimited field
const lenField = (field, buf) => Buffer.concat([varint((field << 3) | 2), varint(buf.length), buf])

const zip = readFileSync(zipPath)
const priv = createPrivateKey(readFileSync(keyPath))
const pubDer = createPublicKey(priv).export({ type: 'spki', format: 'der' })

const crxIdBytes = createHash('sha256').update(pubDer).digest().subarray(0, 16)
const extId = [...crxIdBytes].map((b) => String.fromCharCode(97 + (b >> 4)) + String.fromCharCode(97 + (b & 15))).join('')

const signedData = lenField(1, crxIdBytes) // SignedData { crx_id = 1 }
const lenLE = Buffer.alloc(4)
lenLE.writeUInt32LE(signedData.length)
const toSign = Buffer.concat([Buffer.from('CRX3 SignedData\0', 'latin1'), lenLE, signedData, zip])
const signature = sign('sha256', toSign, priv) // RSASSA-PKCS1-v1_5 + SHA-256

const proof = Buffer.concat([lenField(1, pubDer), lenField(2, signature)]) // AsymmetricKeyProof
const header = Buffer.concat([lenField(2, proof), lenField(10000, signedData)]) // CrxFileHeader

const pre = Buffer.alloc(12)
pre.write('Cr24', 0, 'latin1')
pre.writeUInt32LE(3, 4)
pre.writeUInt32LE(header.length, 8)
writeFileSync(outPath, Buffer.concat([pre, header, zip]))
console.log(extId)
