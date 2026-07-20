// Shared document-scanning module for the browser demos (index.html,
// checkin.html) — the JS port of the native pipeline's v1.1.0 MRZ retry
// strategy (crates/synthpass-ocr/src/preprocess.rs): try the OCR-B-trained model
// over preprocessed variants of the MRZ band (plain, percentile contrast
// stretch, Otsu binarization), then the full image, then the generic model,
// and stop at the first checksum-VALID parse. The ICAO check digits are the
// oracle: a retry can add a valid read but never break one.
//
// Everything runs in-tab: canvases are in-memory, tesseract.js workers are
// local, and the WASM parser is the same `mrz` crate the native pipeline uses.

const MRZ_CHARS = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789<';

/** Downscale an image to maxDim on the largest side — all in-memory. */
async function toCanvas(bitmap, maxDim = 1600) {
  const scale = Math.min(1, maxDim / Math.max(bitmap.width, bitmap.height));
  const c = document.createElement('canvas');
  c.width = Math.round(bitmap.width * scale);
  c.height = Math.round(bitmap.height * scale);
  c.getContext('2d').drawImage(bitmap, 0, 0, c.width, c.height);
  return c;
}

/** Crop the bottom band (where the MRZ lives on every ICAO layout) and
 *  upscale it toward targetWidth (capped — past ~3× there is no new signal,
 *  only interpolation blur and slower OCR). Mirrors the native
 *  `preprocess::bottom_band` + capped upscale. */
function bottomBand(canvas, { fraction = 0.45, targetWidth = 1600, maxScale = 3 } = {}) {
  const srcY = Math.round(canvas.height * (1 - fraction));
  const upscale = Math.min(maxScale, Math.max(1, targetWidth / canvas.width));
  const c = document.createElement('canvas');
  c.width = Math.round(canvas.width * upscale);
  c.height = Math.round((canvas.height - srcY) * upscale);
  const ctx = c.getContext('2d');
  ctx.imageSmoothingEnabled = true;
  ctx.drawImage(canvas, 0, srcY, canvas.width, canvas.height - srcY, 0, 0, c.width, c.height);
  return c;
}

/** Grayscale luma histogram of a canvas + the pixel data to transform. */
function grayData(canvas) {
  const ctx = canvas.getContext('2d');
  const img = ctx.getImageData(0, 0, canvas.width, canvas.height);
  const d = img.data;
  const luma = new Uint8Array(d.length / 4);
  const hist = new Uint32Array(256);
  for (let i = 0, p = 0; i < d.length; i += 4, p++) {
    const y = Math.round(0.299 * d[i] + 0.587 * d[i + 1] + 0.114 * d[i + 2]);
    luma[p] = y;
    hist[y]++;
  }
  return { ctx, img, d, luma, hist };
}

function applyLuma(g, map) {
  const { ctx, img, d, luma } = g;
  for (let i = 0, p = 0; i < d.length; i += 4, p++) {
    d[i] = d[i + 1] = d[i + 2] = map[luma[p]];
  }
  ctx.putImageData(img, 0, 0);
}

/** Copy of `canvas`, grayscaled with a linear contrast stretch mapping the
 *  1st..99th intensity percentiles to 0..255 — robust on washed-out
 *  guilloche backgrounds (native: `preprocess::contrast_stretched`). */
function contrastStretched(canvas) {
  const c = cloneCanvas(canvas);
  const g = grayData(c);
  const total = c.width * c.height;
  let lo = 0, hi = 255, cum = 0, loSet = false;
  for (let v = 0; v < 256; v++) {
    cum += g.hist[v];
    if (!loSet && cum > total * 0.01) { lo = v; loSet = true; }
    if (cum >= total * 0.99) { hi = v; break; }
  }
  const range = Math.max(1, hi - lo);
  const map = new Uint8Array(256);
  for (let v = 0; v < 256; v++) {
    map[v] = Math.max(0, Math.min(255, Math.round(((v - lo) / range) * 255)));
  }
  applyLuma(g, map);
  return c;
}

/** Copy of `canvas`, grayscaled + Otsu global threshold to pure black/white —
 *  strongest on clean-but-tiny scans (native: `preprocess::binarized`). */
function otsuBinarized(canvas) {
  const c = cloneCanvas(canvas);
  const g = grayData(c);
  const total = c.width * c.height;
  let sum = 0;
  for (let v = 0; v < 256; v++) sum += v * g.hist[v];
  let sumB = 0, wB = 0, maxVar = -1, thresh = 127;
  for (let v = 0; v < 256; v++) {
    wB += g.hist[v];
    if (wB === 0) continue;
    const wF = total - wB;
    if (wF === 0) break;
    sumB += v * g.hist[v];
    const mB = sumB / wB;
    const mF = (sum - sumB) / wF;
    const variance = wB * wF * (mB - mF) * (mB - mF);
    if (variance > maxVar) { maxVar = variance; thresh = v; }
  }
  const map = new Uint8Array(256);
  for (let v = 0; v < 256; v++) map[v] = v > thresh ? 255 : 0;
  applyLuma(g, map);
  return c;
}

function cloneCanvas(canvas) {
  const c = document.createElement('canvas');
  c.width = canvas.width;
  c.height = canvas.height;
  c.getContext('2d').drawImage(canvas, 0, 0);
  return c;
}

// Two OCR models: 'mrz' is fine-tuned on the OCR-B font MRZs are printed in
// (vendored, BSD-3-Clause © DoubangoTelecom — see tessdata/LICENSE); 'eng'
// is the generic fallback. Both are restricted to the MRZ charset — the JS
// equivalent of the native engine's allowed_chars. Every runtime asset is
// same-origin: fetched + SHA-256-verified at deploy time by fetch-vendor.sh,
// so the page makes zero CDN requests.
const workers = {};
function getWorker(lang) {
  workers[lang] ??= (async () => {
    const worker = await Tesseract.createWorker(lang, 1, {
      workerPath: './vendor/worker.min.js',
      corePath: './vendor',
      langPath: './tessdata',
      gzip: lang !== 'mrz', // mrz.traineddata is committed uncompressed
    });
    await worker.setParameters({ tessedit_char_whitelist: MRZ_CHARS });
    return worker;
  })();
  return workers[lang];
}

/**
 * Scan `file` for an MRZ. `parse(text)` must return a parse result object
 * with a `.valid` boolean, or null (the caller wraps the WASM parser).
 * `setStatus(msg)` receives progress strings.
 *
 * Resolves to `{ result, raw, valid }` — the first checksum-valid parse, or
 * the best parseable-but-invalid one, or `{ result: null }` if nothing
 * MRZ-shaped was found.
 */
export async function scanDocument(file, parse, setStatus) {
  setStatus('Loading image (downscaling locally)…');
  const bitmap = await createImageBitmap(file);
  const full = await toCanvas(bitmap);
  bitmap.close?.();

  // Ordered like the native retry passes: OCR-B model over band variants
  // (plain → contrast-stretched → binarized), then the full page, then the
  // generic model as a last resort.
  const band = () => bottomBand(full);
  const attempts = [
    ['mrz', 'Reading MRZ band (OCR-B model)…', band],
    ['mrz', 'Retrying with contrast stretch…', () => contrastStretched(band())],
    ['mrz', 'Retrying binarized…', () => otsuBinarized(band())],
    ['mrz', 'Scanning full image…', () => full],
    ['eng', 'Retrying with the general model…', () => contrastStretched(band())],
    ['eng', 'Scanning full image (general model)…', () => full],
  ];

  let best = null;
  let bestRaw = null;
  for (const [lang, msg, makeCanvas] of attempts) {
    setStatus(msg);
    let worker;
    try {
      worker = await getWorker(lang);
    } catch {
      continue; // model failed to load — try the next attempt
    }
    const { data } = await worker.recognize(makeCanvas());
    const parsed = parse(data.text);
    if (parsed) {
      if (parsed.valid) {
        setStatus('');
        return { result: parsed, raw: data.text, valid: true };
      }
      if (!best) { best = parsed; bestRaw = data.text; }
    }
  }
  return { result: best, raw: bestRaw, valid: false };
}
