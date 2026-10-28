// Burli must decode streams of System.IO.Compression, write streams it
// decodes, and follow the API of its Brotli types.

using System.Buffers;
using System.IO.Compression;
using System.Reflection;
using Sys = System.IO.Compression;

int failures = 0;
foreach (MethodInfo test in typeof(Tests).GetMethods(BindingFlags.Public | BindingFlags.Static))
{
    try
    {
        if (test.Invoke(null, null) is Task task)
        {
            await task;
        }
        Console.WriteLine($"ok   {test.Name}");
    }
    catch (Exception error)
    {
        failures++;
        Console.WriteLine($"FAIL {test.Name}: {(error as TargetInvocationException)?.InnerException ?? error}");
    }
}
Console.WriteLine(failures == 0 ? "all passed" : $"{failures} failed");
return failures == 0 ? 0 : 1;

static class Payloads
{
    public static byte[] Text(int size)
    {
        string[] words = ["brotli ", "window ", "literal ", "context ", "copy ", "the\n"];
        var random = new Random(size);
        var text = new System.Text.StringBuilder();
        while (text.Length < size)
        {
            text.Append(words[random.Next(words.Length)]);
        }
        return System.Text.Encoding.ASCII.GetBytes(text.ToString(0, size));
    }

    public static byte[] Noise(int size)
    {
        var bytes = new byte[size];
        new Random(size).NextBytes(bytes);
        return bytes;
    }

    public static readonly Dictionary<string, byte[]> All = new()
    {
        ["empty"] = [],
        ["byte"] = "x"u8.ToArray(),
        ["text"] = Text(100_000),
        ["noise"] = Noise(50_000),
        ["zeros"] = new byte[200_000],
        ["mixed"] = [.. Text(30_000), .. Noise(20_000), .. Text(30_000)],
    };

    /// <summary>Splits <paramref name="data"/> into pieces of 1 to <paramref name="largest"/> bytes.</summary>
    public static IEnumerable<ReadOnlyMemory<byte>> Pieces(byte[] data, int seed, int largest)
    {
        var random = new Random(seed);
        for (int pos = 0; pos < data.Length;)
        {
            int size = Math.Min(random.Next(1, largest + 1), data.Length - pos);
            yield return data.AsMemory(pos, size);
            pos += size;
        }
    }
}

static class Check
{
    public static void True(bool condition, string what = "condition")
    {
        if (!condition)
        {
            throw new Exception($"expected {what}");
        }
    }

    public static void Equal<T>(T expected, T actual, string what = "value")
    {
        if (!EqualityComparer<T>.Default.Equals(expected, actual))
        {
            throw new Exception($"expected {what} {expected}, got {actual}");
        }
    }

    public static void Bytes(ReadOnlySpan<byte> expected, ReadOnlySpan<byte> actual, string what = "bytes")
    {
        if (!expected.SequenceEqual(actual))
        {
            throw new Exception($"{what} differ: expected {expected.Length} bytes, got {actual.Length}");
        }
    }

    public static Exception Throws(Action action)
    {
        try
        {
            action();
        }
        catch (Exception error)
        {
            return error;
        }
        throw new Exception("expected an exception");
    }

    /// <summary>Checks that Burli and System.IO.Compression throw the same exception type.</summary>
    public static void SameThrow(Action burli, Action system) =>
        Equal(Throws(system).GetType(), Throws(burli).GetType(), "exception");
}

static class Codec
{
    public static byte[] SysCompress(byte[] data, int quality, int window = 22)
    {
        var output = new byte[Sys.BrotliEncoder.GetMaxCompressedLength(data.Length)];
        Check.True(Sys.BrotliEncoder.TryCompress(data, output, out int written, quality, window), "System compress");
        return output[..written];
    }

    public static byte[] SysDecompress(byte[] data)
    {
        using var input = new MemoryStream(data);
        using var stream = new Sys.BrotliStream(input, CompressionMode.Decompress);
        using var output = new MemoryStream();
        stream.CopyTo(output);
        return output.ToArray();
    }

    public static byte[] Compress(byte[] data, int quality, int window = 22)
    {
        var output = new byte[Burli.BrotliEncoder.GetMaxCompressedLength(data.Length)];
        Check.True(Burli.BrotliEncoder.TryCompress(data, output, out int written, quality, window), "burli compress within the bound");
        return output[..written];
    }

    /// <summary>Decodes in pieces into a small destination, as a streaming caller does.</summary>
    public static byte[] DecompressInPieces(byte[] data, int seed)
    {
        var decoder = new Burli.BrotliDecoder();
        var output = new MemoryStream();
        var room = new byte[777];
        OperationStatus status = OperationStatus.NeedMoreData;
        foreach (ReadOnlyMemory<byte> piece in Payloads.Pieces(data, seed, 999).Append(ReadOnlyMemory<byte>.Empty))
        {
            ReadOnlySpan<byte> source = piece.Span;
            do
            {
                status = decoder.Decompress(source, room, out int consumed, out int written);
                Check.True(status != OperationStatus.InvalidData, "valid data");
                output.Write(room, 0, written);
                source = source[consumed..];
            } while (status == OperationStatus.DestinationTooSmall);
            Check.True(status == OperationStatus.Done || source.IsEmpty, "all input taken");
        }
        Check.Equal(OperationStatus.Done, status, "status");
        return output.ToArray();
    }
}

static class Tests
{
    public static void SystemDecodesBurliOutput()
    {
        foreach ((string name, byte[] payload) in Payloads.All)
        {
            for (int quality = 0; quality <= 11; quality++)
            {
                Check.Bytes(payload, Codec.SysDecompress(Codec.Compress(payload, quality)), $"{name} at quality {quality}");
            }
        }
        foreach (int window in new[] { 10, 16, 24 })
        {
            Check.Bytes(Payloads.All["mixed"], Codec.SysDecompress(Codec.Compress(Payloads.All["mixed"], 4, window)));
        }
    }

    public static void DecodesSystemOutput()
    {
        foreach ((string name, byte[] payload) in Payloads.All)
        {
            for (int quality = 0; quality <= 11; quality++)
            {
                foreach (int window in new[] { 10, 22 })
                {
                    byte[] data = quality >= 10 ? payload[..Math.Min(payload.Length, 10_000)] : payload;
                    byte[] encoded = Codec.SysCompress(data, quality, window);
                    var output = new byte[data.Length];
                    Check.True(Burli.BrotliDecoder.TryDecompress(encoded, output, out int written), $"{name} at quality {quality}");
                    Check.Bytes(data, output.AsSpan(0, written));
                    Check.Bytes(data, Codec.DecompressInPieces(encoded, quality));
                }
            }
        }
    }

    public static void TryMethodsReportSmallDestinations()
    {
        byte[] payload = Payloads.All["text"];
        Check.True(!Burli.BrotliEncoder.TryCompress(payload, new byte[10], out int written), "compress fails");
        Check.Equal(0, written);
        byte[] encoded = Codec.SysCompress(payload, 5);
        Check.True(!Burli.BrotliDecoder.TryDecompress(encoded, new byte[payload.Length - 1], out written), "decompress fails");
        Check.Equal(0, written);
        var output = new byte[Burli.BrotliEncoder.GetMaxCompressedLength(payload.Length)];
        Check.True(Burli.BrotliEncoder.TryCompress(payload, output, out written), "default compress");
        Check.Bytes(payload, Codec.SysDecompress(output[..written]));
    }

    public static void MaxCompressedLengthMatchesSystem()
    {
        foreach (int size in new[] { 0, 1, 1000, 16_384, 16_385, 1 << 20, (1 << 24) + 5 })
        {
            Check.True(Burli.BrotliEncoder.GetMaxCompressedLength(size) >= Sys.BrotliEncoder.GetMaxCompressedLength(size), $"bound for {size}");
        }
        Check.SameThrow(() => Burli.BrotliEncoder.GetMaxCompressedLength(-1), () => Sys.BrotliEncoder.GetMaxCompressedLength(-1));
        Check.SameThrow(() => Burli.BrotliEncoder.GetMaxCompressedLength(int.MaxValue), () => Sys.BrotliEncoder.GetMaxCompressedLength(int.MaxValue));
    }

    public static void EncoderWritesStreamsInPieces()
    {
        byte[] payload = Payloads.All["mixed"];
        foreach (int quality in new[] { 0, 1, 5, 11 })
        {
            var encoder = new Burli.BrotliEncoder(quality, 22);
            var output = new MemoryStream();
            var room = new byte[100];
            foreach (ReadOnlyMemory<byte> piece in Payloads.Pieces(payload, quality, 5000))
            {
                ReadOnlySpan<byte> source = piece.Span;
                OperationStatus status;
                do
                {
                    status = encoder.Compress(source, room, out int consumed, out int written, isFinalBlock: false);
                    output.Write(room, 0, written);
                    source = source[consumed..];
                } while (status == OperationStatus.DestinationTooSmall);
                Check.Equal(OperationStatus.Done, status);
                Check.True(source.IsEmpty, "all input taken");
            }
            OperationStatus final;
            do
            {
                final = encoder.Compress(default, room, out _, out int written, isFinalBlock: true);
                output.Write(room, 0, written);
            } while (final == OperationStatus.DestinationTooSmall);
            Check.Equal(OperationStatus.Done, final);
            Check.Bytes(payload, Codec.SysDecompress(output.ToArray()));
            // No input after the end of the stream.
            Check.Equal(OperationStatus.InvalidData, encoder.Compress("x"u8, room, out _, out _, isFinalBlock: false));
        }
    }

    public static void FlushMakesInputSoFarDecodable()
    {
        byte[] payload = Payloads.All["text"];
        using var burli = new MemoryStream();
        using var stream = new Burli.BrotliStream(burli, CompressionLevel.Optimal, leaveOpen: true);
        var decoder = new Sys.BrotliDecoder();
        var decoded = new MemoryStream();
        var room = new byte[1 << 20];
        int taken = 0;
        int written = 0;
        foreach (ReadOnlyMemory<byte> piece in Payloads.Pieces(payload, 2, 20_000))
        {
            stream.Write(piece.Span);
            stream.Flush();
            written += piece.Length;
            byte[] encoded = burli.ToArray();
            decoder.Decompress(encoded.AsSpan(taken), room, out int consumed, out int count);
            taken += consumed;
            decoded.Write(room, 0, count);
            Check.Bytes(payload.AsSpan(0, written), decoded.ToArray(), "flushed output");
        }
    }

    public static void StreamsRoundTrip()
    {
        byte[] payload = Payloads.All["mixed"];
        foreach (CompressionLevel level in Enum.GetValues<CompressionLevel>())
        {
            var compressed = new MemoryStream();
            using (var stream = new Burli.BrotliStream(compressed, level, leaveOpen: true))
            {
                foreach (ReadOnlyMemory<byte> piece in Payloads.Pieces(payload, 3, 4000))
                {
                    stream.Write(piece.Span);
                }
            }
            Check.Bytes(payload, Codec.SysDecompress(compressed.ToArray()), level.ToString());

            using var reader = new Burli.BrotliStream(new MemoryStream(Codec.SysCompress(payload, 11)), CompressionMode.Decompress);
            var decoded = new MemoryStream();
            var room = new byte[333];
            int read;
            while ((read = reader.Read(room, 0, room.Length)) > 0)
            {
                decoded.Write(room, 0, read);
            }
            Check.Bytes(payload, decoded.ToArray());
            Check.Equal(0, reader.Read(room));
        }
    }

    public static async Task StreamsRoundTripAsync()
    {
        byte[] payload = Payloads.All["text"];
        var compressed = new MemoryStream();
        await using (var stream = new Burli.BrotliStream(compressed, new Burli.BrotliCompressionOptions { Quality = 3 }, leaveOpen: true))
        {
            foreach (ReadOnlyMemory<byte> piece in Payloads.Pieces(payload, 4, 4000))
            {
                await stream.WriteAsync(piece);
            }
            await stream.FlushAsync();
        }
        Check.Bytes(payload, Codec.SysDecompress(compressed.ToArray()));
        compressed.Position = 0;
        await using var reader = new Burli.BrotliStream(compressed, CompressionMode.Decompress);
        var decoded = new MemoryStream();
        await reader.CopyToAsync(decoded);
        Check.Bytes(payload, decoded.ToArray());
    }

    public static void StreamsLeaveBaseStreamsOpenOnRequest()
    {
        var open = new MemoryStream();
        new Burli.BrotliStream(open, CompressionMode.Compress, leaveOpen: true).Dispose();
        Check.True(open.CanWrite, "left open");
        var closed = new MemoryStream();
        new Burli.BrotliStream(closed, CompressionMode.Compress).Dispose();
        Check.True(!closed.CanWrite, "closed");
    }

    public static void StreamsFailOnBadInput()
    {
        byte[] corrupt = [0xff, 0xff, 0xff];
        Check.SameThrow(
            () => new Burli.BrotliStream(new MemoryStream(corrupt), CompressionMode.Decompress).CopyTo(Stream.Null),
            () => new Sys.BrotliStream(new MemoryStream(corrupt), CompressionMode.Decompress).CopyTo(Stream.Null));
        // System.IO.Compression returns what it decoded of a truncated stream.
        byte[] encoded = Codec.SysCompress(Payloads.All["text"], 5);
        foreach (int end in new[] { 1, encoded.Length / 2, encoded.Length - 1 })
        {
            Exception error = Check.Throws(
                () => new Burli.BrotliStream(new MemoryStream(encoded[..end]), CompressionMode.Decompress).CopyTo(Stream.Null));
            Check.Equal(typeof(InvalidDataException), error.GetType(), "exception");
        }
        // An empty stream decodes to nothing in both.
        Check.Equal(0, new Burli.BrotliStream(new MemoryStream(), CompressionMode.Decompress).Read(new byte[10]));
        Check.Equal(0, new Sys.BrotliStream(new MemoryStream(), CompressionMode.Decompress).Read(new byte[10]));
    }

    public static void StreamsCheckTheirModeAndState()
    {
        var reader = new Burli.BrotliStream(new MemoryStream(), CompressionMode.Decompress);
        var writer = new Burli.BrotliStream(new MemoryStream(), CompressionMode.Compress);
        var sysReader = new Sys.BrotliStream(new MemoryStream(), CompressionMode.Decompress);
        var sysWriter = new Sys.BrotliStream(new MemoryStream(), CompressionMode.Compress);
        Check.Equal((sysReader.CanRead, sysReader.CanWrite, sysReader.CanSeek), (reader.CanRead, reader.CanWrite, reader.CanSeek));
        Check.Equal((sysWriter.CanRead, sysWriter.CanWrite, sysWriter.CanSeek), (writer.CanRead, writer.CanWrite, writer.CanSeek));
        Check.SameThrow(() => reader.Write(new byte[1]), () => sysReader.Write(new byte[1]));
        Check.SameThrow(() => _ = writer.Read(new byte[1]), () => _ = sysWriter.Read(new byte[1]));
        Check.SameThrow(() => _ = reader.Length, () => _ = sysReader.Length);
        Check.SameThrow(() => reader.Seek(0, SeekOrigin.Begin), () => sysReader.Seek(0, SeekOrigin.Begin));
        reader.Dispose();
        sysReader.Dispose();
        Check.SameThrow(() => _ = reader.Read(new byte[1]), () => _ = sysReader.Read(new byte[1]));
        Check.SameThrow(
            () => new Burli.BrotliStream(new MemoryStream([], writable: false), CompressionMode.Compress),
            () => new Sys.BrotliStream(new MemoryStream([], writable: false), CompressionMode.Compress));
        Check.SameThrow(
            () => new Burli.BrotliStream(new MemoryStream(), (CompressionMode)7),
            () => new Sys.BrotliStream(new MemoryStream(), (CompressionMode)7));
        Check.SameThrow(
            () => new Burli.BrotliStream(new MemoryStream(), (CompressionLevel)7),
            () => new Sys.BrotliStream(new MemoryStream(), (CompressionLevel)7));
        Check.SameThrow(() => new Burli.BrotliStream(null!, CompressionMode.Compress), () => new Sys.BrotliStream(null!, CompressionMode.Compress));
    }

    public static void RejectsInvalidParametersAsSystemDoes()
    {
        foreach ((int quality, int window) in new[] { (-1, 22), (12, 22), (5, 9), (5, 25) })
        {
            Check.SameThrow(() => new Burli.BrotliEncoder(quality, window), () => new Sys.BrotliEncoder(quality, window));
            Check.SameThrow(
                () => Burli.BrotliEncoder.TryCompress("x"u8, new byte[10], out _, quality, window),
                () => Sys.BrotliEncoder.TryCompress("x"u8, new byte[10], out _, quality, window));
        }
        Check.SameThrow(() => new Burli.BrotliCompressionOptions { Quality = 12 }, () => new Sys.BrotliCompressionOptions { Quality = 12 });
    }

    public static void DecodersReportMalformedStreams()
    {
        foreach (byte[] corrupt in new[] { new byte[] { 0xff, 0xff, 0xff }, [0x11, 0x00] })
        {
            Check.True(!Burli.BrotliDecoder.TryDecompress(corrupt, new byte[100], out _), "TryDecompress fails");
            var copy = new Burli.BrotliDecoder();
            Check.Equal(OperationStatus.InvalidData, copy.Decompress(corrupt, new byte[100], out _, out _));
            Check.Equal(OperationStatus.InvalidData, copy.Decompress(default, new byte[100], out _, out _));
        }
    }

    public static void DataAfterTheStream()
    {
        byte[] encoded = Codec.SysCompress("payload"u8.ToArray(), 5);
        byte[] trailing = [.. encoded, (byte)'x'];
        var output = new byte[100];
        // One-shot decoding rejects it, which System.IO.Compression does not.
        Check.True(!Burli.BrotliDecoder.TryDecompress(trailing, output, out _), "TryDecompress fails");
        // Streaming decoders stop at the end of the stream.
        var decoder = new Burli.BrotliDecoder();
        Check.Equal(OperationStatus.Done, decoder.Decompress(trailing, output, out int consumed, out int written));
        Check.Equal(encoded.Length, consumed);
        Check.Bytes("payload"u8, output.AsSpan(0, written));
        var decoded = new MemoryStream();
        new Burli.BrotliStream(new MemoryStream(trailing), CompressionMode.Decompress).CopyTo(decoded);
        Check.Bytes("payload"u8, decoded.ToArray());
    }

    public static void DisposedCodersThrow()
    {
        var encoder = new Burli.BrotliEncoder(5, 22);
        encoder.Dispose();
        var sysEncoder = new Sys.BrotliEncoder(5, 22);
        sysEncoder.Dispose();
        Check.SameThrow(() => encoder.Flush(new byte[10], out _), () => sysEncoder.Flush(new byte[10], out _));
        var decoder = new Burli.BrotliDecoder();
        decoder.Dispose();
        var sysDecoder = new Sys.BrotliDecoder();
        sysDecoder.Dispose();
        Check.SameThrow(() => decoder.Decompress(default, new byte[10], out _, out _), () => sysDecoder.Decompress(default, new byte[10], out _, out _));
    }

    public static void WorksInParallel()
    {
        byte[] payload = Payloads.All["mixed"];
        Parallel.For(0, 8, quality =>
        {
            Check.Bytes(payload, Codec.SysDecompress(Codec.Compress(payload, quality)));
        });
    }
}
