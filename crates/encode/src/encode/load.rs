#[inline(always)]
pub(super) fn read_u32_le_trusted(input: &[u8], pos: usize) -> u32 {
    debug_assert!(pos.checked_add(4).is_some_and(|end| end <= input.len()));

    let bytes = input[pos..]
        .first_chunk::<4>()
        .expect("read_u32_le range checked by caller");
    u32::from_le_bytes(*bytes)
}

#[inline(always)]
pub(super) fn read_u64_le_trusted(input: &[u8], pos: usize) -> u64 {
    debug_assert!(pos.checked_add(8).is_some_and(|end| end <= input.len()));

    let bytes = input[pos..]
        .first_chunk::<8>()
        .expect("read_u64_le range checked by caller");
    u64::from_le_bytes(*bytes)
}
