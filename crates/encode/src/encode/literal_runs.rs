//! Writes the literals of literal-only meta-blocks with their prefix codes.
//!
//! The bits from the partial byte on wait in a register, and each store
//! writes all 8 bytes of them, so a store does not wait on the store before
//! it to read the partial byte back. Literals go out in runs, and the stores
//! of a run stay in a window of [`WINDOW`] bytes, at offsets that need no
//! bounds check.

/// Bytes a run may store into: its codes and up to 7 waiting bits take at
/// most 57 bytes, so its 8-byte stores start at most 56 bytes in.
const WINDOW: usize = 72;

/// Room past the codes with which [`write`] never runs out of it.
pub(super) const SLACK_BITS: usize = WINDOW * 8;

/// Writes the codes of `input` into `out`, room from
/// [`burli_core::bits::BitWriter::write_bits_raw`] with the pending bits up
/// to bit `start`. `code` gives the code bits and length of each literal in
/// turn, at most `MAX_BITS` long. Literals come together in groups of
/// `GROUP`, which fit one store, and runs of `RUN` share one room check.
/// Returns the bit position after the codes, and `false` when `out` runs
/// out of room first.
#[inline(always)]
pub(super) fn write<const MAX_BITS: usize, const GROUP: usize, const RUN: usize>(
    out: &mut [u8],
    start: usize,
    input: &[u8],
    mut code: impl FnMut(u8) -> (u64, u32),
) -> (usize, bool) {
    const {
        assert!(GROUP * MAX_BITS + 7 <= 64);
        assert!(RUN * MAX_BITS + 7 <= 57 * 8 && 64 + 8 <= WINDOW);
    };
    let mut sink = Sink {
        pos: 0,
        bits: u64::from(out[0]),
        count: start as u32,
    };
    // Full runs have a fixed length, so their groups unroll.
    let (runs, tail) = input.as_chunks::<RUN>();
    let fits = runs
        .iter()
        .all(|run| sink.write_run::<GROUP>(out, run, &mut code))
        && (tail.is_empty() || sink.write_run::<GROUP>(out, tail, &mut code));
    (sink.pos * 8 + sink.count as usize, fits)
}

struct Sink {
    /// Bytes before the partial one.
    pos: usize,
    bits: u64,
    /// Bits in `bits`, fewer than 8 between runs.
    count: u32,
}

impl Sink {
    /// Writes `run`, of literals whose codes [`write`] bounds. Returns
    /// `false` when `out` has no room for the window of the run.
    #[inline(always)]
    fn write_run<const GROUP: usize>(
        &mut self,
        out: &mut [u8],
        run: &[u8],
        code: &mut impl FnMut(u8) -> (u64, u32),
    ) -> bool {
        let Some(window) = out.get_mut(self.pos..self.pos + WINDOW) else {
            return false;
        };
        let window: &mut [u8; WINDOW] = window.try_into().unwrap();
        let mut offset = 0;
        // Fewer than 8 bits wait, so a group more fits in `bits`. Each group
        // comes together on its own, so its shifts do not wait on the group
        // before.
        let mut write = |group: &[u8]| {
            let mut value = 0_u64;
            let mut width = 0_u32;
            for &byte in group {
                let (bits, len) = code(byte);
                value |= bits << width;
                width += len;
            }
            self.bits |= value << self.count;
            self.count += width;
            window[offset & 63..][..8].copy_from_slice(&self.bits.to_le_bytes());
            offset += (self.count / 8) as usize;
            self.bits >>= self.count & !7;
            self.count %= 8;
        };
        let (groups, rest) = run.as_chunks::<GROUP>();
        for group in groups {
            write(group);
        }
        if !rest.is_empty() {
            write(rest);
        }
        self.pos += offset;
        true
    }
}
