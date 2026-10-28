// Round trips through the installed package and System.IO.Compression.

using System.IO.Compression;

byte[] data = System.Text.Encoding.ASCII.GetBytes(string.Concat(Enumerable.Repeat("burli smoke test ", 1000)));

var compressed = new MemoryStream();
using (var stream = new Burli.BrotliStream(compressed, CompressionLevel.Optimal, leaveOpen: true))
{
    stream.Write(data);
}
var decoded = new MemoryStream();
new System.IO.Compression.BrotliStream(new MemoryStream(compressed.ToArray()), CompressionMode.Decompress).CopyTo(decoded);
if (!decoded.ToArray().AsSpan().SequenceEqual(data))
{
    throw new Exception("System.IO.Compression did not decode burli output");
}

var encoded = new byte[System.IO.Compression.BrotliEncoder.GetMaxCompressedLength(data.Length)];
System.IO.Compression.BrotliEncoder.TryCompress(data, encoded, out int length);
var output = new byte[data.Length];
if (!Burli.BrotliDecoder.TryDecompress(encoded.AsSpan(0, length), output, out int written) || !output.AsSpan(0, written).SequenceEqual(data))
{
    throw new Exception("burli did not decode System.IO.Compression output");
}
Console.WriteLine($"Burli {typeof(Burli.BrotliStream).Assembly.GetName().Version}: round trips OK");
