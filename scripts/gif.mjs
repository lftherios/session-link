// A small animated-GIF writer for scripts/capture-how-it-works.mjs, so the
// recording needs nothing installed beyond Node and a browser. It reads the
// PNG frames a browser screenshot gives, reduces them to one palette, and
// writes each frame as only the rectangle that changed since the last one,
// which is what keeps a recording of typed text small.
import { inflateSync } from "node:zlib";

/** Decode an 8-bit RGB or RGBA, non-interlaced PNG to { width, height, channels, data }. */
export function decodePNG(png) {
  let at = 8, width = 0, height = 0, channels = 0;
  const compressed = [];
  while (at < png.length) {
    const length = png.readUInt32BE(at), type = png.toString("latin1", at + 4, at + 8), body = png.subarray(at + 8, at + 8 + length);
    if (type === "IHDR") {
      width = body.readUInt32BE(0); height = body.readUInt32BE(4);
      channels = { 2: 3, 6: 4 }[body[9]];
      if (body[8] !== 8 || !channels || body[12] !== 0) throw new Error("only 8-bit RGB or RGBA PNGs without interlacing are read");
    } else if (type === "IDAT") compressed.push(body);
    else if (type === "IEND") break;
    at += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(compressed)), stride = width * channels, data = Buffer.alloc(height * stride);
  for (let y = 0, from = 0; y < height; y++) {
    const filter = raw[from++], row = y * stride, above = row - stride;
    for (let x = 0; x < stride; x++) {
      const left = x >= channels ? data[row + x - channels] : 0, up = y ? data[above + x] : 0, corner = y && x >= channels ? data[above + x - channels] : 0;
      let value = raw[from + x];
      if (filter === 1) value += left;
      else if (filter === 2) value += up;
      else if (filter === 3) value += (left + up) >> 1;
      else if (filter === 4) {
        const estimate = left + up - corner, a = Math.abs(estimate - left), b = Math.abs(estimate - up), c = Math.abs(estimate - corner);
        value += a <= b && a <= c ? left : b <= c ? up : corner;
      }
      data[row + x] = value;
    }
    from += stride;
  }
  return { width, height, channels, data };
}

// Colours are counted in cells of 4 levels a channel. A flat colour keeps its
// exact value, because a cell's palette entry is the mean of what fell in it.
const BITS = 6, CELLS = 1 << (3 * BITS), SHIFT = 8 - BITS;
const cellOf = (r, g, b) => ((r >> SHIFT) << (2 * BITS)) | ((g >> SHIFT) << BITS) | (b >> SHIFT);

/** Collects the colours of every frame, then answers with a palette of at most `size` colours. */
export function paletteBuilder() {
  const count = new Float64Array(CELLS), sum = [new Float64Array(CELLS), new Float64Array(CELLS), new Float64Array(CELLS)];
  return {
    add({ data, channels }) {
      for (let i = 0; i < data.length; i += channels) {
        const cell = cellOf(data[i], data[i + 1], data[i + 2]);
        count[cell]++; sum[0][cell] += data[i]; sum[1][cell] += data[i + 1]; sum[2][cell] += data[i + 2];
      }
    },
    // Median cut: split the box holding the most pixels along its longest
    // side, at the pixel that halves it, until there are enough boxes.
    build(size) {
      const used = []; for (let cell = 0; cell < CELLS; cell++) if (count[cell]) used.push(cell);
      const mean = (cell, channel) => sum[channel][cell] / count[cell];
      const measure = cells => {
        const box = { cells, pixels: 0, side: 0, range: 0 };
        for (let channel = 0; channel < 3; channel++) {
          let low = 255, high = 0;
          for (const cell of cells) { const value = mean(cell, channel); if (value < low) low = value; if (value > high) high = value; }
          if (high - low > box.range) { box.range = high - low; box.side = channel; }
        }
        for (const cell of cells) box.pixels += count[cell];
        return box;
      };
      const boxes = [measure(used)];
      while (boxes.length < size) {
        let pick = -1;
        boxes.forEach((box, n) => { if (box.cells.length > 1 && (pick < 0 || box.pixels * box.range > boxes[pick].pixels * boxes[pick].range)) pick = n; });
        if (pick < 0) break;
        const box = boxes[pick], sorted = box.cells.slice().sort((a, b) => mean(a, box.side) - mean(b, box.side));
        let cut = 0, seen = 0;
        while (cut < sorted.length - 1 && seen + count[sorted[cut]] <= box.pixels / 2) seen += count[sorted[cut++]];
        cut = Math.max(1, cut);
        boxes.splice(pick, 1, measure(sorted.slice(0, cut)), measure(sorted.slice(cut)));
      }
      const colors = boxes.map(box => {
        const total = [0, 0, 0];
        for (const cell of box.cells) for (let channel = 0; channel < 3; channel++) total[channel] += sum[channel][cell];
        return total.map(value => Math.round(value / box.pixels));
      });
      const index = new Int16Array(CELLS).fill(-1);
      boxes.forEach((box, n) => { for (const cell of box.cells) index[cell] = n; });
      // A colour no frame was counted with takes the nearest entry.
      const nearest = (r, g, b) => {
        let best = 0, distance = Infinity;
        colors.forEach(([cr, cg, cb], n) => { const d = (r - cr) ** 2 + (g - cg) ** 2 + (b - cb) ** 2; if (d < distance) { distance = d; best = n; } });
        return best;
      };
      return {
        colors,
        map({ data, channels, width, height }) {
          const out = new Uint8Array(width * height);
          for (let i = 0, p = 0; i < data.length; i += channels, p++) {
            const cell = cellOf(data[i], data[i + 1], data[i + 2]);
            if (index[cell] < 0) index[cell] = nearest(data[i], data[i + 1], data[i + 2]);
            out[p] = index[cell];
          }
          return out;
        },
      };
    },
  };
}

// GIF's LZW: codes start one bit wider than the palette index and grow to
// twelve bits, when the table is cleared and starts again.
function compress(pixels) {
  const bytes = [], CLEAR = 256, END = 257;
  let table = new Map(), next = END + 1, width = 9, held = 0, bits = 0;
  const write = code => { held |= code << bits; bits += width; while (bits >= 8) { bytes.push(held & 255); held >>>= 8; bits -= 8; } };
  write(CLEAR);
  let prefix = pixels[0];
  for (let i = 1; i < pixels.length; i++) {
    const key = (prefix << 8) | pixels[i], known = table.get(key);
    if (known !== undefined) { prefix = known; continue; }
    write(prefix);
    if (next === 4096) { write(CLEAR); table = new Map(); next = END + 1; width = 9; }
    else { if (next >= 1 << width) width++; table.set(key, next++); }
    prefix = pixels[i];
  }
  write(prefix); write(END);
  if (bits) bytes.push(held & 255);
  return bytes;
}

const word = value => [value & 255, value >> 8];

/**
 * Write an animated GIF that loops. `frames` are { pixels, delay }: palette
 * indexes for the whole picture and how long it shows, in milliseconds.
 * `colors` has at most 255 entries; the last index means "unchanged".
 */
export function encodeGIF({ width, height, colors, frames }) {
  if (colors.length > 255) throw new Error("the palette leaves one index free for unchanged pixels");
  const SAME = 255, out = [...Buffer.from("GIF89a"), ...word(width), ...word(height), 0xf7, 0, 0];
  for (let n = 0; n < 256; n++) out.push(...(colors[n] ?? [0, 0, 0]));
  out.push(0x21, 0xff, 11, ...Buffer.from("NETSCAPE2.0"), 3, 1, 0, 0, 0);
  // A frame the same as the one before only lengthens it.
  const kept = [];
  for (const frame of frames) {
    const last = kept.at(-1);
    if (last && last.pixels.every((value, n) => value === frame.pixels[n])) last.delay += frame.delay;
    else kept.push({ ...frame });
  }
  kept.forEach((frame, n) => {
    let left = 0, top = 0, right = width - 1, bottom = height - 1, pixels = frame.pixels;
    if (n) {
      const before = kept[n - 1].pixels;
      left = width; top = height; right = -1; bottom = -1;
      for (let y = 0, p = 0; y < height; y++) for (let x = 0; x < width; x++, p++) if (pixels[p] !== before[p]) {
        if (x < left) left = x; if (x > right) right = x; if (y < top) top = y; if (y > bottom) bottom = y;
      }
      const w = right - left + 1, part = new Uint8Array(w * (bottom - top + 1));
      for (let y = top, q = 0; y <= bottom; y++) for (let x = left, p = y * width + left; x <= right; x++, p++, q++) part[q] = pixels[p] === before[p] ? SAME : pixels[p];
      pixels = part;
    }
    // Each frame is drawn over what is there and left in place.
    out.push(0x21, 0xf9, 4, n ? 0x05 : 0x04, ...word(Math.max(2, Math.round(frame.delay / 10))), SAME, 0);
    out.push(0x2c, ...word(left), ...word(top), ...word(right - left + 1), ...word(bottom - top + 1), 0, 8);
    const data = compress(pixels);
    for (let at = 0; at < data.length; at += 255) { const block = data.slice(at, at + 255); out.push(block.length, ...block); }
    out.push(0);
  });
  out.push(0x3b);
  return { bytes: Buffer.from(out), frames: kept.length };
}
