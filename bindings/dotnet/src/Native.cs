using System.Runtime.InteropServices;

namespace Burli;

/// <summary>
/// Calls into the native library this package ships. Status codes are
/// <see cref="System.Buffers.OperationStatus"/> values.
/// </summary>
internal static unsafe partial class Native
{
    private const string Library = "burli_dotnet";

    /// <summary><c>BrotliEncoderOperation</c> values.</summary>
    internal const int OperationProcess = 0;
    internal const int OperationFlush = 1;
    internal const int OperationFinish = 2;

    [LibraryImport(Library)]
    internal static partial EncoderHandle burli_encoder_create(int quality, int window);

    [LibraryImport(Library)]
    internal static partial void burli_encoder_destroy(IntPtr encoder);

    [LibraryImport(Library)]
    internal static partial int burli_encoder_compress(
        EncoderHandle encoder,
        byte* source,
        nuint sourceLength,
        byte* destination,
        nuint destinationLength,
        out nuint consumed,
        out nuint written,
        int operation);

    [LibraryImport(Library)]
    internal static partial DecoderHandle burli_decoder_create();

    [LibraryImport(Library)]
    internal static partial void burli_decoder_destroy(IntPtr decoder);

    [LibraryImport(Library)]
    internal static partial int burli_decoder_decompress(
        DecoderHandle decoder,
        byte* source,
        nuint sourceLength,
        byte* destination,
        nuint destinationLength,
        out nuint consumed,
        out nuint written);

    [LibraryImport(Library)]
    internal static partial int burli_compress(
        byte* source,
        nuint sourceLength,
        byte* destination,
        nuint destinationLength,
        out nuint written,
        int quality,
        int window);

    [LibraryImport(Library)]
    internal static partial int burli_decompress(
        byte* source,
        nuint sourceLength,
        byte* destination,
        nuint destinationLength,
        out nuint written);
}

/// <summary>Owns a native encoder. Calls keep it alive until they return.</summary>
internal sealed class EncoderHandle : SafeHandle
{
    public EncoderHandle() : base(IntPtr.Zero, ownsHandle: true) { }

    public override bool IsInvalid => handle == IntPtr.Zero;

    protected override bool ReleaseHandle()
    {
        Native.burli_encoder_destroy(handle);
        return true;
    }
}

/// <summary>Owns a native decoder. Calls keep it alive until they return.</summary>
internal sealed class DecoderHandle : SafeHandle
{
    public DecoderHandle() : base(IntPtr.Zero, ownsHandle: true) { }

    public override bool IsInvalid => handle == IntPtr.Zero;

    protected override bool ReleaseHandle()
    {
        Native.burli_decoder_destroy(handle);
        return true;
    }
}
