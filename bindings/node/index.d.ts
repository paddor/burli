/// <reference types="node" />

import type { Transform } from "node:stream";

/** The `BROTLI_*` constants of `node:zlib`. */
export declare const constants: { readonly [name: `BROTLI_${string}`]: number };

export interface BrotliOptions {
  /** `BROTLI_OPERATION_*` run after each write. Default `BROTLI_OPERATION_PROCESS`. */
  flush?: number;
  /** `BROTLI_OPERATION_*` run at the end. Default `BROTLI_OPERATION_FINISH`. */
  finishFlush?: number;
  /** Largest decompressed chunk a stream emits. Default 16384. */
  chunkSize?: number;
  /** `BROTLI_PARAM_*` values. Qualities 6 to 11 run as 5; the default is 5. */
  params?: { [key: number]: boolean | number };
  /** Largest output of the one-shot functions. */
  maxOutputLength?: number;
}

export type InputType = string | ArrayBuffer | NodeJS.ArrayBufferView;
export type CompressCallback = (error: Error | null, result: Buffer) => void;

interface BrotliStream extends Transform {
  /** Input bytes written so far. */
  readonly bytesWritten: number;
  flush(kind?: number, callback?: () => void): void;
  flush(callback?: () => void): void;
  close(callback?: () => void): void;
  /** Starts a new stream. Call only between writes. */
  reset(): void;
}

export interface BrotliCompress extends BrotliStream {}
export interface BrotliDecompress extends BrotliStream {}

export declare const BrotliCompress: { new (options?: BrotliOptions): BrotliCompress };
export declare const BrotliDecompress: { new (options?: BrotliOptions): BrotliDecompress };

export declare function createBrotliCompress(options?: BrotliOptions): BrotliCompress;
export declare function createBrotliDecompress(options?: BrotliOptions): BrotliDecompress;

export declare function brotliCompressSync(buffer: InputType, options?: BrotliOptions): Buffer;
export declare function brotliDecompressSync(buffer: InputType, options?: BrotliOptions): Buffer;

export declare function brotliCompress(buffer: InputType, options: BrotliOptions, callback: CompressCallback): void;
export declare function brotliCompress(buffer: InputType, callback: CompressCallback): void;
export declare function brotliDecompress(buffer: InputType, options: BrotliOptions, callback: CompressCallback): void;
export declare function brotliDecompress(buffer: InputType, callback: CompressCallback): void;
