// Generates src-assets/icon.png (1024×1024 RGBA) with zero dependencies:
// a white harness ring around a DeepSeek-blue core on a dark field.
// `pnpm icon` then feeds it to `tauri icon` for all platform sizes.
import { deflateSync } from 'node:zlib'
import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const SIZE = 1024
const out = join(
  dirname(fileURLToPath(import.meta.url)),
  '..',
  'src-assets',
  'icon.png',
)

const lerp = (a, b, t) => a + (b - a) * t

function pixel(x, y) {
  const t = y / SIZE
  let [r, g, b] = [lerp(11, 16, t), lerp(16, 27, t), lerp(35, 60, t)]
  const d = Math.hypot(x - SIZE / 2, y - SIZE / 2)
  if (Math.abs(d - 330) < 13) {
    ;[r, g, b] = [255, 255, 255] // harness ring
  }
  if (d < 236) {
    ;[r, g, b] = [77, 107, 254] // core dot #4D6BFE
  }
  return [Math.round(r), Math.round(g), Math.round(b), 255]
}

const raw = Buffer.alloc(SIZE * (1 + SIZE * 4))
for (let y = 0; y < SIZE; y += 1) {
  const row = y * (1 + SIZE * 4)
  raw[row] = 0 // filter: none
  for (let x = 0; x < SIZE; x += 1) {
    const [r, g, b, a] = pixel(x, y)
    const offset = row + 1 + x * 4
    raw[offset] = r
    raw[offset + 1] = g
    raw[offset + 2] = b
    raw[offset + 3] = a
  }
}

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n
  for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
  return c >>> 0
})
const crc32 = (buf) => {
  let c = 0xffffffff
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}
const chunk = (type, data) => {
  const length = Buffer.alloc(4)
  length.writeUInt32BE(data.length)
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(body))
  return Buffer.concat([length, body, crc])
}

const ihdr = Buffer.alloc(13)
ihdr.writeUInt32BE(SIZE, 0)
ihdr.writeUInt32BE(SIZE, 4)
ihdr[8] = 8 // bit depth
ihdr[9] = 6 // color type: RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk('IHDR', ihdr),
  chunk('IDAT', deflateSync(raw, { level: 9 })),
  chunk('IEND', Buffer.alloc(0)),
])

mkdirSync(dirname(out), { recursive: true })
writeFileSync(out, png)
console.log(`wrote ${out} (${png.length} bytes)`)
