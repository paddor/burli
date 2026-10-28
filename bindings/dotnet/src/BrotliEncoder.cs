using System.Buffers;

namespace Burli;

/// <summary>
/// Brotli compression with the API of
/// <see cref="System.IO.Compression.BrotliEncoder"/>. Qualities 6 to 11 run
/// as 5.
/// </summary>
public struct BrotliEncoder : IDisposable
{
    internal const int MinQuality = 0;
    internal const int MaxQuality = 11;
    internal const int DefaultQuality = 4;
    internal const int MinWindow = 10;
    internal const int MaxWindow = 24;
    internal const int DefaultWindow = 22;
    private const int MaxInputSize = int.MaxValue - 515;

    private EncoderHandle? _handle;
    private bool _disposed;

    /// <summary>Creates an encoder with a quality from 0 to 11 and a window from 10 to 24.</summary>
    /// <exception cref="ArgumentOutOfRangeException">The quality or window is out of range.</exception>
    public BrotliEncoder(int quality, int window)
    {
        _handle = Create(quality, window);
        _disposed = false;
    }

    private static EncoderHandle Create(int quality, int window)
    {
        CheckParameters(quality, window);
        EncoderHandle handle = Native.burli_encoder_create(quality, window);
        if (handle.IsInvalid)
        {
            handle.Dispose();
            throw new InvalidOperationException("Failed to create the Brotli encoder.");
        }
        return handle;
    }

    private static void CheckParameters(int quality, int window)
    {
        ArgumentOutOfRangeException.ThrowIfLessThan(quality, MinQuality);
        ArgumentOutOfRangeException.ThrowIfGreaterThan(quality, MaxQuality);
        ArgumentOutOfRangeException.ThrowIfLessThan(window, MinWindow);
        ArgumentOutOfRangeException.ThrowIfGreaterThan(window, MaxWindow);
    }

    /// <summary>
    /// Compresses <paramref name="source"/> into <paramref name="destination"/>.
    /// With <paramref name="isFinalBlock"/>, also ends the stream.
    /// </summary>
    /// <returns>
    /// <see cref="OperationStatus.DestinationTooSmall"/> while output waits for
    /// room; call again with the rest of the source.
    /// </returns>
    public OperationStatus Compress(ReadOnlySpan<byte> source, Span<byte> destination, out int bytesConsumed, out int bytesWritten, bool isFinalBlock) =>
        Run(source, destination, out bytesConsumed, out bytesWritten, isFinalBlock ? Native.OperationFinish : Native.OperationProcess);

    /// <summary>Writes output that decodes to all source so far.</summary>
    public OperationStatus Flush(Span<byte> destination, out int bytesWritten) =>
        Run(default, destination, out _, out bytesWritten, Native.OperationFlush);

    private unsafe OperationStatus Run(ReadOnlySpan<byte> source, Span<byte> destination, out int bytesConsumed, out int bytesWritten, int operation)
    {
        ObjectDisposedException.ThrowIf(_disposed, typeof(BrotliEncoder));
        // As System.IO.Compression, a default encoder uses Brotli's defaults.
        _handle ??= Create(MaxQuality, DefaultWindow);
        fixed (byte* input = source)
        fixed (byte* output = destination)
        {
            int status = Native.burli_encoder_compress(
                _handle, input, (nuint)source.Length, output, (nuint)destination.Length, out nuint consumed, out nuint written, operation);
            bytesConsumed = (int)consumed;
            bytesWritten = (int)written;
            return (OperationStatus)status;
        }
    }

    /// <summary>Frees the native encoder.</summary>
    public void Dispose()
    {
        _disposed = true;
        _handle?.Dispose();
    }

    /// <summary>The largest compressed size of <paramref name="inputSize"/> bytes.</summary>
    public static int GetMaxCompressedLength(int inputSize)
    {
        ArgumentOutOfRangeException.ThrowIfNegative(inputSize);
        ArgumentOutOfRangeException.ThrowIfGreaterThan(inputSize, MaxInputSize);
        if (inputSize == 0)
        {
            return 2;
        }
        // Stream header, one stored block header per 16 KiB, and the last
        // empty block, as Brotli bounds it.
        return inputSize + 2 + 4 * (inputSize >> 14) + 3 + 1;
    }

    /// <summary>Compresses all of <paramref name="source"/> at quality 4.</summary>
    /// <returns>Whether the output fit into <paramref name="destination"/>.</returns>
    public static bool TryCompress(ReadOnlySpan<byte> source, Span<byte> destination, out int bytesWritten) =>
        TryCompress(source, destination, out bytesWritten, DefaultQuality, DefaultWindow);

    /// <summary>Compresses all of <paramref name="source"/>.</summary>
    /// <returns>Whether the output fit into <paramref name="destination"/>.</returns>
    /// <exception cref="ArgumentOutOfRangeException">The quality or window is out of range.</exception>
    public static unsafe bool TryCompress(ReadOnlySpan<byte> source, Span<byte> destination, out int bytesWritten, int quality, int window)
    {
        CheckParameters(quality, window);
        fixed (byte* input = source)
        fixed (byte* output = destination)
        {
            int status = Native.burli_compress(
                input, (nuint)source.Length, output, (nuint)destination.Length, out nuint written, quality, window);
            bytesWritten = status == (int)OperationStatus.Done ? (int)written : 0;
            return status == (int)OperationStatus.Done;
        }
    }
}
