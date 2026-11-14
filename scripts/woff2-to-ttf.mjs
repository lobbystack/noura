// Decode a WOFF2 font (TrueType flavor, transformed glyf and loca) to a
// TTF: node scripts/woff2-to-ttf.mjs in.woff2 out.ttf. Noura bundles
// Public Sans this way; see docs/provenance/public-sans.md.
import fs from 'node:fs';
import zlib from 'node:zlib';
const [input, output] = process.argv.slice(2);
const b = fs.readFileSync(input);
let o = 0;
const u32 = () => { const v = b.readUInt32BE(o); o += 4; return v; };
const u16 = () => { const v = b.readUInt16BE(o); o += 2; return v; };
const base128 = () => { let v = 0; for (let i = 0; i < 5; i++) { const c = b[o++]; v = v * 128 + (c & 0x7f); if (!(c & 0x80)) return v; } throw 'bad base128'; };
const KNOWN = ['cmap','head','hhea','hmtx','maxp','name','OS/2','post','cvt ','fpgm','glyf','loca','prep','CFF ','VORG','EBDT','EBLC','gasp','hdmx','kern','LTSH','PCLT','VDMX','vhea','vmtx','BASE','GDEF','GPOS','GSUB','EBSC','JSTF','MATH','CBDT','CBLC','COLR','CPAL','SVG ','sbix','acnt','avar','bdat','bloc','bsln','cvar','fdsc','feat','fmtx','fvar','gvar','hsty','just','lcar','mort','morx','opbd','prop','trak','Zapf','Silf','Glat','Gloc','Feat','Sill'];
if (u32() !== 0x774f4632) throw 'not woff2';
const flavor = u32(); u32(); const num = u16(); u16(); u32(); const compLen = (o = 20, u32());
o = 48;
const tables = [];
for (let i = 0; i < num; i++) {
  const f = b[o++];
  const tag = (f & 63) === 63 ? b.toString('latin1', o, (o += 4)) : KNOWN[f & 63];
  const tv = f >> 6;
  const orig = base128();
  let len = orig;
  const transformed = (tag === 'glyf' || tag === 'loca') ? tv === 0 : tv !== 0;
  if (transformed) len = base128();
  tables.push({ tag, orig, len, transformed });
}
const data = zlib.brotliDecompressSync(b.subarray(o, o + compLen));
let at = 0;
for (const t of tables) { t.data = data.subarray(at, at + t.len); at += t.len; }
const byTag = Object.fromEntries(tables.map((t) => [t.tag, t]));
for (const t of tables) if (t.transformed && t.tag !== 'glyf' && t.tag !== 'loca') throw `unsupported transform of ${t.tag}`;

// Reconstruct glyf and loca.
const g = byTag.glyf.data;
let p = 0;
const r16 = () => { const v = g.readUInt16BE(p); p += 2; return v; };
r16(); const optionFlags = r16(); const numGlyphs = r16(); r16();
const sizes = []; for (let i = 0; i < 7; i++) { sizes.push(g.readUInt32BE(p)); p += 4; }
const streams = []; for (const s of sizes) { streams.push({ buf: g.subarray(p, p + s), pos: 0 }); p += s; }
const [nContourS, nPointsS, flagS, glyphS, compositeS, bboxS, instrS] = streams;
const overlap = optionFlags & 1 ? { buf: g.subarray(p), pos: 0 } : null;
const bitmapLen = 4 * Math.floor((numGlyphs + 31) / 32);
const bboxBitmap = bboxS.buf.subarray(0, bitmapLen); bboxS.pos = bitmapLen;
const s16 = (s) => { const v = s.buf.readInt16BE(s.pos); s.pos += 2; return v; };
const s8 = (s) => s.buf[s.pos++];
const read255 = (s) => { const c = s8(s); if (c === 253) { const v = s.buf.readUInt16BE(s.pos); s.pos += 2; return v; } if (c === 255) return 253 + s8(s); if (c === 254) return 506 + s8(s); return c; };
const bytes = (s, n) => { const v = s.buf.subarray(s.pos, s.pos + n); s.pos += n; return v; };
const withSign = (flag, base) => (flag & 1 ? base : -base);
const glyphs = [];
for (let i = 0; i < numGlyphs; i++) {
  const nContours = s16(nContourS);
  const hasBbox = bboxBitmap[i >> 3] & (0x80 >> (i & 7));
  const overlapBit = overlap && overlap.buf[i >> 3] & (0x80 >> (i & 7));
  if (nContours === 0) { glyphs.push(Buffer.alloc(0)); continue; }
  if (nContours === -1) {
    const start = compositeS.pos; let flags; let instructions = false;
    do {
      flags = compositeS.buf.readUInt16BE(compositeS.pos);
      let size = 4 + (flags & 1 ? 4 : 2);
      if (flags & 8) size += 2; else if (flags & 0x40) size += 4; else if (flags & 0x80) size += 8;
      if (flags & 0x100) instructions = true;
      compositeS.pos += size;
    } while (flags & 0x20);
    const comp = compositeS.buf.subarray(start, compositeS.pos);
    const bbox = bytes(bboxS, 8);
    const parts = [Buffer.from([0xff, 0xff]), bbox, comp];
    if (instructions) { const n = read255(glyphS); const l = Buffer.alloc(2); l.writeUInt16BE(n); parts.push(l, bytes(instrS, n)); }
    glyphs.push(Buffer.concat(parts)); continue;
  }
  const endPts = []; let total = 0;
  for (let c = 0; c < nContours; c++) { total += read255(nPointsS); endPts.push(total - 1); }
  const xs = [], ys = [], on = [];
  let x = 0, y = 0;
  for (let k = 0; k < total; k++) {
    let flag = s8(flagS); on.push(!(flag & 0x80)); flag &= 0x7f;
    const s = glyphS; let dx, dy;
    if (flag < 10) { dx = 0; dy = withSign(flag, ((flag & 14) << 7) + s8(s)); }
    else if (flag < 20) { dx = withSign(flag, (((flag - 10) & 14) << 7) + s8(s)); dy = 0; }
    else if (flag < 84) { const b0 = flag - 20, b1 = s8(s); dx = withSign(flag, 1 + (b0 & 0x30) + (b1 >> 4)); dy = withSign(flag >> 1, 1 + ((b0 & 0x0c) << 2) + (b1 & 0x0f)); }
    else if (flag < 120) { const b0 = flag - 84, i0 = s8(s), i1 = s8(s); dx = withSign(flag, 1 + (Math.floor(b0 / 12) << 8) + i0); dy = withSign(flag >> 1, 1 + (((b0 % 12) >> 2) << 8) + i1); }
    else if (flag < 124) { const i0 = s8(s), i1 = s8(s), i2 = s8(s); dx = withSign(flag, (i0 << 4) + (i1 >> 4)); dy = withSign(flag >> 1, ((i1 & 0x0f) << 8) + i2); }
    else { const i0 = s8(s), i1 = s8(s), i2 = s8(s), i3 = s8(s); dx = withSign(flag, (i0 << 8) + i1); dy = withSign(flag >> 1, (i2 << 8) + i3); }
    x += dx; y += dy; xs.push(x); ys.push(y);
  }
  const nInstr = read255(glyphS);
  const instr = bytes(instrS, nInstr);
  let bbox;
  if (hasBbox) bbox = bytes(bboxS, 8);
  else { bbox = Buffer.alloc(8); bbox.writeInt16BE(Math.min(...xs), 0); bbox.writeInt16BE(Math.min(...ys), 2); bbox.writeInt16BE(Math.max(...xs), 4); bbox.writeInt16BE(Math.max(...ys), 6); }
  const head = Buffer.alloc(2); head.writeInt16BE(nContours);
  const ends = Buffer.alloc(2 * nContours); endPts.forEach((e, k) => ends.writeUInt16BE(e, 2 * k));
  const il = Buffer.alloc(2); il.writeUInt16BE(nInstr);
  const fl = Buffer.alloc(total); on.forEach((v, k) => { fl[k] = (v ? 1 : 0) | (k === 0 && overlapBit ? 0x40 : 0); });
  const cx = Buffer.alloc(2 * total), cy = Buffer.alloc(2 * total);
  let px = 0, py = 0;
  for (let k = 0; k < total; k++) { cx.writeInt16BE(xs[k] - px, 2 * k); cy.writeInt16BE(ys[k] - py, 2 * k); px = xs[k]; py = ys[k]; }
  glyphs.push(Buffer.concat([head, bbox, ends, il, instr, fl, cx, cy]));
}
const glyfParts = []; const loca = Buffer.alloc(4 * (numGlyphs + 1)); let off = 0;
glyphs.forEach((gl, i) => { loca.writeUInt32BE(off, 4 * i); const pad = (4 - (gl.length % 4)) % 4; glyfParts.push(gl, Buffer.alloc(pad)); off += gl.length + pad; });
loca.writeUInt32BE(off, 4 * numGlyphs);
byTag.glyf.data = Buffer.concat(glyfParts);
byTag.loca.data = loca;
const headT = Buffer.from(byTag.head.data); headT.writeInt16BE(1, 50); headT.writeUInt32BE(0, 8); byTag.head.data = headT;

// Write the sfnt.
const sorted = [...tables].sort((a, c) => (a.tag < c.tag ? -1 : 1));
const n = sorted.length; let pow = 1, log = 0; while (pow * 2 <= n) { pow *= 2; log++; }
const header = Buffer.alloc(12 + 16 * n);
header.writeUInt32BE(flavor, 0); header.writeUInt16BE(n, 4); header.writeUInt16BE(pow * 16, 6); header.writeUInt16BE(log, 8); header.writeUInt16BE(n * 16 - pow * 16, 10);
const checksum = (buf) => { let sum = 0; const padded = Buffer.concat([buf, Buffer.alloc((4 - (buf.length % 4)) % 4)]); for (let k = 0; k < padded.length; k += 4) sum = (sum + padded.readUInt32BE(k)) >>> 0; return sum; };
let offset = header.length; const body = [];
sorted.forEach((t, k) => {
  const rec = 12 + 16 * k;
  header.write(t.tag, rec, 'latin1'); header.writeUInt32BE(checksum(t.data), rec + 4); header.writeUInt32BE(offset, rec + 8); header.writeUInt32BE(t.data.length, rec + 12);
  const pad = (4 - (t.data.length % 4)) % 4; body.push(t.data, Buffer.alloc(pad)); offset += t.data.length + pad;
});
const font = Buffer.concat([header, ...body]);
const headOffset = header.readUInt32BE(12 + 16 * sorted.findIndex((t) => t.tag === 'head') + 8);
font.writeUInt32BE((0xb1b0afba - checksum(font)) >>> 0, headOffset + 8);
fs.writeFileSync(output, font);
console.log(output, font.length, 'glyphs', numGlyphs);
