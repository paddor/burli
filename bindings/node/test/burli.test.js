"use strict";

// burli must decode streams of `node:zlib`, write streams it decodes, and
// follow the API of its Brotli functions.

const assert = require("node:assert/strict");
const { once } = require("node:events");
const { Readable, Transform } = require("node:stream");
const { pipeline } = require("node:stream/promises");
const { test } = require("node:test");
const { promisify } = require("node:util");
const zlib = require("node:zlib");

const burli = require("..");

const { constants } = zlib;
const quality = (value) => ({ params: { [constants.BROTLI_PARAM_QUALITY]: value } });

/** A seeded generator, so failures repeat. */
function random(seed) {
  let state = seed >>> 0 || 1;
  return () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return (state >>> 0) / 2 ** 32;
  };
}

function text(size) {
  const words = ["brotli ", "window ", "literal ", "context ", "copy ", "the\n"];
  const next = random(size);
  let out = "";
  while (out.length < size) {
    out += words[Math.floor(next() * words.length)];
  }
  return Buffer.from(out.slice(0, size));
}

function noise(size) {
  const next = random(size);
  return Buffer.from(Array.from({ length: size }, () => Math.floor(next() * 256)));
}

const payloads = {
  empty: Buffer.alloc(0),
  byte: Buffer.from("x"),
  text: text(100_000),
  noise: noise(50_000),
  zeros: Buffer.alloc(200_000),
  mixed: Buffer.concat([text(30_000), noise(20_000), text(30_000)]),
};

/** Splits `data` into pieces of 1 to `largest` bytes. */
function pieces(data, seed, largest) {
  const next = random(seed);
  const out = [];
  for (let pos = 0; pos < data.length; ) {
    const size = 1 + Math.floor(next() * largest);
    out.push(data.subarray(pos, pos + size));
    pos += size;
  }
  return out;
}

async function streamThrough(stream, chunks) {
  const out = [];
  await pipeline(Readable.from(chunks), stream, async function* (source) {
    for await (const chunk of source) {
      out.push(chunk);
    }
  });
  return Buffer.concat(out);
}

function catchError(run) {
  try {
    run();
  } catch (error) {
    return error;
  }
  assert.fail("expected an error");
}

test("node:zlib decodes burli output", () => {
  for (const [name, payload] of Object.entries(payloads)) {
    for (let level = 0; level <= 11; level++) {
      const encoded = burli.brotliCompressSync(payload, quality(level));
      assert.deepEqual(zlib.brotliDecompressSync(encoded), payload, `${name} at quality ${level}`);
    }
  }
});

test("compression takes Brotli parameters", () => {
  const payload = payloads.mixed;
  for (const [mode, lgwin, lgblock] of [
    [constants.BROTLI_MODE_TEXT, 16, 0],
    [constants.BROTLI_MODE_FONT, 10, 16],
    [constants.BROTLI_MODE_GENERIC, 24, 24],
    // Out of range values clamp, as in `node:zlib`.
    [7, 30, 5],
  ]) {
    const params = {
      [constants.BROTLI_PARAM_MODE]: mode,
      [constants.BROTLI_PARAM_QUALITY]: 4,
      [constants.BROTLI_PARAM_LGWIN]: lgwin,
      [constants.BROTLI_PARAM_LGBLOCK]: lgblock,
      [constants.BROTLI_PARAM_SIZE_HINT]: payload.length,
    };
    assert.deepEqual(zlib.brotliDecompressSync(burli.brotliCompressSync(payload, { params })), payload);
  }
});

test("decodes node:zlib output", async () => {
  for (const [name, payload] of Object.entries(payloads)) {
    for (let level = 0; level <= 11; level++) {
      for (const lgwin of [10, 22]) {
        const data = level >= 10 ? payload.subarray(0, 10_000) : payload;
        const params = { [constants.BROTLI_PARAM_QUALITY]: level, [constants.BROTLI_PARAM_LGWIN]: lgwin };
        const encoded = zlib.brotliCompressSync(data, { params });
        assert.deepEqual(burli.brotliDecompressSync(encoded), data, `${name} at quality ${level}`);
        const decoded = await streamThrough(burli.createBrotliDecompress(), pieces(encoded, level, 999));
        assert.deepEqual(decoded, data);
      }
    }
  }
});

test("callback functions run on the thread pool", async () => {
  const payload = payloads.mixed;
  let ticks = 0;
  const timer = setInterval(() => ticks++, 0);
  const encoded = await promisify(burli.brotliCompress)(Buffer.concat(Array(40).fill(payload)));
  clearInterval(timer);
  assert.ok(ticks > 0);
  assert.deepEqual(await promisify(burli.brotliDecompress)(encoded), Buffer.concat(Array(40).fill(payload)));
  const decoded = await new Promise((resolve, reject) => {
    burli.brotliDecompress(zlib.brotliCompressSync(payload), {}, (error, result) => (error ? reject(error) : resolve(result)));
  });
  assert.deepEqual(decoded, payload);
});

test("streams round trip in pieces", async () => {
  const payload = payloads.mixed;
  for (const level of [0, 1, 5, 11]) {
    const encoded = await streamThrough(burli.createBrotliCompress(quality(level)), pieces(payload, 1, 5000));
    assert.deepEqual(zlib.brotliDecompressSync(encoded), payload);
    assert.deepEqual(await streamThrough(new burli.BrotliDecompress(), pieces(encoded, 2, 300)), payload);
  }
});

test("decompression streams emit at most chunkSize bytes at a time", async () => {
  const payload = Buffer.concat([payloads.zeros, payloads.text]);
  const encoded = zlib.brotliCompressSync(payload);
  for (const chunkSize of [64, 1000, 65536]) {
    const stream = burli.createBrotliDecompress({ chunkSize });
    const sizes = [];
    stream.on("data", (chunk) => sizes.push(chunk.length));
    const decoded = await streamThrough(stream, [encoded]);
    assert.deepEqual(decoded, payload);
    assert.ok(Math.max(...sizes) <= chunkSize);
    assert.equal(stream.bytesWritten, encoded.length);
  }
});

// Without a working flush, these tests time out waiting for output.
test("flush makes the input so far decodable", { timeout: 10_000 }, async () => {
  const payload = payloads.text;
  const compressor = burli.createBrotliCompress();
  const decompressor = zlib.createBrotliDecompress();
  const decoded = [];
  let decodedLength = 0;
  decompressor.on("data", (chunk) => {
    decoded.push(chunk);
    decodedLength += chunk.length;
  });
  compressor.pipe(decompressor);
  let written = 0;
  for (const piece of pieces(payload, 3, 20_000)) {
    compressor.write(piece);
    written += piece.length;
    await new Promise((resolve) => compressor.flush(resolve));
    while (decodedLength < written) {
      await once(decompressor, "data");
    }
    assert.deepEqual(Buffer.concat(decoded), payload.subarray(0, written));
  }
  compressor.end();
  await new Promise((resolve) => decompressor.on("end", resolve));
  assert.deepEqual(Buffer.concat(decoded), payload);
  assert.equal(compressor.bytesWritten, payload.length);
});

test("flush option flushes after each write", { timeout: 10_000 }, async () => {
  const compressor = burli.createBrotliCompress({ flush: constants.BROTLI_OPERATION_FLUSH });
  const chunks = [];
  compressor.on("data", (chunk) => chunks.push(chunk));
  await new Promise((resolve) => compressor.write("hello ", resolve));
  const decompressor = zlib.createBrotliDecompress();
  const partial = await new Promise((resolve) => {
    decompressor.once("data", resolve);
    decompressor.write(Buffer.concat(chunks));
  });
  assert.equal(partial.toString(), "hello ");
  compressor.destroy();
  decompressor.destroy();
});

test("rejects invalid options as node:zlib does", () => {
  const cases = [
    { params: { 9999: 1 } },
    { params: { quality: 5 } },
    { params: { [constants.BROTLI_PARAM_QUALITY]: "5" } },
    { chunkSize: 10 },
    { chunkSize: "big" },
    { flush: 99 },
    { finishFlush: -1 },
    { maxOutputLength: 0 },
  ];
  for (const options of cases) {
    for (const run of [
      (module) => module.brotliCompressSync("x", options),
      (module) => module.createBrotliCompress(options),
      (module) => module.createBrotliDecompress(options),
    ]) {
      const [ours, theirs] = [burli, zlib].map((module) => catchError(() => run(module)));
      assert.equal(ours.code, theirs.code, JSON.stringify(options));
      assert.equal(ours.constructor, theirs.constructor, JSON.stringify(options));
    }
  }
  for (const input of [123, -0, NaN, true, null, undefined, {}, Object.create(null), () => {}, Symbol("s"), 2n ** 100n]) {
    const [ours, theirs] = [burli, zlib].map((module) => catchError(() => module.brotliCompressSync(input)));
    assert.equal(ours.code, theirs.code);
    assert.equal(ours.message, theirs.message);
  }
  assert.throws(() => burli.brotliCompress("x", {}), { code: "ERR_INVALID_ARG_TYPE" });
});

test("takes the inputs node:zlib takes", () => {
  const payload = payloads.text.subarray(0, 5000);
  const arrayBuffer = payload.buffer.slice(payload.byteOffset, payload.byteOffset + payload.length);
  for (const input of [
    payload,
    payload.toString("latin1"),
    new Uint8Array(arrayBuffer),
    new DataView(arrayBuffer),
    new Uint16Array(arrayBuffer),
    arrayBuffer,
  ]) {
    const expected = zlib.brotliDecompressSync(zlib.brotliCompressSync(input));
    assert.deepEqual(zlib.brotliDecompressSync(burli.brotliCompressSync(input)), expected);
  }
  const encoded = zlib.brotliCompressSync(payload);
  const encodedBuffer = encoded.buffer.slice(encoded.byteOffset, encoded.byteOffset + encoded.length);
  for (const input of [new Uint8Array(encodedBuffer), new DataView(encodedBuffer), encodedBuffer]) {
    assert.deepEqual(burli.brotliDecompressSync(input), payload);
  }
});

test("truncated streams fail with Z_BUF_ERROR", async () => {
  const encoded = zlib.brotliCompressSync(payloads.text);
  for (const end of [0, 1, 5, encoded.length - 1]) {
    const truncated = encoded.subarray(0, end);
    assert.throws(() => burli.brotliDecompressSync(truncated), { code: "Z_BUF_ERROR", message: "unexpected end of file" });
    await assert.rejects(promisify(burli.brotliDecompress)(truncated), { code: "Z_BUF_ERROR" });
    await assert.rejects(streamThrough(burli.createBrotliDecompress(), [truncated]), { code: "Z_BUF_ERROR", errno: -5 });
  }
});

test("malformed streams fail", async () => {
  // Reserved bits set, and a large-window header.
  for (const corrupt of [Buffer.from([0xff, 0xff, 0xff]), Buffer.from([0x11, 0x00])]) {
    const expected = { code: "ERR_BURLI_DECOMPRESSION_FAILED", message: /^Decompression failed: / };
    assert.throws(() => burli.brotliDecompressSync(corrupt), expected);
    await assert.rejects(promisify(burli.brotliDecompress)(corrupt), expected);
    await assert.rejects(streamThrough(burli.createBrotliDecompress(), [corrupt]), expected);
  }
});

test("rejects data after the stream", async () => {
  const encoded = zlib.brotliCompressSync("payload");
  const expected = { code: "ERR_BURLI_DECOMPRESSION_FAILED" };
  const trailing = Buffer.concat([encoded, Buffer.from("x")]);
  assert.throws(() => burli.brotliDecompressSync(trailing), expected);
  await assert.rejects(streamThrough(burli.createBrotliDecompress(), [trailing]), expected);
  await assert.rejects(streamThrough(burli.createBrotliDecompress(), [encoded, Buffer.from("x")]), expected);
});

test("maxOutputLength bounds one-shot output as in node:zlib", async () => {
  const encoded = zlib.brotliCompressSync(Buffer.alloc(100_000));
  const [ours, theirs] = [burli, zlib].map((module) =>
    catchError(() => module.brotliDecompressSync(encoded, { maxOutputLength: 1000 })),
  );
  assert.equal(ours.code, theirs.code);
  assert.equal(ours.message, theirs.message);
  assert.ok(ours instanceof RangeError);
  await assert.rejects(promisify(burli.brotliDecompress)(encoded, { maxOutputLength: 1000 }), { code: "ERR_BUFFER_TOO_LARGE" });
  assert.equal(burli.brotliDecompressSync(encoded, { maxOutputLength: 100_000 }).length, 100_000);
});

test("callback errors are uncaught exceptions", async () => {
  const rejections = [];
  const onRejection = (reason) => rejections.push(reason);
  process.on("unhandledRejection", onRejection);
  try {
    const thrown = new Promise((resolve) => process.setUncaughtExceptionCaptureCallback(resolve));
    burli.brotliCompress("x", () => {
      throw new Error("from callback");
    });
    assert.equal((await thrown).message, "from callback");
    assert.deepEqual(rejections, []);
  } finally {
    process.setUncaughtExceptionCaptureCallback(null);
    process.off("unhandledRejection", onRejection);
  }
});

test("matches node:zlib names", () => {
  for (const name of Object.keys(burli)) {
    assert.ok(name in zlib, name);
  }
  for (const [name, value] of Object.entries(burli.constants)) {
    assert.equal(value, zlib.constants[name]);
  }
  assert.ok(burli.createBrotliCompress() instanceof Transform);
});
