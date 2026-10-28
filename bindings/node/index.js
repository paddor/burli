"use strict";

// Memory-safe Brotli compression and decompression, with the API of the
// Brotli functions of `node:zlib`. Qualities 6 to 11 run as 5.

const { kMaxLength } = require("node:buffer");
const { Transform, finished } = require("node:stream");
const { inspect } = require("node:util");
const zlib = require("node:zlib");

const native = loadNative();

/** The `BROTLI_*` constants of `node:zlib`. */
const constants = Object.freeze(
  Object.fromEntries(Object.entries(zlib.constants).filter(([name]) => name.startsWith("BROTLI_"))),
);

const {
  BROTLI_OPERATION_PROCESS,
  BROTLI_OPERATION_FLUSH,
  BROTLI_OPERATION_FINISH,
  BROTLI_OPERATION_EMIT_METADATA,
  BROTLI_PARAM_MODE,
  BROTLI_PARAM_QUALITY,
  BROTLI_PARAM_LGWIN,
  BROTLI_PARAM_LGBLOCK,
  BROTLI_PARAM_NDIRECT,
  BROTLI_DECODER_PARAM_DISABLE_RING_BUFFER_REALLOCATION,
  BROTLI_DECODER_PARAM_LARGE_WINDOW,
  BROTLI_MODE_GENERIC,
  BROTLI_DEFAULT_WINDOW,
  BROTLI_MIN_WINDOW_BITS,
  BROTLI_MAX_WINDOW_BITS,
  BROTLI_MIN_INPUT_BLOCK_BITS,
  BROTLI_MAX_INPUT_BLOCK_BITS,
  BROTLI_MAX_QUALITY,
} = constants;

/** The highest quality the encoder implements. Higher ones run as this. */
const DEFAULT_QUALITY = 5;
const EMPTY = Buffer.alloc(0);

// Empty chunks that `flush()` writes; they carry their flush kind.
const kFlushFlag = Symbol("kFlushFlag");
const flushBuffers = [];
for (let kind = BROTLI_OPERATION_PROCESS; kind <= BROTLI_OPERATION_EMIT_METADATA; kind++) {
  flushBuffers[kind] = Buffer.alloc(0);
  flushBuffers[kind][kFlushFlag] = kind;
}

function codeError(Base, code, message, properties) {
  const error = new Base(message);
  error.code = code;
  return Object.assign(error, properties);
}

/** Describes `value` as Node does in `ERR_INVALID_ARG_TYPE` messages. */
function describe(value) {
  if (value === null || value === undefined) {
    return String(value);
  }
  if (typeof value === "function") {
    return `function ${value.name}`;
  }
  if (typeof value === "object") {
    return value.constructor?.name ? `an instance of ${value.constructor.name}` : inspect(value, { depth: -1 });
  }
  if (typeof value === "string") {
    const shown = value.length > 28 ? `${value.slice(0, 25)}...` : value;
    return `type string (${shown.includes("'") ? JSON.stringify(shown) : `'${shown}'`})`;
  }
  // Numbers, booleans, bigints, and symbols.
  return `type ${typeof value} (${inspect(value)})`;
}

function invalidType(name, expected, value) {
  const received = describe(value);
  return codeError(
    TypeError,
    "ERR_INVALID_ARG_TYPE",
    `The "${name}" argument must be ${expected}. Received ${received}`,
  );
}

function outOfRange(name, range, value) {
  return codeError(
    RangeError,
    "ERR_OUT_OF_RANGE",
    `The value of "${name}" is out of range. It must be ${range}. Received ${value}`,
  );
}

/** Turns a native error, whose message starts with a code, into a Node one. */
function nodeError(error, maxOutputLength) {
  const match = /^([A-Z0-9_]+): (.*)$/s.exec(error?.message ?? "");
  if (!match) {
    return error;
  }
  const [, code, detail] = match;
  switch (code) {
    case "Z_BUF_ERROR":
      return codeError(Error, code, detail, { errno: zlib.constants.Z_BUF_ERROR });
    case "ERR_BUFFER_TOO_LARGE":
      return codeError(RangeError, code, `Cannot create a Buffer larger than ${maxOutputLength} bytes`);
    case "ERR_BURLI_DECOMPRESSION_FAILED":
      return codeError(Error, code, `Decompression failed: ${detail}`);
    default:
      return codeError(Error, code, detail);
  }
}

function truncatedError() {
  return codeError(Error, "Z_BUF_ERROR", "unexpected end of file", { errno: zlib.constants.Z_BUF_ERROR });
}

/** Views `buffer` as bytes, as `node:zlib` accepts it. */
function toBytes(buffer) {
  if (typeof buffer === "string") {
    return Buffer.from(buffer);
  }
  if (ArrayBuffer.isView(buffer)) {
    return new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
  }
  if (buffer instanceof ArrayBuffer || buffer instanceof SharedArrayBuffer) {
    return new Uint8Array(buffer);
  }
  throw invalidType(
    "buffer",
    "of type string or an instance of Buffer, TypedArray, DataView, or ArrayBuffer",
    buffer,
  );
}

function integerOption(options, name, min, max, fallback) {
  const value = options[name];
  if (value === undefined) {
    return fallback;
  }
  if (typeof value !== "number") {
    throw invalidType(`options.${name}`, "of type number", value);
  }
  if (!Number.isFinite(value) || value < min || value > max) {
    throw outOfRange(`options.${name}`, `>= ${min} and <= ${max}`, value);
  }
  return value;
}

/** Reads the options that `node:zlib` takes for Brotli. */
function settings(options) {
  options = options ?? {};
  if (options.info) {
    throw codeError(TypeError, "ERR_BURLI_UNSUPPORTED", "burli does not support options.info");
  }
  const operation = [BROTLI_OPERATION_PROCESS, BROTLI_OPERATION_EMIT_METADATA];
  const params = new Map();
  for (const key of Object.keys(options.params ?? {})) {
    const number = Number(key);
    if (!Number.isInteger(number) || number < 0 || number > BROTLI_PARAM_NDIRECT) {
      throw codeError(RangeError, "ERR_BROTLI_INVALID_PARAM", `${key} is not a valid Brotli parameter`);
    }
    const value = options.params[key];
    if (typeof value !== "number" && typeof value !== "boolean") {
      throw invalidType("options.params[key]", "of type number", value);
    }
    // As the Uint32Array that `node:zlib` hands to Brotli.
    params.set(number, Number(value) >>> 0);
  }
  return {
    flush: integerOption(options, "flush", ...operation, BROTLI_OPERATION_PROCESS),
    finishFlush: integerOption(options, "finishFlush", ...operation, BROTLI_OPERATION_FINISH),
    chunkSize: integerOption(options, "chunkSize", 64, Infinity, zlib.constants.Z_DEFAULT_CHUNK),
    maxOutputLength: integerOption(options, "maxOutputLength", 1, kMaxLength, kMaxLength),
    params,
  };
}

const clamp = (value, min, max) => Math.min(Math.max(value, min), max);

/** Encoder parameters, clamped as Brotli clamps them. Other parameters are
 * hints that burli ignores. */
function encoderParams({ params }) {
  const lgblock = params.get(BROTLI_PARAM_LGBLOCK) ?? 0;
  return [
    params.get(BROTLI_PARAM_MODE) ?? BROTLI_MODE_GENERIC,
    Math.min(params.get(BROTLI_PARAM_QUALITY) ?? DEFAULT_QUALITY, BROTLI_MAX_QUALITY),
    clamp(params.get(BROTLI_PARAM_LGWIN) ?? BROTLI_DEFAULT_WINDOW, BROTLI_MIN_WINDOW_BITS, BROTLI_MAX_WINDOW_BITS),
    lgblock === 0 ? 0 : clamp(lgblock, BROTLI_MIN_INPUT_BLOCK_BITS, BROTLI_MAX_INPUT_BLOCK_BITS),
  ];
}

/** Checks decoder parameters; burli needs none of them. */
function checkDecoderParams({ params }) {
  for (const key of params.keys()) {
    if (key !== BROTLI_DECODER_PARAM_DISABLE_RING_BUFFER_REALLOCATION && key !== BROTLI_DECODER_PARAM_LARGE_WINDOW) {
      throw codeError(Error, "ERR_ZLIB_INITIALIZATION_FAILED", "Initialization failed");
    }
  }
}

function brotliCompressSync(buffer, options) {
  const bytes = toBytes(buffer);
  const params = encoderParams(settings(options));
  try {
    return native.compressSync(bytes, ...params);
  } catch (error) {
    throw nodeError(error);
  }
}

function brotliDecompressSync(buffer, options) {
  const bytes = toBytes(buffer);
  const setup = settings(options);
  checkDecoderParams(setup);
  try {
    return native.decompressSync(bytes, setup.maxOutputLength);
  } catch (error) {
    throw nodeError(error, setup.maxOutputLength);
  }
}

/** Calls back outside the promise chain, so errors thrown there surface. */
function settle(promise, callback, maxOutputLength) {
  promise.then(
    (result) => process.nextTick(callback, null, result),
    (error) => process.nextTick(callback, nodeError(error, maxOutputLength)),
  );
}

function callbackArguments(options, callback) {
  if (typeof options === "function") {
    return [{}, options];
  }
  if (typeof callback !== "function") {
    throw invalidType("callback", "of type function", callback);
  }
  return [options, callback];
}

/** Compresses on the libuv thread pool. */
function brotliCompress(buffer, options, callback) {
  [options, callback] = callbackArguments(options, callback);
  const bytes = toBytes(buffer);
  const params = encoderParams(settings(options));
  settle(native.compress(bytes, ...params), callback);
}

/** Decompresses on the libuv thread pool. */
function brotliDecompress(buffer, options, callback) {
  [options, callback] = callbackArguments(options, callback);
  const bytes = toBytes(buffer);
  const setup = settings(options);
  checkDecoderParams(setup);
  settle(native.decompress(bytes, setup.maxOutputLength), callback, setup.maxOutputLength);
}

/** Rethrows outside the promise chain an error from a stream callback. */
function rethrow(error) {
  process.nextTick(() => {
    throw error;
  });
}

/** What `BrotliCompress` and `BrotliDecompress` share with `node:zlib`. */
class BrotliBase extends Transform {
  constructor(options, setup) {
    super({ autoDestroy: true, ...options });
    this.bytesWritten = 0;
    this._chunkSize = setup.chunkSize;
    this._defaultFlushFlag = setup.flush;
    this._finishFlushFlag = setup.finishFlush;
  }

  /** Writes out the input so far. `kind` is a `BROTLI_OPERATION_*` value. */
  flush(kind, callback) {
    if (typeof kind === "function" || (kind === undefined && !callback)) {
      callback = kind;
      kind = BROTLI_OPERATION_FLUSH;
    }
    if (this.writableFinished) {
      if (callback) {
        process.nextTick(callback);
      }
    } else if (this.writableEnded) {
      if (callback) {
        this.once("end", callback);
      }
    } else {
      this.write(flushBuffers[kind], "", callback);
    }
  }

  close(callback) {
    if (callback) {
      finished(this, callback);
    }
    this.destroy();
  }

  _transform(chunk, _encoding, callback) {
    const kind = chunk[kFlushFlag];
    if (kind === undefined) {
      this.bytesWritten += chunk.length;
    }
    this._process(chunk, kind ?? this._defaultFlushFlag, callback);
  }
}

/** Compresses what is written, on the libuv thread pool. */
class BrotliCompress extends BrotliBase {
  constructor(options) {
    const setup = settings(options);
    const params = encoderParams(setup);
    super(options, setup);
    this._params = params;
    this._native = new native.Compressor(...params);
  }

  /** Starts a new stream. Call only between writes. */
  reset() {
    this._native = new native.Compressor(...this._params);
  }

  _process(chunk, operation, callback) {
    this._native
      .write(chunk, operation)
      .then(
        (output) => {
          if (output.length > 0) {
            this.push(output);
          }
          callback();
        },
        (error) => callback(nodeError(error)),
      )
      .catch(rethrow);
  }

  _flush(callback) {
    this._process(EMPTY, this._finishFlushFlag, callback);
  }
}

/** Decompresses what is written, on the libuv thread pool. */
class BrotliDecompress extends BrotliBase {
  constructor(options) {
    const setup = settings(options);
    checkDecoderParams(setup);
    super(options, setup);
    this._native = new native.Decompressor();
  }

  /** Starts a new stream. Call only between writes. */
  reset() {
    this._native = new native.Decompressor();
  }

  // Each step returns at most `chunkSize` bytes, as in `node:zlib`.
  _process(chunk, operation, callback) {
    this._native
      .write(chunk, this._chunkSize)
      .then(
        ({ output, more }) => {
          if (output.length > 0) {
            this.push(output);
          }
          if (more) {
            this._process(EMPTY, operation, callback);
          } else {
            callback();
          }
        },
        (error) => callback(nodeError(error)),
      )
      .catch(rethrow);
  }

  _flush(callback) {
    callback(this._native.finished ? null : truncatedError());
  }
}

function createBrotliCompress(options) {
  return new BrotliCompress(options);
}

function createBrotliDecompress(options) {
  return new BrotliDecompress(options);
}

function loadNative() {
  const errors = [];
  for (const name of ["./burli.node", platformPackage()]) {
    if (name === undefined) {
      continue;
    }
    try {
      return require(name);
    } catch (error) {
      errors.push(error);
    }
  }
  throw new Error(`burli has no native module for ${process.platform}-${process.arch}`, {
    cause: new AggregateError(errors),
  });
}

function platformPackage() {
  const { platform, arch } = process;
  if (!["darwin", "linux", "win32"].includes(platform) || !["arm64", "x64"].includes(arch)) {
    return undefined;
  }
  let abi = "";
  if (platform === "linux") {
    process.report.excludeNetwork = true;
    abi = process.report.getReport().header.glibcVersionRuntime ? "-gnu" : "-musl";
  } else if (platform === "win32") {
    abi = "-msvc";
  }
  return `@paddor/burli-${platform}-${arch}${abi}`;
}

module.exports = {
  BrotliCompress,
  BrotliDecompress,
  brotliCompress,
  brotliCompressSync,
  brotliDecompress,
  brotliDecompressSync,
  constants,
  createBrotliCompress,
  createBrotliDecompress,
};
