#!/usr/bin/env node
// Regenerates icon.png (128x128 RGBA) with zero image dependencies: a tiny
// software rasterizer + the PNG encoder from node:zlib. Deterministic —
// re-running it on an unchanged source must be byte-identical.
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const S = 128; // final size
const SS = 3; // supersampling factor for anti-aliasing
const W = S * SS;

// Palette (dark slate panel, oxo-flow DAG in cyan/teal).
const BG = [16, 24, 40, 255]; // #101828
const EDGE = [29, 41, 64, 255]; // subtle inner edge
const NODE_A = [34, 211, 238, 255]; // #22d3ee cyan
const NODE_B = [52, 211, 153, 255]; // #34d399 emerald
const NODE_C = [167, 139, 250, 255]; // #a78bfa violet
const FLOW = [148, 163, 184, 255]; // #94a3b8 slate edges

function px(x, y) {
  // Canvas coordinates in final-pixel space (0..S); returns RGBA or null.
  // Rounded-square panel
  const r = 24;
  const x0 = 4, y0 = 4, x1 = S - 4, y1 = S - 4;
  if (x < x0 || x > x1 || y < y0 || y > y1) return null;
  const cx = Math.min(Math.max(x, x0 + r), x1 - r);
  const cy = Math.min(Math.max(y, y0 + r), y1 - r);
  if ((x - cx) ** 2 + (y - cy) ** 2 > r * r) return null;
  let c = BG;
  // inner edge ring for depth
  const rIn = 23;
  const cxi = Math.min(Math.max(x, x0 + rIn), x1 - rIn);
  const cyi = Math.min(Math.max(y, y0 + rIn), y1 - rIn);
  if ((x - cxi) ** 2 + (y - cyi) ** 2 > rIn * rIn) c = EDGE;

  // DAG: three nodes and two edges (A -> B, A -> C)
  const nodes = [
    { x: 42, y: 40, r: 11, c: NODE_A },
    { x: 40, y: 88, r: 11, c: NODE_B },
    { x: 88, y: 64, r: 13, c: NODE_C },
  ];
  const [a, b, d] = nodes;
  for (const [p, q] of [[a, d], [b, d]]) {
    if (distToSegment(x, y, p.x, p.y, q.x, q.y) < 2.6) return FLOW;
  }
  for (const n of nodes) {
    const dist = Math.hypot(x - n.x, y - n.y);
    if (dist < n.r - 3) return n.c;
    if (dist < n.r - 1.6) return darken(n.c);
  }
  return c;
}

function darken([R, G, B, a]) {
  return [Math.round(R * 0.55), Math.round(G * 0.55), Math.round(B * 0.55), a];
}

function distToSegment(px0, py0, x1, y1, x2, y2) {
  const dx = x2 - x1, dy = y2 - y1;
  const l2 = dx * dx + dy * dy;
  let t = l2 === 0 ? 0 : ((px0 - x1) * dx + (py0 - y1) * dy) / l2;
  t = Math.min(1, Math.max(0, t));
  return Math.hypot(px0 - (x1 + t * dx), py0 - (y1 + t * dy));
}

// Render with NxN box supersampling.
const raw = Buffer.alloc(S * S * 4);
for (let y = 0; y < S; y++) {
  for (let x = 0; x < S; x++) {
    let R = 0, G = 0, B = 0, A = 0;
    for (let sy = 0; sy < SS; sy++) {
      for (let sx = 0; sx < SS; sx++) {
        const c = px(x + (sx + 0.5) / SS, y + (sy + 0.5) / SS);
        if (c) {
          R += c[0]; G += c[1]; B += c[2]; A += c[3];
        }
      }
    }
    const n = SS * SS;
    const o = (y * S + x) * 4;
    raw[o] = Math.round(R / n);
    raw[o + 1] = Math.round(G / n);
    raw[o + 2] = Math.round(B / n);
    raw[o + 3] = Math.round(A / n);
  }
}

// Minimal PNG encoder: 8-bit RGBA, no filtering (filter byte 0 per scanline).
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crcTable = crc32Table();
  let crc = 0xffffffff;
  for (const byte of body) crc = crcTable[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  crc = (crc ^ 0xffffffff) >>> 0;
  const crcBuf = Buffer.alloc(4);
  crcBuf.writeUInt32BE(crc);
  return Buffer.concat([len, body, crcBuf]);
}

function crc32Table() {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0);
ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // color type RGBA
const scanlines = Buffer.alloc(S * (S * 4 + 1));
for (let y = 0; y < S; y++) {
  scanlines[y * (S * 4 + 1)] = 0;
  raw.copy(scanlines, y * (S * 4 + 1) + 1, y * S * 4, (y + 1) * S * 4);
}
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(scanlines, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

const out = join(dirname(fileURLToPath(import.meta.url)), "..", "icon.png");
writeFileSync(out, png);
console.log(`wrote ${out} (${png.length} bytes)`);
