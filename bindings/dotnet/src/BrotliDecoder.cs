using System.Buffers;

namespace Burli;

/// <summary>
/// Brotli decompression with the API of
/// <see cref="System.IO.Compression.BrotliDecoder"/>.
/// </summary>
public struct BrotliDecoder : IDisposable
{
    private DecoderHandle? _handle;
    private bool _disposed;

    /// <summary>
    /// Decompresses <paramref name="source"/> into <paramref name="destination"/>.
    /// </summary>
    /// <returns>
    /// <see cref="OperationStatus.Done"/> at the end of the stream, which
    /// takes no source past it.
    /// </returns>
    public unsafe OperationStatus Decompress(ReadOnlySpan<byte> source, Span<byte> destination, out int bytesConsumed, out int bytesWritten)
    {
        ObjectDisposedException.ThrowIf(_disposed, typeof(BrotliDecoder));
        _handle ??= Native.burli_decoder_create();
        fixed (byte* input = source)
        fixed (byte* output = destination)
        {
            int status = Native.burli_decoder_decompress(
                _handle, input, (nuint)source.Length, output, (nuint)destination.Length, out nuint consumed, out nuint written);
            bytesConsumed = (int)consumed;
            bytesWritten = (int)written;
            return (OperationStatus)status;
        }
    }

    /// <summary>Frees the native decoder.</summary>
    public void Dispose()
    {
        _disposed = true;
        _handle?.Dispose();
    }

    /// <summary>Decompresses <paramref name="source"/>, a whole stream.</summary>
    /// <returns>
    /// Whether the stream is valid, has no data after its end, and fit into
    /// <paramref name="destination"/>.
    /// </returns>
    public static unsafe bool TryDecompress(ReadOnlySpan<byte> source, Span<byte> destination, out int bytesWritten)
    {
        fixed (byte* input = source)
        fixed (byte* output = destination)
        {
            int status = Native.burli_decompress(input, (nuint)source.Length, output, (nuint)destination.Length, out nuint written);
            bytesWritten = status == (int)OperationStatus.Done ? (int)written : 0;
            return status == (int)OperationStatus.Done;
        }
    }
}
