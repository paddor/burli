namespace Burli;

/// <summary>
/// Options for <see cref="BrotliStream"/>, as
/// <c>System.IO.Compression.BrotliCompressionOptions</c>.
/// </summary>
public sealed class BrotliCompressionOptions
{
    private int _quality = BrotliEncoder.DefaultQuality;

    /// <summary>Quality from 0 to 11; 6 to 11 run as 5. The default is 4.</summary>
    /// <exception cref="ArgumentOutOfRangeException">The value is out of range.</exception>
    public int Quality
    {
        get => _quality;
        set
        {
            ArgumentOutOfRangeException.ThrowIfLessThan(value, BrotliEncoder.MinQuality);
            ArgumentOutOfRangeException.ThrowIfGreaterThan(value, BrotliEncoder.MaxQuality);
            _quality = value;
        }
    }
}
