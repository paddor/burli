import {
  assert,
  assertEquals,
  assertThrows,
} from "jsr:@std/assert";
import {
  compress,
  Compressor,
  decompress,
  Decompressor,
  init,
} from "./mod.ts";

const encoder = new TextEncoder();

Deno.test("one-shot round-trip", async () => {
  await init();
  const data = encoder.encode("hello Brotli".repeat(500));
  const compressed = compress(data);
  assert(compressed.length < data.length);
  assertEquals(decompress(compressed), data);
});

Deno.test("quality validation", async () => {
  await init();
  assertThrows(() => compress(new Uint8Array(), { quality: -1 }), RangeError);
  assertThrows(() => compress(new Uint8Array(), { quality: 6 }), RangeError);
});

Deno.test("decompression limit", async () => {
  await init();
  const data = encoder.encode("bounded Brotli".repeat(100));
  const compressed = compress(data);
  assertThrows(
    () => decompress(compressed, { maxDecompressedSize: data.length - 1 }),
    Error,
  );
  assertEquals(
    decompress(compressed, { maxDecompressedSize: data.length }),
    data,
  );
});

Deno.test("reusable contexts", async () => {
  await init();
  const compressor = new Compressor(4);
  const decompressor = new Decompressor();
  const data1 = encoder.encode("first message".repeat(100));
  const data2 = encoder.encode("second message".repeat(100));
  const compressed1 = compressor.compress(data1);
  const compressed2 = compressor.compress(data2);
  assertEquals(decompressor.decompress(compressed1), data1);
  assertEquals(decompressor.decompress(compressed2), data2);
  compressor.free();
  decompressor.free();
});

Deno.test("empty input", async () => {
  await init();
  assertEquals(decompress(compress(new Uint8Array())), new Uint8Array());
});
