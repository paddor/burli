use burli_core::{
    BurliError, CompressError,
    bits::BitWriter,
    format::{MAX_WINDOW_BITS, MIN_WINDOW_BITS},
};

pub(crate) fn write_window_bits(
    writer: &mut BitWriter,
    window_bits: u8,
) -> Result<(), CompressError> {
    if !(MIN_WINDOW_BITS..=MAX_WINDOW_BITS).contains(&window_bits) {
        return Err(BurliError::InvalidWindowBits(window_bits));
    }

    if window_bits == 16 {
        writer.write_bits(1, 0)
    } else if window_bits == 17 {
        writer.write_bits(7, 1)
    } else if window_bits > 17 {
        let bits = ((window_bits - 17) << 1) | 1;
        writer.write_bits(4, u64::from(bits))
    } else {
        let bits = ((window_bits - 8) << 4) | 1;
        writer.write_bits(7, u64::from(bits))
    }
}

pub(crate) fn write_last_empty_meta_block(writer: &mut BitWriter) -> Result<(), CompressError> {
    writer.write_bits(1, 1)?;
    writer.write_bits(1, 1)
}
