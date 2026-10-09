import assert from "node:assert/strict";
import { test } from "node:test";
import { deflateSync } from "node:zlib";
import { decodePNG, encodeGIF, paletteBuilder } from "../scripts/gif.mjs";

// A PNG with every row written through the filter given, the way a browser's
// encoder mixes them. No checksum is needed: the reader does not check one.
function png(width, height, rgba, filter) {
  const stride = width * 4, raw = Buffer.alloc(height * (stride + 1));
  const at = (x, y) => x < 0 || y < 0 ? 0 : rgba[y * stride + x];
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = filter;
    for (let x = 0; x < stride; x++) {
      const left = at(x - 4, y), up = at(x, y - 1), corner = x < 4 ? 0 : at(x - 4, y - 1), estimate = left + up - corner;
      const a = Math.abs(estimate - left), b = Math.abs(estimate - up), c = Math.abs(estimate - corner);
      const guess = [0, left, up, (left + up) >> 1, a <= b && a <= c ? left : b <= c ? up : corner][filter];
      raw[y * (stride + 1) + 1 + x] = rgba[y * stride + x] - guess;
    }
  }
  const chunk = (type, body) => { const head = Buffer.alloc(8); head.writeUInt32BE(body.length); head.write(type, 4, "latin1"); return Buffer.concat([head, body, Buffer.alloc(4)]); };
  const header = Buffer.alloc(13); header.writeUInt32BE(width, 0); header.writeUInt32BE(height, 4); header[8] = 8; header[9] = 6;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", header), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}

// Read a GIF back the way a browser draws it: each frame over the last, with
// the "unchanged" index leaving what is there.
function play(gif) {
  const width = gif.readUInt16LE(6), height = gif.readUInt16LE(8), colors = [];
  let at = 13;
  for (let n = 0; n < 256; n++, at += 3) colors.push([gif[at], gif[at + 1], gif[at + 2]]);
  const canvas = new Uint8Array(width * height), frames = [];
  let delay = 0, same = -1, loops = false;
  while (gif[at] !== 0x3b) {
    if (gif[at] === 0x21 && gif[at + 1] === 0xff) { loops = gif.toString("latin1", at + 3, at + 14) === "NETSCAPE2.0"; at += 19; continue; }
    if (gif[at] === 0x21 && gif[at + 1] === 0xf9) { same = gif[at + 3] & 1 ? gif[at + 6] : -1; delay = gif.readUInt16LE(at + 4) * 10; at += 8; continue; }
    assert.equal(gif[at], 0x2c, "an image follows its control block");
    const left = gif.readUInt16LE(at + 1), top = gif.readUInt16LE(at + 3), w = gif.readUInt16LE(at + 5), h = gif.readUInt16LE(at + 7);
    at += 11;
    const data = [];
    while (gif[at]) { data.push(...gif.subarray(at + 1, at + 1 + gif[at])); at += 1 + gif[at]; }
    at++;
    let table = [], size = 9, held = 0, bits = 0, before = null;
    const pixels = [];
    for (let n = 0; n < data.length || bits >= size;) {
      while (bits < size && n < data.length) { held |= data[n++] << bits; bits += 8; }
      const code = held & ((1 << size) - 1); held >>>= size; bits -= size;
      if (code === 256) { table = []; size = 9; before = null; continue; }
      if (code === 257) break;
      const entry = code < 256 ? [code] : table[code - 258] ?? [...before, before[0]];
      pixels.push(...entry);
      if (before) { table.push([...before, entry[0]]); if (table.length + 258 === 1 << size && size < 12) size++; }
      before = entry;
    }
    assert.equal(pixels.length, w * h, "a frame fills its rectangle");
    for (let y = 0, p = 0; y < h; y++) for (let x = 0; x < w; x++, p++) if (pixels[p] !== same) canvas[(top + y) * width + left + x] = pixels[p];
    frames.push({ delay, area: w * h, picture: [...canvas].map(index => colors[index]) });
  }
  return { width, height, loops, frames };
}

test("gif: a PNG is read whichever filter its rows were written with", () => {
  const width = 7, height = 5, rgba = Buffer.alloc(width * height * 4);
  for (let i = 0; i < rgba.length; i++) rgba[i] = (i * 37 + (i >> 3) * 11) & 255;
  for (const filter of [0, 1, 2, 3, 4]) {
    const image = decodePNG(png(width, height, rgba, filter));
    assert.deepEqual([image.width, image.height, image.channels], [width, height, 4]);
    assert.ok(image.data.equals(rgba), `filter ${filter}`);
  }
});

test("gif: an animation plays back as the frames it was made from", () => {
  // A dark terminal on a light page with text arriving a character at a time:
  // flat colours, few of them, and small changes between frames.
  const width = 96, height = 40, paper = [246, 248, 255], term = [11, 16, 48], ink = [230, 234, 255], ok = [83, 211, 154];
  const picture = typed => {
    const rgba = Buffer.alloc(width * height * 4, 255);
    const fill = (x0, y0, x1, y1, [r, g, b]) => { for (let y = y0; y < y1; y++) for (let x = x0; x < x1; x++) rgba.set([r, g, b], (y * width + x) * 4); };
    fill(0, 0, width, height, paper); fill(8, 6, 88, 34, term);
    for (let n = 0; n < typed; n++) fill(12 + n * 6, 12, 16 + n * 6, 20, n % 5 === 4 ? ok : ink);
    return { width, height, channels: 4, data: rgba };
  };
  const shown = [0, 1, 2, 2, 2, 3, 7, 12];
  const builder = paletteBuilder(); shown.forEach(typed => builder.add(picture(typed)));
  const palette = builder.build(255);
  assert.equal(palette.colors.length, 4, "four flat colours make four entries");
  const { bytes, frames } = encodeGIF({ width, height, colors: palette.colors, frames: shown.map(typed => ({ pixels: palette.map(picture(typed)), delay: 80 })) });
  const played = play(bytes);
  assert.deepEqual([played.width, played.height, played.loops], [width, height, true]);
  // The three identical frames became one that lasts as long as all three.
  assert.equal(frames, 6); assert.equal(played.frames.length, 6);
  assert.deepEqual(played.frames.map(frame => frame.delay), [80, 80, 240, 80, 80, 80]);
  const distinct = [0, 1, 2, 3, 7, 12];
  played.frames.forEach((frame, n) => {
    const { data } = picture(distinct[n]);
    frame.picture.forEach(([r, g, b], p) => assert.deepEqual([r, g, b], [data[p * 4], data[p * 4 + 1], data[p * 4 + 2]], `frame ${n}, pixel ${p}`));
  });
  // Only what changed is stored: one character's rectangle, not the picture.
  assert.equal(played.frames[0].area, width * height);
  assert.equal(played.frames[1].area, 4 * 8);
});

test("gif: a picture with more colours than the palette is still drawn close to itself", () => {
  // A long run of distinct colours also takes the code table past its limit,
  // so the stream clears it and starts again.
  const width = 256, height = 96, rgba = Buffer.alloc(width * height * 4, 255);
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) rgba.set([x, (y * 255 / (height - 1)) | 0, (x * 7 + y * 13) & 255], (y * width + x) * 4);
  const image = { width, height, channels: 4, data: rgba }, builder = paletteBuilder();
  builder.add(image);
  const palette = builder.build(255);
  assert.equal(palette.colors.length, 255);
  const [frame] = play(encodeGIF({ width, height, colors: palette.colors, frames: [{ pixels: palette.map(image), delay: 100 }] }).bytes).frames;
  let worst = 0;
  frame.picture.forEach(([r, g, b], p) => { worst = Math.max(worst, Math.abs(r - rgba[p * 4]), Math.abs(g - rgba[p * 4 + 1]), Math.abs(b - rgba[p * 4 + 2])); });
  assert.ok(worst <= 96, `no pixel is far from its colour (worst channel error ${worst})`);
});
