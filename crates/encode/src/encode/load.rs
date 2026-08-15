#[inline(always)]
pub(super) fn read_u32_le_trusted(input: &[u8], pos: usize) -> u32 {
    debug_assert!(pos.checked_add(4).is_some_and(|end| end <= input.len()));

    #[cfg(not(feature = "paranoid"))]
    {
        // SAFETY: callers must prove `pos..pos + 4` is inside `input`.
        // The load is unaligned because Brotli match scans advance by bytes.
        unsafe { u32::from_le(core::ptr::read_unaligned(input.as_ptr().add(pos).cast())) }
    }

    #[cfg(feature = "paranoid")]
    {
        let bytes = input[pos..]
            .first_chunk::<4>()
            .expect("read_u32_le range checked by caller");
        u32::from_le_bytes(*bytes)
    }
}

#[inline(always)]
pub(super) fn read_u64_le_trusted(input: &[u8], pos: usize) -> u64 {
    debug_assert!(pos.checked_add(8).is_some_and(|end| end <= input.len()));

    #[cfg(not(feature = "paranoid"))]
    {
        // SAFETY: callers must prove `pos..pos + 8` is inside `input`.
        // The load is unaligned because Brotli match scans advance by bytes.
        unsafe { u64::from_le(core::ptr::read_unaligned(input.as_ptr().add(pos).cast())) }
    }

    #[cfg(feature = "paranoid")]
    {
        let bytes = input[pos..]
            .first_chunk::<8>()
            .expect("read_u64_le range checked by caller");
        u64::from_le_bytes(*bytes)
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    fn trusted_u32_load_matches_safe_little_endian_load() {
        let input = [
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
        ];
        let pos = usize::from(kani::any::<u8>());
        kani::assume(pos <= input.len() - 4);

        let expected =
            u32::from_le_bytes([input[pos], input[pos + 1], input[pos + 2], input[pos + 3]]);

        assert_eq!(read_u32_le_trusted(&input, pos), expected);
    }

    #[kani::proof]
    fn trusted_u64_load_matches_safe_little_endian_load() {
        let input = [
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
            kani::any::<u8>(),
        ];
        let pos = usize::from(kani::any::<u8>());
        kani::assume(pos <= input.len() - 8);

        let expected = u64::from_le_bytes([
            input[pos],
            input[pos + 1],
            input[pos + 2],
            input[pos + 3],
            input[pos + 4],
            input[pos + 5],
            input[pos + 6],
            input[pos + 7],
        ]);

        assert_eq!(read_u64_le_trusted(&input, pos), expected);
    }
}
