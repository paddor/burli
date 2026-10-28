using System.Buffers;
using System.IO.Compression;

namespace Burli;

/// <summary>
/// A stream that compresses into or decompresses from another stream, with
/// the API of <see cref="System.IO.Compression.BrotliStream"/>. Qualities 6
/// to 11 run as 5.
/// </summary>
public sealed class BrotliStream : Stream
{
    // As System.IO.Compression.
    private const int BufferSize = (1 << 16) - 16;

    private Stream? _stream;
    private readonly bool _leaveOpen;
    private readonly CompressionMode _mode;
    private BrotliEncoder _encoder;
    private BrotliDecoder _decoder;
    private readonly byte[] _buffer = new byte[BufferSize];
    // Compressed bytes read from the stream that the decoder has not taken.
    private int _bufferOffset;
    private int _bufferCount;
    private bool _nonEmptyInput;

    /// <summary>Compresses into <paramref name="stream"/> at <paramref name="compressionLevel"/>.</summary>
    public BrotliStream(Stream stream, CompressionLevel compressionLevel)
        : this(stream, compressionLevel, leaveOpen: false) { }

    /// <summary>Compresses into <paramref name="stream"/> at <paramref name="compressionLevel"/>.</summary>
    public BrotliStream(Stream stream, CompressionLevel compressionLevel, bool leaveOpen)
        : this(stream, CompressionMode.Compress, leaveOpen, Quality(compressionLevel)) { }

    /// <summary>Compresses into or decompresses from <paramref name="stream"/>.</summary>
    public BrotliStream(Stream stream, CompressionMode mode)
        : this(stream, mode, leaveOpen: false) { }

    /// <summary>Compresses into or decompresses from <paramref name="stream"/>.</summary>
    public BrotliStream(Stream stream, CompressionMode mode, bool leaveOpen)
        : this(stream, mode, leaveOpen, BrotliEncoder.DefaultQuality) { }

    /// <summary>Compresses into <paramref name="stream"/> with <paramref name="compressionOptions"/>.</summary>
    public BrotliStream(Stream stream, BrotliCompressionOptions compressionOptions, bool leaveOpen = false)
        : this(stream, CompressionMode.Compress, leaveOpen, Options(compressionOptions).Quality) { }

    private BrotliStream(Stream stream, CompressionMode mode, bool leaveOpen, int quality)
    {
        ArgumentNullException.ThrowIfNull(stream);
        switch (mode)
        {
            case CompressionMode.Compress:
                if (!stream.CanWrite)
                {
                    throw new ArgumentException("Stream does not support writing.", nameof(stream));
                }
                _encoder = new BrotliEncoder(quality, BrotliEncoder.DefaultWindow);
                break;
            case CompressionMode.Decompress:
                if (!stream.CanRead)
                {
                    throw new ArgumentException("Stream does not support reading.", nameof(stream));
                }
                break;
            default:
                throw new ArgumentException("Enum value was out of legal range.", nameof(mode));
        }
        _stream = stream;
        _mode = mode;
        _leaveOpen = leaveOpen;
    }

    // As System.IO.Compression maps them.
    private static int Quality(CompressionLevel level) => level switch
    {
        CompressionLevel.NoCompression => BrotliEncoder.MinQuality,
        CompressionLevel.Fastest => 1,
        CompressionLevel.Optimal => BrotliEncoder.DefaultQuality,
        CompressionLevel.SmallestSize => BrotliEncoder.MaxQuality,
        _ => throw new ArgumentException("Enum value was out of legal range.", nameof(level)),
    };

    private static BrotliCompressionOptions Options(BrotliCompressionOptions options)
    {
        ArgumentNullException.ThrowIfNull(options, "compressionOptions");
        return options;
    }

    /// <summary>The stream this one compresses into or decompresses from.</summary>
    public Stream BaseStream => _stream!;

    /// <inheritdoc/>
    public override bool CanRead => _mode == CompressionMode.Decompress && _stream?.CanRead == true;

    /// <inheritdoc/>
    public override bool CanWrite => _mode == CompressionMode.Compress && _stream?.CanWrite == true;

    /// <inheritdoc/>
    public override bool CanSeek => false;

    /// <inheritdoc/>
    public override long Length => throw new NotSupportedException();

    /// <inheritdoc/>
    public override long Position
    {
        get => throw new NotSupportedException();
        set => throw new NotSupportedException();
    }

    /// <inheritdoc/>
    public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();

    /// <inheritdoc/>
    public override void SetLength(long value) => throw new NotSupportedException();

    private Stream Open(CompressionMode mode)
    {
        ObjectDisposedException.ThrowIf(_stream is null, this);
        if (_mode != mode)
        {
            throw new InvalidOperationException(mode == CompressionMode.Compress
                ? "Can not perform Write operations on a BrotliStream constructed with CompressionMode.Decompress."
                : "Can not perform Read operations on a BrotliStream constructed with CompressionMode.Compress.");
        }
        return _stream;
    }

    /// <inheritdoc/>
    public override void Write(byte[] buffer, int offset, int count)
    {
        ValidateBufferArguments(buffer, offset, count);
        Write(new ReadOnlySpan<byte>(buffer, offset, count));
    }

    /// <inheritdoc/>
    public override void Write(ReadOnlySpan<byte> buffer)
    {
        Stream stream = Open(CompressionMode.Compress);
        OperationStatus status = OperationStatus.DestinationTooSmall;
        while (status == OperationStatus.DestinationTooSmall)
        {
            status = Compress(buffer, out int consumed, out int written, isFinalBlock: false);
            if (written > 0)
            {
                stream.Write(_buffer, 0, written);
            }
            buffer = buffer[consumed..];
        }
    }

    /// <inheritdoc/>
    public override Task WriteAsync(byte[] buffer, int offset, int count, CancellationToken cancellationToken)
    {
        ValidateBufferArguments(buffer, offset, count);
        return WriteAsync(new ReadOnlyMemory<byte>(buffer, offset, count), cancellationToken).AsTask();
    }

    /// <inheritdoc/>
    public override async ValueTask WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken cancellationToken = default)
    {
        Stream stream = Open(CompressionMode.Compress);
        OperationStatus status = OperationStatus.DestinationTooSmall;
        while (status == OperationStatus.DestinationTooSmall)
        {
            status = Compress(buffer.Span, out int consumed, out int written, isFinalBlock: false);
            if (written > 0)
            {
                await stream.WriteAsync(_buffer.AsMemory(0, written), cancellationToken).ConfigureAwait(false);
            }
            buffer = buffer[consumed..];
        }
    }

    private OperationStatus Compress(ReadOnlySpan<byte> source, out int consumed, out int written, bool isFinalBlock)
    {
        OperationStatus status = _encoder.Compress(source, _buffer, out consumed, out written, isFinalBlock);
        if (status == OperationStatus.InvalidData)
        {
            throw new InvalidOperationException("Encoder ran into invalid data.");
        }
        return status;
    }

    private OperationStatus FlushEncoder(out int written)
    {
        OperationStatus status = _encoder.Flush(_buffer, out written);
        if (status == OperationStatus.InvalidData)
        {
            throw new InvalidOperationException("Encoder ran into invalid data.");
        }
        return status;
    }

    /// <summary>Writes output that decodes to all data so far, and flushes the base stream.</summary>
    public override void Flush()
    {
        ObjectDisposedException.ThrowIf(_stream is null, this);
        if (_mode != CompressionMode.Compress)
        {
            return;
        }
        OperationStatus status = OperationStatus.DestinationTooSmall;
        while (status == OperationStatus.DestinationTooSmall)
        {
            status = FlushEncoder(out int written);
            if (written > 0)
            {
                _stream.Write(_buffer, 0, written);
            }
        }
        _stream.Flush();
    }

    /// <inheritdoc/>
    public override async Task FlushAsync(CancellationToken cancellationToken)
    {
        ObjectDisposedException.ThrowIf(_stream is null, this);
        if (_mode != CompressionMode.Compress)
        {
            return;
        }
        OperationStatus status = OperationStatus.DestinationTooSmall;
        while (status == OperationStatus.DestinationTooSmall)
        {
            status = FlushEncoder(out int written);
            if (written > 0)
            {
                await _stream.WriteAsync(_buffer.AsMemory(0, written), cancellationToken).ConfigureAwait(false);
            }
        }
        await _stream.FlushAsync(cancellationToken).ConfigureAwait(false);
    }

    /// <inheritdoc/>
    public override int Read(byte[] buffer, int offset, int count)
    {
        ValidateBufferArguments(buffer, offset, count);
        return Read(new Span<byte>(buffer, offset, count));
    }

    /// <inheritdoc/>
    public override int Read(Span<byte> buffer)
    {
        Stream stream = Open(CompressionMode.Decompress);
        int written;
        while (!TryDecompress(buffer, out written))
        {
            int read = stream.Read(_buffer, StartRefill(), _buffer.Length - _bufferCount);
            if (!Refilled(read, buffer.IsEmpty))
            {
                break;
            }
        }
        return written;
    }

    /// <inheritdoc/>
    public override Task<int> ReadAsync(byte[] buffer, int offset, int count, CancellationToken cancellationToken)
    {
        ValidateBufferArguments(buffer, offset, count);
        return ReadAsync(new Memory<byte>(buffer, offset, count), cancellationToken).AsTask();
    }

    /// <inheritdoc/>
    public override async ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken cancellationToken = default)
    {
        Stream stream = Open(CompressionMode.Decompress);
        int written;
        while (!TryDecompress(buffer.Span, out written))
        {
            int start = StartRefill();
            int read = await stream.ReadAsync(_buffer.AsMemory(start, _buffer.Length - _bufferCount), cancellationToken).ConfigureAwait(false);
            if (!Refilled(read, buffer.IsEmpty))
            {
                break;
            }
        }
        return written;
    }

    /// <summary>
    /// Decompresses buffered input. Returns whether the read is over: it wrote
    /// output, reached the end of the stream, or was a zero-byte read with
    /// buffered input.
    /// </summary>
    private bool TryDecompress(Span<byte> destination, out int written)
    {
        OperationStatus status = _decoder.Decompress(
            _buffer.AsSpan(_bufferOffset, _bufferCount), destination, out int consumed, out written);
        if (status == OperationStatus.InvalidData)
        {
            throw new InvalidOperationException("Decoder ran into invalid data.");
        }
        _bufferOffset += consumed;
        _bufferCount -= consumed;
        return written != 0 || status == OperationStatus.Done || (destination.IsEmpty && _bufferCount != 0);
    }

    /// <summary>Moves buffered input to the front; returns where new input goes.</summary>
    private int StartRefill()
    {
        _buffer.AsSpan(_bufferOffset, _bufferCount).CopyTo(_buffer);
        _bufferOffset = 0;
        return _bufferCount;
    }

    /// <summary>Counts <paramref name="read"/> new bytes. Returns false at the end of the base stream.</summary>
    /// <exception cref="InvalidDataException">The base stream ended inside the Brotli stream.</exception>
    private bool Refilled(int read, bool zeroByteRead)
    {
        if (read <= 0)
        {
            if (_nonEmptyInput && !zeroByteRead)
            {
                throw new InvalidDataException("Found truncated data while decoding.");
            }
            return false;
        }
        _nonEmptyInput = true;
        _bufferCount += read;
        return true;
    }

    /// <summary>Ends the stream when compressing, then closes the base stream unless left open.</summary>
    protected override void Dispose(bool disposing)
    {
        try
        {
            if (disposing && _stream is not null && _mode == CompressionMode.Compress)
            {
                OperationStatus status = OperationStatus.DestinationTooSmall;
                while (status == OperationStatus.DestinationTooSmall)
                {
                    status = Compress(default, out _, out int written, isFinalBlock: true);
                    if (written > 0)
                    {
                        _stream.Write(_buffer, 0, written);
                    }
                }
            }
        }
        finally
        {
            Release(disposing);
            base.Dispose(disposing);
        }
    }

    /// <inheritdoc/>
    public override async ValueTask DisposeAsync()
    {
        try
        {
            if (_stream is not null && _mode == CompressionMode.Compress)
            {
                OperationStatus status = OperationStatus.DestinationTooSmall;
                while (status == OperationStatus.DestinationTooSmall)
                {
                    status = Compress(default, out _, out int written, isFinalBlock: true);
                    if (written > 0)
                    {
                        await _stream.WriteAsync(_buffer.AsMemory(0, written)).ConfigureAwait(false);
                    }
                }
            }
        }
        finally
        {
            Stream? stream = _stream;
            Release(disposing: false);
            if (stream is not null && !_leaveOpen)
            {
                await stream.DisposeAsync().ConfigureAwait(false);
            }
            GC.SuppressFinalize(this);
        }
    }

    private void Release(bool disposing)
    {
        Stream? stream = _stream;
        _stream = null;
        _encoder.Dispose();
        _decoder.Dispose();
        if (disposing && stream is not null && !_leaveOpen)
        {
            stream.Dispose();
        }
    }
}
