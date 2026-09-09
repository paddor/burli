/**
 * @module
 *
 * Pure Rust Brotli codec compiled to WebAssembly. The decoder accepts standard
 * Brotli streams. The encoder supports qualities 0 through 5.
 *
 * ```ts
 * import { compress, decompress, init } from "@paddor/burli";
 *
 * await init();
 *
 * const data = new TextEncoder().encode("hello world".repeat(1000));
 * const compressed = compress(data);
 * const original = decompress(compressed);
 * ```
 *
 * Use a reusable compressor or decompressor when processing many messages:
 *
 * ```ts
 * const compressor = new Compressor(4);
 * const compressed = compressor.compress(data);
 * compressor.free();
 *
 * const decompressor = new Decompressor();
 * const original = decompressor.decompress(compressed);
 * decompressor.free();
 * ```
 */

import * as wasmBindings from "./pkg/burli_wasm_bg.js";
import type * as Wasm from "./pkg/burli_wasm.js";

// Use generated types for the bindings, including dynamically added methods
// such as Symbol.dispose. The type-only import does not initialize WASM.
const {
  compress: wasmCompress,
  Compressor: WasmCompressor,
  decompress: wasmDecompress,
  Decompressor: WasmDecompressor,
} = wasmBindings as unknown as typeof Wasm;

/** Default Brotli encoder quality. */
export const DEFAULT_QUALITY = 5;
const MAX_QUALITY = 5;
const MAX_WASM_USIZE = 0xffff_ffff;

/** Options for Brotli compression. */
export interface CompressOptions {
  /** Encoder quality from 0 (fastest) through 5 (best ratio). Default: 5. */
  quality?: number;
}

/** Options for Brotli decompression. */
export interface DecompressOptions {
  /** Maximum decoded bytes. Omit for no practical limit. */
  maxDecompressedSize?: number;
}

function quality(value: number): number {
  if (!Number.isInteger(value) || value < 0 || value > MAX_QUALITY) {
    throw new RangeError("quality must be an integer from 0 through 5");
  }
  return value;
}

function maxDecompressedSize(options?: DecompressOptions): number | undefined {
  const max = options?.maxDecompressedSize;
  if (max === undefined) return undefined;
  if (!Number.isSafeInteger(max) || max < 0 || max > MAX_WASM_USIZE) {
    throw new RangeError(
      "maxDecompressedSize must be an integer from 0 to 4294967295",
    );
  }
  return max;
}

let initialized = false;
let initialization: Promise<void> | undefined;

function finishInitialization(wasm: Record<string, unknown>): void {
  // A synchronous caller may have initialized while the import was pending.
  if (initialized) return;
  wasmBindings.__wbg_set_wasm(wasm);
  // wasm-bindgen emits this initializer when the module needs startup work.
  if (typeof wasm.__wbindgen_start === "function") wasm.__wbindgen_start();
  initialized = true;
}

/**
 * Initialize the WASM module. Must be called before compression or decoding.
 */
export function init(): Promise<void> {
  if (initialized) return Promise.resolve();
  if (initialization) return initialization;

  initialization = (async () => {
    // Literal imports let bundlers include WASM and its generated JS bindings.
    const wasm = await import("./pkg/burli_wasm_bg.wasm");
    finishInitialization(wasm);
  })().catch((error) => {
    initialization = undefined;
    throw error;
  });
  return initialization;
}

/** Initialize synchronously from preloaded WASM bytes. */
export function initSyncFromBytes(bytes: BufferSource): void {
  if (initialized) return;
  const module = new WebAssembly.Module(bytes);
  const instance = new WebAssembly.Instance(module, {
    "./burli_wasm_bg.js": wasmBindings,
  });
  finishInitialization(instance.exports);
}

/** Compress a Brotli stream. */
export function compress(
  input: Uint8Array,
  options?: CompressOptions,
): Uint8Array {
  return wasmCompress(input, quality(options?.quality ?? DEFAULT_QUALITY));
}

/** Decompress a Brotli stream. */
export function decompress(
  input: Uint8Array,
  options?: DecompressOptions,
): Uint8Array {
  return wasmDecompress(input, maxDecompressedSize(options));
}

const compressorInner = new WeakMap<Compressor, Wasm.Compressor>();

function getCompressorInner(compressor: Compressor): Wasm.Compressor {
  const inner = compressorInner.get(compressor);
  if (!inner) throw new TypeError("invalid or freed Compressor");
  return inner;
}

/** Reusable compression context for repeated messages at one quality. */
export class Compressor {
  constructor(qualityValue = DEFAULT_QUALITY) {
    compressorInner.set(
      this,
      new WasmCompressor(quality(qualityValue)),
    );
  }

  compress(input: Uint8Array): Uint8Array {
    return getCompressorInner(this).compress(input);
  }

  free(): void {
    const inner = compressorInner.get(this);
    if (!inner) return;
    compressorInner.delete(this);
    inner.free();
  }

  [Symbol.dispose](): void {
    this.free();
  }
}

const decompressorInner = new WeakMap<Decompressor, Wasm.Decompressor>();

function getDecompressorInner(decompressor: Decompressor): Wasm.Decompressor {
  const inner = decompressorInner.get(decompressor);
  if (!inner) throw new TypeError("invalid or freed Decompressor");
  return inner;
}

/** Reusable decompression context for repeated messages. */
export class Decompressor {
  constructor(private readonly defaultOptions?: DecompressOptions) {
    maxDecompressedSize(defaultOptions);
    decompressorInner.set(this, new WasmDecompressor());
  }

  decompress(
    input: Uint8Array,
    options?: DecompressOptions,
  ): Uint8Array {
    const max = maxDecompressedSize(options ?? this.defaultOptions);
    return getDecompressorInner(this).decompress(input, max);
  }

  free(): void {
    const inner = decompressorInner.get(this);
    if (!inner) return;
    decompressorInner.delete(this);
    inner.free();
  }

  [Symbol.dispose](): void {
    this.free();
  }
}
