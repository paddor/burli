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

import {
  compress as wasmCompress,
  Compressor as WasmCompressor,
  decompress as wasmDecompress,
  Decompressor as WasmDecompressor,
  initSync,
} from "./pkg/burli_wasm.js";

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

/** Initialize the WASM module. Must be called before compression or decoding. */
export function init(): Promise<void> {
  if (initialized) return Promise.resolve();
  if (initialization) return initialization;

  initialization = (async () => {
    const wasmUrl = new URL("./pkg/burli_wasm_bg.wasm", import.meta.url);
    const response = await fetch(wasmUrl);
    if (!response.ok) {
      throw new Error(`failed to load Bürli WASM: ${response.status}`);
    }
    const bytes = await response.arrayBuffer();
    initSync({ module: new WebAssembly.Module(bytes) });
    initialized = true;
  })().catch((error) => {
    initialization = undefined;
    throw error;
  });
  return initialization;
}

/** Initialize synchronously from preloaded WASM bytes. */
export function initSyncFromBytes(bytes: BufferSource): void {
  if (initialized) return;
  initSync({ module: new WebAssembly.Module(bytes) });
  initialized = true;
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

const compressorInner = new WeakMap<Compressor, WasmCompressor>();

function getCompressorInner(compressor: Compressor): WasmCompressor {
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

const decompressorInner = new WeakMap<Decompressor, WasmDecompressor>();

function getDecompressorInner(decompressor: Decompressor): WasmDecompressor {
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
