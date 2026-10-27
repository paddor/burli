use alloc::vec;
use alloc::vec::Vec;

use burli_core::{BurliError, DecompressError, bits::BitReader};

use crate::{
    dictionary::RawDictionary,
    huffman::{CodeGroup, PrefixCode, ROOT_SIZE, Table, Tables, WIDE_ROOT_SIZE},
};

const LITERAL_ALPHABET_SIZE: usize = 256;
const COMMAND_ALPHABET_SIZE: usize = 704;
const BLOCK_LENGTH_ALPHABET_SIZE: usize = 26;
const LAST_DISTANCES: [usize; 4] = [16, 15, 11, 4];
/// Meta-blocks shorter than this decode too few literals to pay for building
/// a wide literal root.
const WIDE_ROOT_MIN_LEN: usize = 4 * 1024;
/// Bytes of zeroed room the command loop adds at a time.
const ZERO_AHEAD: usize = 64 * 1024;
/// Zeroed room past the meta-block end. Short copies write whole 16- or
/// 32-byte windows; the bytes past the copy land here or ahead of the
/// decoder, where later writes replace them.
const OUTPUT_SLACK: usize = 32;

#[derive(Clone, Copy, Debug)]
pub(crate) struct DistanceRing {
    distances: [usize; 4],
}

impl DistanceRing {
    pub(crate) const fn new() -> Self {
        Self {
            distances: LAST_DISTANCES,
        }
    }

    #[inline(always)]
    fn resolve(&self, symbol: usize) -> Result<usize, DecompressError> {
        let distance = match symbol {
            0 => self.distances[3],
            1 => self.distances[2],
            2 => self.distances[1],
            3 => self.distances[0],
            4 => self.distances[3].saturating_sub(1),
            5 => self.distances[3] + 1,
            6 => self.distances[3].saturating_sub(2),
            7 => self.distances[3] + 2,
            8 => self.distances[3].saturating_sub(3),
            9 => self.distances[3] + 3,
            10 => self.distances[2].saturating_sub(1),
            11 => self.distances[2] + 1,
            12 => self.distances[2].saturating_sub(2),
            13 => self.distances[2] + 2,
            14 => self.distances[2].saturating_sub(3),
            15 => self.distances[2] + 3,
            _ => return Err(BurliError::Format("invalid Brotli short distance code")),
        };
        if distance == 0 {
            return Err(BurliError::Format("invalid Brotli zero distance"));
        }
        Ok(distance)
    }

    #[inline(always)]
    fn push(&mut self, distance: usize) {
        self.distances = [
            self.distances[1],
            self.distances[2],
            self.distances[3],
            distance,
        ];
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MetaBlockDecodeParams<'a> {
    pub(crate) output_base: usize,
    pub(crate) len: usize,
    /// The meta-block ends the stream, so the output ends with it.
    pub(crate) is_last: bool,
    pub(crate) max_output_size: usize,
    pub(crate) window_bits: u8,
    pub(crate) raw_dictionary: RawDictionary<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DistancePolicy {
    Standard,
    LocalOnly,
}

pub(crate) fn decode_meta_block_with_base(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    params: MetaBlockDecodeParams<'_>,
    distances: &mut DistanceRing,
) -> Result<(), DecompressError> {
    decode_meta_block_with_base_and_policy(
        reader,
        output,
        params,
        distances,
        DistancePolicy::Standard,
    )
    .map(|_| ())
}

pub(crate) fn decode_meta_block_with_base_and_policy(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    params: MetaBlockDecodeParams<'_>,
    distances: &mut DistanceRing,
    distance_policy: DistancePolicy,
) -> Result<bool, DecompressError> {
    let start = output.len();
    let needed = start
        .checked_add(params.len)
        .ok_or(BurliError::Format("Brotli output length overflow"))?;
    let global_needed = params
        .output_base
        .checked_add(needed)
        .ok_or(BurliError::Format("Brotli output length overflow"))?;
    if global_needed > params.max_output_size {
        return Err(BurliError::OutputLimitExceeded {
            limit: params.max_output_size,
            needed: global_needed,
        });
    }
    let room = needed + OUTPUT_SLACK;
    if output.capacity() < room {
        // The last meta-block ends the output, so it grows by exactly its
        // length and the slack.
        if params.is_last {
            output.reserve_exact(room - start);
        } else {
            output.reserve(room - start);
        }
    }

    let mut header = read_header(reader)?;

    let literal_codes = CodeGroup::read(
        reader,
        header.literal_tree_count,
        LITERAL_ALPHABET_SIZE,
        params.len >= WIDE_ROOT_MIN_LEN,
    )?;
    let command_codes = CodeGroup::read(
        reader,
        header.commands.types(),
        COMMAND_ALPHABET_SIZE,
        false,
    )?;
    let distance_codes = CodeGroup::read(
        reader,
        header.distance_tree_count,
        header.distance_alphabet_size,
        false,
    )?;
    if distance_policy == DistancePolicy::LocalOnly && literal_codes.len() != 1 {
        return Err(BurliError::Format(
            "concat fragment uses literal context state",
        ));
    }
    let window_size = (1_usize << params.window_bits) - 16;
    let body: MetaBlockBody<'_> = MetaBlockBody {
        start,
        needed,
        room,
        output_base: params.output_base,
        window_size,
        literal_codes: &literal_codes,
        command_codes: &command_codes,
        distance_codes: &distance_codes,
        raw_dictionary: params.raw_dictionary,
        single_literal_code: None,
    };
    // A meta-block with one literal code decodes literals through it alone.
    // Its body is built for one root size, so the command loop holds one copy
    // of the literal loop.
    let single = header.literals.types() == 1 && literal_codes.len() == 1;
    let narrow = body.with_single_literal_code(single.then(|| literal_codes.tables().table(0)));
    let result = match (distance_policy, literal_codes.wide_table()) {
        (DistancePolicy::Standard, Some(wide)) if single => {
            let wide = body.with_single_literal_code(Some(wide));
            decode_meta_block_body::<false, WIDE_ROOT_SIZE>(
                reader,
                output,
                &mut header,
                distances,
                wide,
            )
        }
        (DistancePolicy::Standard, _) => decode_meta_block_body::<false, ROOT_SIZE>(
            reader,
            output,
            &mut header,
            distances,
            narrow,
        ),
        (DistancePolicy::LocalOnly, _) => decode_meta_block_body::<true, ROOT_SIZE>(
            reader,
            output,
            &mut header,
            distances,
            narrow,
        ),
    };
    output.truncate(if result.is_ok() { needed } else { start });
    result
}

/// The read-only inputs of one meta-block's command loop.
#[derive(Clone, Copy)]
struct MetaBlockBody<'a, const N: usize = ROOT_SIZE> {
    /// Output position where the meta-block starts.
    start: usize,
    /// Output position where the meta-block ends.
    needed: usize,
    /// `needed` plus [`OUTPUT_SLACK`], the most room the loop zeroes.
    room: usize,
    output_base: usize,
    window_size: usize,
    literal_codes: &'a CodeGroup,
    command_codes: &'a CodeGroup,
    distance_codes: &'a CodeGroup,
    raw_dictionary: RawDictionary<'a>,
    /// The meta-block's one literal code, with a root of `N` entries.
    single_literal_code: Option<Table<'a, N>>,
}

impl<'a, const N: usize> MetaBlockBody<'a, N> {
    const fn with_single_literal_code<const M: usize>(
        self,
        single_literal_code: Option<Table<'a, M>>,
    ) -> MetaBlockBody<'a, M> {
        MetaBlockBody {
            start: self.start,
            needed: self.needed,
            room: self.room,
            output_base: self.output_base,
            window_size: self.window_size,
            literal_codes: self.literal_codes,
            command_codes: self.command_codes,
            distance_codes: self.distance_codes,
            raw_dictionary: self.raw_dictionary,
            single_literal_code,
        }
    }
}

/// Runs the command loop on local copies of `reader` and `distances`. The
/// loop never takes their addresses outside inlined code, so their fields
/// stay in registers. `LOCAL_ONLY` selects [`DistancePolicy::LocalOnly`].
#[inline(never)]
fn decode_meta_block_body<const LOCAL_ONLY: bool, const N: usize>(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    header: &mut CompressedHeader,
    distances: &mut DistanceRing,
    body: MetaBlockBody<'_, N>,
) -> Result<bool, DecompressError> {
    let mut local_reader = reader.clone();
    let mut local_distances = *distances;
    let result = decode_commands::<LOCAL_ONLY, N>(
        &mut local_reader,
        output,
        header,
        &mut local_distances,
        body,
    );
    *reader = local_reader;
    *distances = local_distances;
    result
}

/// Calls `f` with a copy of `reader` and stores the copy back. Use it around
/// calls that are not inlined, so the caller's reader keeps its fields in
/// registers.
#[inline(always)]
fn with_reader_copy<'a, T>(
    reader: &mut BitReader<'a>,
    f: impl FnOnce(&mut BitReader<'a>) -> T,
) -> T {
    let mut copy = reader.clone();
    let result = f(&mut copy);
    *reader = copy;
    result
}

/// The distance codes of distance block type `block_type`, one per distance
/// context.
fn distance_trees<'a>(
    header: &CompressedHeader,
    distance_tables: Tables<'a>,
    block_type: usize,
) -> [Table<'a>; 4] {
    let map = &header.distance_context_map[block_type * 4..block_type * 4 + 4];
    core::array::from_fn(|context| distance_tables.table(map[context]))
}

/// Decodes commands until `output` holds `body.needed` bytes. Returns
/// whether a command copied, which only `LOCAL_ONLY` decodes track; others
/// return `false`.
///
/// Writes go to a slice of zeroed room past the decoded bytes, so they need
/// no capacity checks and the position stays in a register. The room grows
/// [`ZERO_AHEAD`] bytes at a time, so its lines are still cached when the
/// decoder writes them.
///
/// Block counts, the current command code and the current distance codes
/// live in locals and change only at block switches. A category with one
/// block type counts down from `usize::MAX`, which a meta-block of at most
/// 16 MiB never reaches, so the loop needs no single-type flags.
#[inline(always)]
fn decode_commands<const LOCAL_ONLY: bool, const N: usize>(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    header: &mut CompressedHeader,
    distances: &mut DistanceRing,
    body: MetaBlockBody<'_, N>,
) -> Result<bool, DecompressError> {
    let MetaBlockBody {
        start,
        needed,
        room,
        output_base,
        window_size,
        literal_codes,
        command_codes,
        distance_codes,
        raw_dictionary,
        single_literal_code,
    } = body;
    let literal_tables = literal_codes.tables();
    let command_tables = command_codes.tables();
    let distance_tables = distance_codes.tables();
    let literal_shape = LiteralDecodeShape {
        max_bits: usize::from(literal_codes.max_bits()),
        single_symbol: literal_codes.single_symbol(0),
    };
    let no_postfix_distances = header.npostfix == 0 && header.ndirect == 0;
    let mut command_remaining = header.commands.remaining;
    let mut command_code = command_tables.table(header.commands.current_type);
    let mut distance_remaining = header.distances.remaining;
    let mut distance_codes_by_context =
        distance_trees(header, distance_tables, header.distances.current_type);
    let mut literal_remaining = header.literals.remaining;
    let mut literal_block = LiteralBlock::new(
        &header.literal_context_map,
        &header.context_modes,
        &header.literal_trivial_trees,
        header.literals.current_type,
    );
    let mut has_copy = false;
    let mut pos = start;
    let mut out = output.as_mut_slice();

    while pos < needed {
        if command_remaining == 0 {
            let block_type = header.commands.next_block(reader)?;
            command_remaining = header.commands.remaining;
            command_code = command_tables.table(block_type);
        }
        command_remaining -= 1;
        let command = read_command(reader, command_code)?;
        // Every write of this command ends before `reach`. A dictionary
        // word adds at most 16 bytes of affixes to its copy length.
        let reach = pos + command.insert_len + command.copy_len + OUTPUT_SLACK;
        if reach > out.len() {
            out = grow_room(output, reach, room);
        }
        if command.insert_len != 0 {
            if command.insert_len > needed - pos {
                return Err(BurliError::Format(
                    "Brotli literal run exceeds meta-block size",
                ));
            }
            if let Some(table) = single_literal_code {
                copy_literals_single_code(
                    reader,
                    out,
                    &mut pos,
                    command.insert_len,
                    table,
                    literal_shape.single_symbol,
                    literal_shape.max_bits,
                )?;
            } else {
                // Decode block by block so that the context map and mode are
                // fixed for each run.
                let mut count = command.insert_len;
                loop {
                    if literal_remaining == 0 {
                        let block_type = header.literals.next_block(reader)?;
                        literal_remaining = header.literals.remaining;
                        literal_block = LiteralBlock::new(
                            &header.literal_context_map,
                            &header.context_modes,
                            &header.literal_trivial_trees,
                            block_type,
                        );
                    }
                    let run = count.min(literal_remaining);
                    copy_literal_run(
                        reader,
                        out,
                        &mut pos,
                        run,
                        literal_codes,
                        literal_tables,
                        literal_block,
                        literal_shape.max_bits,
                    )?;
                    literal_remaining -= run;
                    count -= run;
                    if count == 0 {
                        break;
                    }
                }
            }
        }
        if pos == needed {
            break;
        }

        if LOCAL_ONLY {
            has_copy = true;
        }
        if LOCAL_ONLY && command.reuse_last_distance {
            return Err(BurliError::Format(
                "concat fragment uses distance-ring state",
            ));
        }

        let distance_symbol = if command.reuse_last_distance {
            0
        } else {
            if distance_remaining == 0 {
                let block_type = header.distances.next_block(reader)?;
                distance_remaining = header.distances.remaining;
                distance_codes_by_context = distance_trees(header, distance_tables, block_type);
            }
            distance_remaining -= 1;
            let code = distance_codes_by_context[command.distance_context & 3];
            decode_prefix_symbol(reader, code)? as usize
        };
        if LOCAL_ONLY && distance_symbol < 16 {
            return Err(BurliError::Format(
                "concat fragment uses short distance code",
            ));
        }
        let distance = if no_postfix_distances {
            read_distance_no_postfix_with_ring(reader, distance_symbol, distances)?
        } else {
            read_distance(
                reader,
                distance_symbol,
                header.npostfix,
                header.ndirect,
                distances,
            )?
        };
        copy_from_distance::<LOCAL_ONLY>(
            out,
            &mut pos,
            CopyRequest {
                needed,
                window_size,
                output_base,
                distance,
                len: command.copy_len,
                push_distance: distance_symbol != 0,
            },
            distances,
            raw_dictionary,
        )?;
    }

    Ok(has_copy)
}

/// Zeroes `output` up to `reach` plus [`ZERO_AHEAD`], but not past `room`,
/// and returns all of it.
#[cold]
#[inline(never)]
fn grow_room(output: &mut Vec<u8>, reach: usize, room: usize) -> &mut [u8] {
    let len = reach.saturating_add(ZERO_AHEAD).min(room);
    if len > output.len() {
        output.resize(len, 0);
    }
    output
}

#[derive(Clone, Copy, Debug)]
struct CopyRequest {
    needed: usize,
    window_size: usize,
    output_base: usize,
    distance: usize,
    len: usize,
    push_distance: bool,
}

#[derive(Clone, Copy, Debug)]
struct LiteralDecodeShape {
    /// Longest literal code length across all literal trees.
    max_bits: usize,
    /// The symbol of literal tree 0 when it has one symbol.
    single_symbol: Option<u16>,
}

/// Writes the bytes of a command's copy to `output[*pos..]` and advances
/// `pos`. `output` holds [`OUTPUT_SLACK`] bytes of room past `needed`.
#[inline(always)]
fn copy_from_distance<const LOCAL_ONLY: bool>(
    output: &mut [u8],
    pos: &mut usize,
    request: CopyRequest,
    distances: &mut DistanceRing,
    raw_dictionary: RawDictionary<'_>,
) -> Result<(), DecompressError> {
    let produced = *pos;
    debug_assert!(produced <= request.needed);
    // A distance within both this output and the window copies from the
    // output. A zero distance wraps to the largest value.
    if request.distance.wrapping_sub(1) >= produced.min(request.window_size) {
        *pos = copy_from_far::<LOCAL_ONLY>(
            output,
            produced,
            request.distance,
            request.len,
            request.needed,
            request.push_distance,
            request.window_size,
            request.output_base,
            distances,
            raw_dictionary,
        )?;
        return Ok(());
    }
    if request.len > request.needed - produced {
        return Err(BurliError::Format("Brotli copy exceeds meta-block size"));
    }

    copy_backward(output, produced, request.distance, request.len);
    *pos = produced + request.len;
    if request.push_distance {
        distances.push(request.distance);
    }
    Ok(())
}

/// Resolves a distance past this output or the window: a dictionary copy
/// when it reaches past the window, an error otherwise. Returns the new
/// output end.
///
/// It takes scalars, not a [`CopyRequest`], so the hot path never stores
/// one to the stack for this call.
#[allow(clippy::too_many_arguments)]
#[cold]
#[inline(never)]
fn copy_from_far<const LOCAL_ONLY: bool>(
    output: &mut [u8],
    pos: usize,
    distance: usize,
    len: usize,
    needed: usize,
    push_distance: bool,
    window_size: usize,
    output_base: usize,
    distances: &mut DistanceRing,
    raw_dictionary: RawDictionary<'_>,
) -> Result<usize, DecompressError> {
    // The caller checked that `output_base + needed` does not overflow.
    let max_allowed_distance = window_size.min(output_base + pos);
    let local = distance.wrapping_sub(1) < pos;
    if LOCAL_ONLY && !local {
        return Err(BurliError::Format(
            "concat fragment uses non-local backward distance",
        ));
    }
    if distance > max_allowed_distance {
        return copy_from_dictionary(
            output,
            pos,
            distance,
            len,
            needed,
            push_distance,
            max_allowed_distance,
            distances,
            raw_dictionary,
        );
    }
    // A local distance within the window takes the hot path.
    Err(BurliError::Format("invalid Brotli backward distance"))
}

/// Resolves a distance past the window: a raw dictionary copy when one is
/// set and the distance reaches into it, a static dictionary word otherwise.
/// Returns the new output end.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn copy_from_dictionary(
    output: &mut [u8],
    pos: usize,
    distance: usize,
    len: usize,
    needed: usize,
    push_distance: bool,
    max_allowed_distance: usize,
    distances: &mut DistanceRing,
    raw_dictionary: RawDictionary<'_>,
) -> Result<usize, DecompressError> {
    let mut static_dictionary_distance_base = max_allowed_distance;
    if !raw_dictionary.is_empty() {
        let raw_dictionary_distance_end = max_allowed_distance
            .checked_add(raw_dictionary.len())
            .ok_or(BurliError::Format(
                "Brotli raw dictionary distance overflow",
            ))?;
        if distance <= raw_dictionary_distance_end {
            let end = crate::dictionary::copy_raw_lz77(
                output,
                pos,
                raw_dictionary,
                distance,
                max_allowed_distance,
                len,
                needed,
            )?;
            if push_distance {
                distances.push(distance);
            }
            return Ok(end);
        }
        static_dictionary_distance_base = raw_dictionary_distance_end;
    }
    crate::dictionary::copy_lookup(
        output,
        pos,
        distance,
        static_dictionary_distance_base,
        len,
        needed,
    )
}

/// Copies `len` bytes from `distance` back to `output[pos..]`, with
/// `0 < distance <= pos` and [`OUTPUT_SLACK`] bytes of room past `pos + len`.
///
/// A short copy that does not overlap its source moves one whole 16- or
/// 32-byte window. The window is loaded before it is stored, so its source
/// may run into the destination; bytes past `pos + len` are later replaced.
#[inline(always)]
fn copy_backward(output: &mut [u8], pos: usize, distance: usize, len: usize) {
    debug_assert!(distance != 0 && distance <= pos);
    debug_assert!(pos + len + OUTPUT_SLACK <= output.len());
    // The caller proved `distance <= pos`. The checked form hands that to
    // the window bounds checks below.
    let Some(src) = pos.checked_sub(distance) else {
        return;
    };
    if distance < len {
        copy_overlapping(output, src, pos, len, distance);
    } else if !(len <= 16 && copy_window::<16>(output, src, pos)
        || len <= 32 && copy_window::<32>(output, src, pos))
    {
        output.copy_within(src..src + len, pos);
    }
}

/// Copies `output[src..src + N]` to `output[dst..dst + N]` for `src < dst`,
/// and returns `false` without writing when the target window does not fit.
#[inline(always)]
fn copy_window<const N: usize>(output: &mut [u8], src: usize, dst: usize) -> bool {
    let Some(end) = dst.checked_add(N).filter(|&end| end <= output.len()) else {
        return false;
    };
    // The source window ends before the target window, so it fits too.
    let Some(&window) = output.get(src..src + N).and_then(<[u8]>::first_chunk::<N>) else {
        return false;
    };
    let Some(target) = output
        .get_mut(dst..end)
        .and_then(<[u8]>::first_chunk_mut::<N>)
    else {
        return false;
    };
    *target = window;
    true
}

/// Copies `len` bytes from `distance` back where source and destination
/// overlap, so the copy repeats the last `distance` bytes.
fn copy_overlapping(output: &mut [u8], src: usize, dst: usize, len: usize, distance: usize) {
    debug_assert!(distance < len);
    if distance >= 16 {
        copy_chunks::<16>(output, src, dst, len);
    } else if distance >= 8 {
        copy_chunks::<8>(output, src, dst, len);
    } else if distance == 1 {
        let byte = output[src];
        output[dst..dst + len].fill(byte);
    } else {
        // Write the pattern byte by byte up to its first multiple of at
        // least 8 bytes. That period then copies in 8-byte chunks.
        let period = distance * 8_usize.div_ceil(distance);
        let head = period.min(len);
        for offset in 0..head {
            output[dst + offset] = output[src + offset];
        }
        if len > head {
            copy_chunks::<8>(output, dst, dst + head, len - head);
        }
    }
}

/// Copies `len` bytes from `src` to `dst` in `N`-byte chunks. The last
/// chunk may write up to `N - 1` bytes past `dst + len`.
///
/// Needs `dst - src >= N`, so a chunk reads only bytes already written, or
/// `dst - src >= len`, so the bytes to copy all lie before `dst`.
#[inline(always)]
fn copy_chunks<const N: usize>(output: &mut [u8], mut src: usize, mut dst: usize, len: usize) {
    debug_assert!(dst - src >= N || dst - src >= len);
    let end = dst + len;
    while dst < end {
        if !copy_window::<N>(output, src, dst) {
            // Unreachable with the room past the meta-block end.
            for offset in 0..end - dst {
                output[dst + offset] = output[src + offset];
            }
            return;
        }
        src += N;
        dst += N;
    }
}

#[derive(Clone, Debug)]
struct CompressedHeader {
    literals: BlockCategory,
    commands: BlockCategory,
    distances: BlockCategory,
    npostfix: u8,
    ndirect: usize,
    context_modes: Vec<u8>,
    literal_tree_count: usize,
    distance_tree_count: usize,
    literal_context_map: Vec<usize>,
    /// The one tree of each literal block type whose context map sends every
    /// context to the same tree. Such a run needs no context.
    literal_trivial_trees: Vec<Option<usize>>,
    distance_context_map: Vec<usize>,
    distance_alphabet_size: usize,
}

#[derive(Clone, Debug)]
struct BlockCategory {
    block_types: usize,
    current_type: usize,
    previous_type: usize,
    remaining: usize,
    type_code: Option<PrefixCode>,
    count_code: Option<PrefixCode>,
}

impl BlockCategory {
    const fn single() -> Self {
        Self {
            block_types: 1,
            current_type: 0,
            previous_type: 1,
            remaining: usize::MAX,
            type_code: None,
            count_code: None,
        }
    }

    fn new(
        block_types: usize,
        type_code: PrefixCode,
        count_code: PrefixCode,
        remaining: usize,
    ) -> Self {
        Self {
            block_types,
            current_type: 0,
            previous_type: 1,
            remaining,
            type_code: Some(type_code),
            count_code: Some(count_code),
        }
    }

    const fn types(&self) -> usize {
        self.block_types
    }

    /// Reads the next block switch and returns the new block type. Its
    /// count is in `remaining`.
    #[inline(always)]
    fn next_block(&mut self, reader: &mut BitReader<'_>) -> Result<usize, DecompressError> {
        with_reader_copy(reader, |reader| self.switch(reader))?;
        Ok(self.current_type)
    }

    #[cold]
    #[inline(never)]
    fn switch(&mut self, reader: &mut BitReader<'_>) -> Result<(), DecompressError> {
        let type_code = self
            .type_code
            .as_ref()
            .ok_or(BurliError::Format("missing Brotli block type code"))?;
        let count_code = self
            .count_code
            .as_ref()
            .ok_or(BurliError::Format("missing Brotli block count code"))?;
        let symbol = type_code.decode(reader)? as usize;
        let next_type = match symbol {
            0 => self.previous_type,
            1 => (self.current_type + 1) % self.block_types,
            _ => {
                let value = symbol - 2;
                if value >= self.block_types {
                    return Err(BurliError::Format("invalid Brotli block type"));
                }
                value
            }
        };
        self.previous_type = self.current_type;
        self.current_type = next_type;
        self.remaining = read_block_count(reader, count_code)?;
        Ok(())
    }
}

fn read_header(reader: &mut BitReader<'_>) -> Result<CompressedHeader, DecompressError> {
    let literals = read_block_category_header(reader)?;
    let commands = read_block_category_header(reader)?;
    let distances = read_block_category_header(reader)?;
    let npostfix = reader.read_bits(2)? as u8;
    let ndirect = (reader.read_bits(4)? as usize) << npostfix;

    let mut context_modes = Vec::with_capacity(literals.types());
    for _ in 0..literals.types() {
        context_modes.push(reader.read_bits(2)? as u8);
    }

    let literal_trees = read_var_len_u8(reader)? + 1;
    let literal_context_map = read_context_map(reader, literals.types() * 64, literal_trees)?;
    let literal_trivial_trees = literal_context_map
        .as_chunks::<64>()
        .0
        .iter()
        .map(|map| map.iter().all(|&tree| tree == map[0]).then_some(map[0]))
        .collect();

    let distance_trees = read_var_len_u8(reader)? + 1;
    let distance_context_map = read_context_map(reader, distances.types() * 4, distance_trees)?;

    let distance_alphabet_size = 16 + ndirect + (48 << npostfix);
    Ok(CompressedHeader {
        literals,
        commands,
        distances,
        npostfix,
        ndirect,
        context_modes,
        literal_tree_count: literal_trees,
        distance_tree_count: distance_trees,
        literal_context_map,
        literal_trivial_trees,
        distance_context_map,
        distance_alphabet_size,
    })
}

fn read_block_category_header(
    reader: &mut BitReader<'_>,
) -> Result<BlockCategory, DecompressError> {
    let block_types = read_var_len_u8(reader)? + 1;
    if block_types == 1 {
        return Ok(BlockCategory::single());
    }

    let block_type_code = PrefixCode::read(reader, block_types + 2)?;
    let block_count_code = PrefixCode::read(reader, BLOCK_LENGTH_ALPHABET_SIZE)?;
    let first_block_count = read_block_count(reader, &block_count_code)?;
    Ok(BlockCategory::new(
        block_types,
        block_type_code,
        block_count_code,
        first_block_count,
    ))
}

fn read_var_len_u8(reader: &mut BitReader<'_>) -> Result<usize, DecompressError> {
    if !reader.read_bit()? {
        return Ok(0);
    }

    let width = reader.read_bits(3)? as u8;
    if width == 0 {
        return Ok(1);
    }

    let extra = reader.read_bits(width)?;
    Ok((1_usize << width) + extra as usize)
}

fn read_context_map(
    reader: &mut BitReader<'_>,
    size: usize,
    tree_count: usize,
) -> Result<Vec<usize>, DecompressError> {
    if tree_count == 0 || tree_count > 256 {
        return Err(BurliError::Format("invalid Brotli context tree count"));
    }
    if tree_count == 1 {
        return Ok(vec![0; size]);
    }

    let rlemax = read_context_rlemax(reader)?;
    let code = PrefixCode::read(reader, tree_count + rlemax)?;
    let mut map = Vec::with_capacity(size);
    while map.len() < size {
        let symbol = code.decode(reader)? as usize;
        if rlemax != 0 && (1..=rlemax).contains(&symbol) {
            let repeat = (1_usize << symbol) + reader.read_bits(symbol as u8)? as usize;
            let end = map
                .len()
                .checked_add(repeat)
                .ok_or(BurliError::Format("Brotli context map repeat overflow"))?;
            if end > size {
                return Err(BurliError::Format("Brotli context map repeat exceeds size"));
            }
            map.resize(end, 0);
        } else {
            let value = if rlemax == 0 {
                symbol
            } else {
                symbol.saturating_sub(rlemax)
            };
            if value >= tree_count {
                return Err(BurliError::Format("invalid Brotli context map value"));
            }
            map.push(value);
        }
    }

    if reader.read_bit()? {
        inverse_move_to_front(&mut map)?;
    }
    Ok(map)
}

fn read_context_rlemax(reader: &mut BitReader<'_>) -> Result<usize, DecompressError> {
    if !reader.read_bit()? {
        return Ok(0);
    }
    Ok(reader.read_bits(4)? as usize + 1)
}

fn inverse_move_to_front(map: &mut [usize]) -> Result<(), DecompressError> {
    let mut mtf = [0_usize; 256];
    for (index, slot) in mtf.iter_mut().enumerate() {
        *slot = index;
    }

    for value in map {
        let index = *value;
        let Some(&resolved) = mtf.get(index) else {
            return Err(BurliError::Format("invalid Brotli move-to-front index"));
        };
        for shift in (1..=index).rev() {
            mtf[shift] = mtf[shift - 1];
        }
        mtf[0] = resolved;
        *value = resolved;
    }
    Ok(())
}

fn read_block_count(
    reader: &mut BitReader<'_>,
    code: &PrefixCode,
) -> Result<usize, DecompressError> {
    let symbol = code.decode(reader)? as usize;
    let (base, extra_bits) = block_count_prefix(symbol)?;
    Ok(base + reader.read_bits(extra_bits)? as usize)
}

fn block_count_prefix(symbol: usize) -> Result<(usize, u8), DecompressError> {
    match symbol {
        0 => Ok((1, 2)),
        1 => Ok((5, 2)),
        2 => Ok((9, 2)),
        3 => Ok((13, 2)),
        4 => Ok((17, 3)),
        5 => Ok((25, 3)),
        6 => Ok((33, 3)),
        7 => Ok((41, 3)),
        8 => Ok((49, 4)),
        9 => Ok((65, 4)),
        10 => Ok((81, 4)),
        11 => Ok((97, 4)),
        12 => Ok((113, 5)),
        13 => Ok((145, 5)),
        14 => Ok((177, 5)),
        15 => Ok((209, 5)),
        16 => Ok((241, 6)),
        17 => Ok((305, 6)),
        18 => Ok((369, 7)),
        19 => Ok((497, 8)),
        20 => Ok((753, 9)),
        21 => Ok((1265, 10)),
        22 => Ok((2289, 11)),
        23 => Ok((4337, 12)),
        24 => Ok((8433, 13)),
        25 => Ok((16625, 24)),
        _ => Err(BurliError::Format("invalid Brotli block count code")),
    }
}

#[derive(Clone, Copy, Debug)]
struct Command {
    insert_len: usize,
    copy_len: usize,
    reuse_last_distance: bool,
    distance_context: usize,
}

const COMMAND_LENGTH_CODE_BASES: [(usize, usize); 11] = [
    (0, 0),
    (0, 8),
    (0, 0),
    (0, 8),
    (8, 0),
    (8, 8),
    (0, 16),
    (16, 0),
    (8, 16),
    (16, 8),
    (16, 16),
];
const COMMAND_DISTANCE_CONTEXTS: [[usize; 8]; 11] = [
    [0, 1, 2, 3, 3, 3, 3, 3],
    [3; 8],
    [0, 1, 2, 3, 3, 3, 3, 3],
    [3; 8],
    [0, 1, 2, 3, 3, 3, 3, 3],
    [3; 8],
    [3; 8],
    [0, 1, 2, 3, 3, 3, 3, 3],
    [3; 8],
    [3; 8],
    [3; 8],
];
const COMMAND_PREFIX_TABLE_SIZE: usize = 4096;
static COMMAND_PREFIXES: [CommandPrefix; COMMAND_PREFIX_TABLE_SIZE] = command_prefixes();

#[derive(Clone, Copy, Debug)]
struct CommandPrefix(u64);

impl CommandPrefix {
    const COPY_BASE_SHIFT: u64 = 16;
    const INSERT_EXTRA_SHIFT: u64 = 32;
    const COPY_EXTRA_SHIFT: u64 = 37;
    const DISTANCE_CONTEXT_SHIFT: u64 = 42;
    const REUSE_LAST_DISTANCE_SHIFT: u64 = 44;
    const U16_MASK: u64 = 0xffff;
    const U5_MASK: u64 = 0x1f;
    const U2_MASK: u64 = 0x03;

    const fn new(
        insert_base: usize,
        copy_base: usize,
        insert_extra_bits: u8,
        copy_extra_bits: u8,
        distance_context: usize,
        reuse_last_distance: bool,
    ) -> Self {
        Self(
            (insert_base as u64)
                | ((copy_base as u64) << Self::COPY_BASE_SHIFT)
                | ((insert_extra_bits as u64) << Self::INSERT_EXTRA_SHIFT)
                | ((copy_extra_bits as u64) << Self::COPY_EXTRA_SHIFT)
                | ((distance_context as u64) << Self::DISTANCE_CONTEXT_SHIFT)
                | ((reuse_last_distance as u64) << Self::REUSE_LAST_DISTANCE_SHIFT),
        )
    }

    const fn insert_base(self) -> usize {
        (self.0 & Self::U16_MASK) as usize
    }

    const fn copy_base(self) -> usize {
        ((self.0 >> Self::COPY_BASE_SHIFT) & Self::U16_MASK) as usize
    }

    const fn insert_extra_bits(self) -> u8 {
        ((self.0 >> Self::INSERT_EXTRA_SHIFT) & Self::U5_MASK) as u8
    }

    const fn copy_extra_bits(self) -> u8 {
        ((self.0 >> Self::COPY_EXTRA_SHIFT) & Self::U5_MASK) as u8
    }

    const fn distance_context(self) -> usize {
        ((self.0 >> Self::DISTANCE_CONTEXT_SHIFT) & Self::U2_MASK) as usize
    }

    const fn reuse_last_distance(self) -> bool {
        ((self.0 >> Self::REUSE_LAST_DISTANCE_SHIFT) & 1) != 0
    }
}

#[allow(clippy::large_stack_arrays)]
const fn command_prefixes() -> [CommandPrefix; COMMAND_PREFIX_TABLE_SIZE] {
    let mut prefixes = [CommandPrefix(0); COMMAND_PREFIX_TABLE_SIZE];
    let mut code = 0;
    while code < COMMAND_ALPHABET_SIZE {
        prefixes[code] = command_prefix(code);
        code += 1;
    }
    prefixes
}

const fn command_prefix(code: usize) -> CommandPrefix {
    let low = code & 0b111;
    let high = (code >> 3) & 0b111;
    let cell = code >> 6;
    let (insert_base_code, copy_base_code) = COMMAND_LENGTH_CODE_BASES[cell];
    let (insert_base, insert_extra_bits) = INSERT_LENGTH_PREFIXES[insert_base_code + high];
    let (copy_base, copy_extra_bits) = COPY_LENGTH_PREFIXES[copy_base_code + low];
    CommandPrefix::new(
        insert_base,
        copy_base,
        insert_extra_bits,
        copy_extra_bits,
        COMMAND_DISTANCE_CONTEXTS[cell][low],
        code < 128,
    )
}
const INSERT_LENGTH_PREFIXES: [(usize, u8); 24] = [
    (0, 0),
    (1, 0),
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 1),
    (8, 1),
    (10, 2),
    (14, 2),
    (18, 3),
    (26, 3),
    (34, 4),
    (50, 4),
    (66, 5),
    (98, 5),
    (130, 6),
    (194, 7),
    (322, 8),
    (578, 9),
    (1090, 10),
    (2114, 12),
    (6210, 14),
    (22594, 24),
];
const COPY_LENGTH_PREFIXES: [(usize, u8); 24] = [
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 0),
    (7, 0),
    (8, 0),
    (9, 0),
    (10, 1),
    (12, 1),
    (14, 2),
    (18, 2),
    (22, 3),
    (30, 3),
    (38, 4),
    (54, 4),
    (70, 5),
    (102, 5),
    (134, 6),
    (198, 7),
    (326, 8),
    (582, 9),
    (1094, 10),
    (2118, 24),
];

#[inline(always)]
fn read_command(
    reader: &mut BitReader<'_>,
    command_code: Table<'_>,
) -> Result<Command, DecompressError> {
    let code = usize::from(decode_prefix_symbol(reader, command_code)? & 0x0fff);
    debug_assert!(code < COMMAND_ALPHABET_SIZE);
    let prefix = COMMAND_PREFIXES[code];
    let insert_extra = reader.read_short_bits(prefix.insert_extra_bits())? as usize;
    let copy_extra = reader.read_short_bits(prefix.copy_extra_bits())? as usize;
    let insert_len = prefix.insert_base() + insert_extra;
    let copy_len = prefix.copy_base() + copy_extra;

    Ok(Command {
        insert_len,
        copy_len,
        reuse_last_distance: prefix.reuse_last_distance(),
        distance_context: prefix.distance_context(),
    })
}

/// Decodes a command or distance symbol. The unconditional refill also
/// covers the extra bits that follow, so their reads rarely refill again.
#[inline(always)]
fn decode_prefix_symbol(
    reader: &mut BitReader<'_>,
    code: Table<'_>,
) -> Result<u16, DecompressError> {
    reader.refill();
    code.decode_refilled(reader)
}

#[cfg(test)]
fn command_code_parts(code: usize) -> Result<(usize, usize, bool, usize), DecompressError> {
    if code >= COMMAND_ALPHABET_SIZE {
        return Err(BurliError::Format("invalid Brotli command code"));
    }

    let low = code & 0b111;
    let high = (code >> 3) & 0b111;
    let cell = code >> 6;
    let reuse_last_distance = code < 128;
    let (insert_base, copy_base) = match cell {
        0 | 2 => (0, 0),
        1 | 3 => (0, 8),
        4 => (8, 0),
        5 => (8, 8),
        6 => (0, 16),
        7 => (16, 0),
        8 => (8, 16),
        9 => (16, 8),
        10 => (16, 16),
        _ => return Err(BurliError::Format("invalid Brotli command code")),
    };

    let distance_context = if matches!(cell, 0 | 2 | 4 | 7) && low <= 2 {
        low
    } else {
        3
    };

    Ok((
        insert_base + high,
        copy_base + low,
        reuse_last_distance,
        distance_context,
    ))
}

/// The literal block type in force: its context map and mode, and its one
/// tree when every context maps to it.
#[derive(Clone, Copy, Debug)]
struct LiteralBlock<'a> {
    context_map: &'a [usize; 64],
    mode: u8,
    trivial_tree: Option<usize>,
}

impl<'a> LiteralBlock<'a> {
    /// The literal block of `block_type`. It takes header fields, not the
    /// header, so the command loop can switch other categories meanwhile.
    fn new(
        context_map: &'a [usize],
        context_modes: &[u8],
        trivial_trees: &[Option<usize>],
        block_type: usize,
    ) -> Self {
        let map_start = block_type * 64;
        Self {
            context_map: context_map[map_start..map_start + 64]
                .try_into()
                .expect("64 contexts per literal block type"),
            mode: context_modes[block_type],
            trivial_tree: trivial_trees[block_type],
        }
    }
}

/// Decodes `count` literals of literal block `block` to `output[*pos..]`
/// and advances `pos`. A block whose contexts all share one tree skips the
/// context per literal.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn copy_literal_run(
    reader: &mut BitReader<'_>,
    output: &mut [u8],
    pos: &mut usize,
    count: usize,
    literal_codes: &CodeGroup,
    literal_tables: Tables<'_>,
    block: LiteralBlock<'_>,
    max_bits: usize,
) -> Result<(), DecompressError> {
    if let Some(tree) = block.trivial_tree {
        return copy_literals_single_code(
            reader,
            output,
            pos,
            count,
            literal_tables.table(tree),
            literal_codes.single_symbol(tree),
            max_bits,
        );
    }
    copy_literals_single_block(
        reader,
        output,
        pos,
        count,
        literal_tables,
        block.context_map,
        block.mode,
        max_bits,
    )
}

/// Decodes `count` literals of the one literal code `table`, whose only
/// symbol is `single` when it has one.
#[inline(always)]
fn copy_literals_single_code<const N: usize>(
    reader: &mut BitReader<'_>,
    output: &mut [u8],
    pos: &mut usize,
    count: usize,
    table: Table<'_, N>,
    single: Option<u16>,
    max_bits: usize,
) -> Result<(), DecompressError> {
    let slots = &mut output[*pos..*pos + count];
    *pos += count;
    if let Some(symbol) = single {
        slots.fill(symbol as u8);
        return Ok(());
    }
    if max_bits != 0
        && count
            .checked_mul(max_bits)
            .is_some_and(|bits| bits <= reader.remaining_bits())
    {
        copy_literals_single_code_trusted_fast(reader, slots, table);
        return Ok(());
    }
    for slot in slots {
        *slot = table.decode(reader)? as u8;
    }
    Ok(())
}

/// Three 15-bit codes fit in the 57 bits one padded peek provides.
const LITERALS_PER_REFILL: usize = 3;

/// Decodes three literals from one bit refill. The caller proves that the
/// input holds three codes of the longest length.
#[inline(always)]
fn decode_three_literals<const N: usize>(
    reader: &mut BitReader<'_>,
    table: Table<'_, N>,
) -> [u8; 3] {
    let mut bits = reader.peek_bits_padded();
    let (a, a_len) = table.lookup_bits(bits);
    bits >>= a_len;
    let (b, b_len) = table.lookup_bits(bits);
    bits >>= b_len;
    let (c, c_len) = table.lookup_bits(bits);
    reader.drop_bits_trusted(a_len + b_len + c_len);
    [a as u8, b as u8, c as u8]
}

/// Fills `slots` with literals of `table`. The caller proves that the input
/// holds `slots.len()` codes of the longest length.
#[inline(always)]
fn copy_literals_single_code_trusted_fast<const N: usize>(
    reader: &mut BitReader<'_>,
    slots: &mut [u8],
    table: Table<'_, N>,
) {
    let (triples, rest) = slots.as_chunks_mut::<LITERALS_PER_REFILL>();
    for triple in triples {
        *triple = decode_three_literals(reader, table);
    }
    for slot in rest {
        *slot = table.decode_trusted(reader) as u8;
    }
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn copy_literals_single_block(
    reader: &mut BitReader<'_>,
    output: &mut [u8],
    pos: &mut usize,
    count: usize,
    literal_tables: Tables<'_>,
    context_map: &[usize; 64],
    mode: u8,
    max_bits: usize,
) -> Result<(), DecompressError> {
    let mut previous = previous_literal_bytes(&output[..*pos]);
    let slots = &mut output[*pos..*pos + count];
    *pos += count;
    if count
        .checked_mul(max_bits)
        .is_some_and(|bits| bits <= reader.remaining_bits())
    {
        copy_literals_single_block_trusted(
            reader,
            slots,
            previous,
            literal_tables,
            context_map,
            mode,
        );
        return Ok(());
    }

    match mode {
        0 => {
            for slot in slots {
                let tree_index = context_map[usize::from(previous.0 & 0x3f)];
                let literal = read_literal(reader, literal_tables.table(tree_index))?;
                *slot = literal;
                previous = (literal, previous.0);
            }
        }
        1 => {
            for slot in slots {
                let tree_index = context_map[usize::from(previous.0 >> 2)];
                let literal = read_literal(reader, literal_tables.table(tree_index))?;
                *slot = literal;
                previous = (literal, previous.0);
            }
        }
        2 => {
            for slot in slots {
                let context = crate::context_lookup::literal_context::<2>(previous.0, previous.1);
                let tree_index = context_map[usize::from(context)];
                let literal = read_literal(reader, literal_tables.table(tree_index))?;
                *slot = literal;
                previous = (literal, previous.0);
            }
        }
        3 => {
            for slot in slots {
                let context = crate::context_lookup::literal_context::<3>(previous.0, previous.1);
                let tree_index = context_map[usize::from(context)];
                let literal = read_literal(reader, literal_tables.table(tree_index))?;
                *slot = literal;
                previous = (literal, previous.0);
            }
        }
        _ => return Err(BurliError::Format("invalid literal context mode")),
    }
    Ok(())
}

#[inline(always)]
fn copy_literals_single_block_trusted(
    reader: &mut BitReader<'_>,
    slots: &mut [u8],
    previous: (u8, u8),
    literal_tables: Tables<'_>,
    context_map: &[usize; 64],
    mode: u8,
) {
    match mode {
        0 => copy_literals_single_block_trusted_mode::<0>(
            reader,
            slots,
            previous,
            literal_tables,
            context_map,
        ),
        1 => copy_literals_single_block_trusted_mode::<1>(
            reader,
            slots,
            previous,
            literal_tables,
            context_map,
        ),
        2 => copy_literals_single_block_trusted_mode::<2>(
            reader,
            slots,
            previous,
            literal_tables,
            context_map,
        ),
        3 => copy_literals_single_block_trusted_mode::<3>(
            reader,
            slots,
            previous,
            literal_tables,
            context_map,
        ),
        _ => unreachable!(),
    }
}

/// Fills `slots` with literals that follow the bytes `previous`. The caller
/// proves that the input holds `slots.len()` codes of the longest length.
/// A single-symbol code decodes through its zero-length entries, so every
/// literal takes the same path.
#[inline(always)]
fn copy_literals_single_block_trusted_mode<const MODE: u8>(
    reader: &mut BitReader<'_>,
    slots: &mut [u8],
    mut previous: (u8, u8),
    literal_tables: Tables<'_>,
    context_map: &[usize; 64],
) {
    let (triples, rest) = slots.as_chunks_mut::<LITERALS_PER_REFILL>();
    // One refill covers three codes of at most 15 bits. It replaces a
    // per-literal "buffer low" branch that mispredicts often.
    for triple in triples {
        reader.refill();
        for slot in triple {
            let table = literal_tables.table(context_map[literal_context_for_mode(previous, MODE)]);
            let literal = table.decode_buffered(reader) as u8;
            *slot = literal;
            previous = (literal, previous.0);
        }
    }
    for slot in rest {
        let table = literal_tables.table(context_map[literal_context_for_mode(previous, MODE)]);
        let literal = read_literal_trusted(reader, table);
        *slot = literal;
        previous = (literal, previous.0);
    }
}

#[inline(always)]
fn read_literal(reader: &mut BitReader<'_>, table: Table<'_>) -> Result<u8, DecompressError> {
    Ok(table.decode(reader)? as u8)
}

#[inline(always)]
fn read_literal_trusted(reader: &mut BitReader<'_>, table: Table<'_>) -> u8 {
    table.decode_trusted(reader) as u8
}

#[inline(always)]
fn previous_literal_bytes(output: &[u8]) -> (u8, u8) {
    let len = output.len();
    if len >= 2 {
        (output[len - 1], output[len - 2])
    } else if len == 1 {
        (output[0], 0)
    } else {
        (0, 0)
    }
}

#[inline(always)]
fn literal_context_for_mode(previous: (u8, u8), mode: u8) -> usize {
    let (p1, p2) = previous;
    match mode {
        0 => p1 & 0x3f,
        1 => p1 >> 2,
        2 => crate::context_lookup::literal_context::<2>(p1, p2),
        _ => {
            debug_assert_eq!(mode, 3);
            crate::context_lookup::literal_context::<3>(p1, p2)
        }
    }
    .into()
}

#[inline(always)]
fn read_distance(
    reader: &mut BitReader<'_>,
    symbol: usize,
    npostfix: u8,
    ndirect: usize,
    distances: &DistanceRing,
) -> Result<usize, DecompressError> {
    if symbol < 16 {
        return distances.resolve(symbol);
    }
    if npostfix == 0 && ndirect == 0 {
        return read_distance_no_postfix(reader, symbol);
    }
    if symbol < 16 + ndirect {
        return Ok(symbol - 15);
    }

    let adjusted = symbol - ndirect - 16;
    let ndistbits = 1 + (adjusted >> (npostfix + 1));
    if ndistbits > 24 {
        return Err(BurliError::Format("invalid Brotli distance extra bits"));
    }
    let dextra = reader.read_short_bits(ndistbits as u8)? as usize;
    let hcode = adjusted >> npostfix;
    let lcode = adjusted & ((1_usize << npostfix) - 1);
    let offset = ((2 + (hcode & 1)) << ndistbits) - 4;
    Ok(((offset + dextra) << npostfix) + lcode + ndirect + 1)
}

#[inline(always)]
fn read_distance_no_postfix_with_ring(
    reader: &mut BitReader<'_>,
    symbol: usize,
    distances: &DistanceRing,
) -> Result<usize, DecompressError> {
    if symbol < 16 {
        return distances.resolve(symbol);
    }
    read_distance_no_postfix(reader, symbol)
}

#[inline(always)]
fn read_distance_no_postfix(
    reader: &mut BitReader<'_>,
    symbol: usize,
) -> Result<usize, DecompressError> {
    let adjusted = symbol - 16;
    let ndistbits = 1 + (adjusted >> 1);
    if ndistbits > 24 {
        return Err(BurliError::Format("invalid Brotli distance extra bits"));
    }
    let dextra = reader.read_short_bits(ndistbits as u8)? as usize;
    let offset = ((2 + (adjusted & 1)) << ndistbits) - 4;
    Ok(offset + dextra + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use burli_core::bits::BitWriter;

    /// Decodes the single meta-block of a small stream into an empty output
    /// one byte short of its length, and returns the output.
    fn decode_single_meta_block(is_last: bool) -> (Vec<u8>, Vec<u8>) {
        let data: Vec<u8> = (0..3000_u32).map(|i| (i * 7 % 251) as u8).collect();
        let stream = burli::compress(&data, 1).unwrap();
        let mut reader = BitReader::new(&stream);
        let window_bits = crate::stored::read_window_bits(&mut reader).unwrap();
        let crate::stored::MetaBlockHeader::Compressed { len, .. } =
            crate::stored::read_meta_block_header(&mut reader).unwrap()
        else {
            panic!("expected a compressed meta-block");
        };
        let mut output = Vec::with_capacity(len - 1);
        decode_meta_block_with_base(
            &mut reader,
            &mut output,
            MetaBlockDecodeParams {
                output_base: 0,
                len,
                is_last,
                max_output_size: usize::MAX,
                window_bits,
                raw_dictionary: RawDictionary::empty(),
            },
            &mut DistanceRing::new(),
        )
        .unwrap();
        (output, data)
    }

    #[test]
    fn last_meta_block_grows_the_output_by_its_length() {
        let (output, data) = decode_single_meta_block(true);
        assert_eq!(output, data);
        assert_eq!(output.capacity(), data.len() + OUTPUT_SLACK);
    }

    #[test]
    fn earlier_meta_blocks_grow_the_output_geometrically() {
        let (output, data) = decode_single_meta_block(false);
        assert_eq!(output, data);
        assert_eq!(output.capacity(), 2 * (data.len() - 1));
    }

    #[test]
    fn reads_single_type_headers_without_prefix_codes() {
        let mut bits = BitWriter::new();
        bits.write_bits(1, 0).unwrap();
        bits.write_bits(1, 0).unwrap();
        bits.write_bits(1, 0).unwrap();
        let bytes = bits.into_bytes();
        let mut reader = BitReader::new(&bytes);
        let literal = read_block_category_header(&mut reader).unwrap();
        let command = read_block_category_header(&mut reader).unwrap();
        let distance = read_block_category_header(&mut reader).unwrap();

        assert_eq!(literal.types(), 1);
        assert_eq!(command.types(), 1);
        assert_eq!(distance.types(), 1);
    }

    #[test]
    fn reads_var_len_u8_boundaries() {
        let mut bits = BitWriter::new();
        bits.write_bits(1, 0).unwrap();
        bits.write_bits(1, 1).unwrap();
        bits.write_bits(3, 0).unwrap();
        bits.write_bits(1, 1).unwrap();
        bits.write_bits(3, 3).unwrap();
        bits.write_bits(3, 5).unwrap();
        let bytes = bits.into_bytes();
        let mut reader = BitReader::new(&bytes);

        assert_eq!(read_var_len_u8(&mut reader).unwrap(), 0);
        assert_eq!(read_var_len_u8(&mut reader).unwrap(), 1);
        assert_eq!(read_var_len_u8(&mut reader).unwrap(), 13);
    }

    /// Runs [`copy_from_distance`] after `history` in zeroed room for
    /// `needed`. Returns the output up to the copy end and the distance ring.
    fn copy_after(
        history: &[u8],
        needed: usize,
        distance: usize,
        len: usize,
        push_distance: bool,
    ) -> (Vec<u8>, DistanceRing) {
        let mut output = history.to_vec();
        output.resize(needed + OUTPUT_SLACK, 0);
        let mut pos = history.len();
        let mut distances = DistanceRing::new();
        copy_from_distance::<false>(
            &mut output,
            &mut pos,
            CopyRequest {
                needed,
                window_size: 1 << 16,
                output_base: 0,
                distance,
                len,
                push_distance,
            },
            &mut distances,
            RawDictionary::empty(),
        )
        .unwrap();
        output.truncate(pos);
        (output, distances)
    }

    #[test]
    fn distance_symbol_zero_does_not_update_ring() {
        let (_, distances) = copy_after(b"0123456789abcdef", 20, 4, 4, false);

        let mut reader = BitReader::new(&[]);
        assert_eq!(read_distance(&mut reader, 1, 0, 0, &distances).unwrap(), 11);
    }

    #[test]
    fn distance_one_copy_repeats_last_byte() {
        let (output, distances) = copy_after(b"aaaaab", 10, 1, 4, true);

        assert_eq!(output, b"aaaaabbbbb");
        let mut reader = BitReader::new(&[]);
        assert_eq!(read_distance(&mut reader, 0, 0, 0, &distances).unwrap(), 1);
    }

    #[test]
    fn small_distance_copy_repeats_source_pattern() {
        let (output, distances) = copy_after(b"abcdef", 14, 3, 8, true);

        assert_eq!(output, b"abcdefdefdefde");
        let mut reader = BitReader::new(&[]);
        assert_eq!(read_distance(&mut reader, 0, 0, 0, &distances).unwrap(), 3);
    }

    #[test]
    fn backward_copy_handles_non_overlapping_range() {
        let (output, distances) = copy_after(b"abcdefghijklmnop", 20, 16, 4, true);

        assert_eq!(output, b"abcdefghijklmnopabcd");
        let mut reader = BitReader::new(&[]);
        assert_eq!(read_distance(&mut reader, 0, 0, 0, &distances).unwrap(), 16);
    }

    #[test]
    fn trusted_fast_literal_bulk_copy_matches_push_loop() {
        let code = PrefixCode::from_lengths(&[1, 1]).unwrap();
        let mut bits = BitWriter::new();
        for bit in [0_u64, 1, 0, 1, 1, 0] {
            bits.write_bits(1, bit).unwrap();
        }
        let bytes = bits.into_bytes();
        let mut reader = BitReader::new(&bytes);
        let mut slots = [0xff_u8; 6];

        copy_literals_single_code_trusted_fast(&mut reader, &mut slots, code.table());

        assert_eq!(slots, [0, 1, 0, 1, 1, 0]);
    }

    #[test]
    fn backward_copy_doubles_large_overlapping_range() {
        let (output, distances) = copy_after(b"abcdefghijklmnop", 56, 16, 40, true);

        assert_eq!(
            output,
            b"abcdefghijklmnopabcdefghijklmnopabcdefghijklmnopabcdefgh"
        );
        let mut reader = BitReader::new(&[]);
        assert_eq!(read_distance(&mut reader, 0, 0, 0, &distances).unwrap(), 16);
    }

    #[test]
    fn backward_copy_matches_byte_copy() {
        let history: Vec<u8> = (0..160_u8).map(|byte| byte.wrapping_mul(37)).collect();
        for distance in 1..=history.len() {
            for len in 1..=160 {
                let mut expected = history.clone();
                for _ in 0..len {
                    expected.push(expected[expected.len() - distance]);
                }

                let (output, _) = copy_after(&history, history.len() + len, distance, len, true);

                assert_eq!(output, expected, "distance {distance} len {len}");
            }
        }
    }

    #[test]
    fn command_distance_context_comes_from_command_prefix() {
        assert_eq!(command_code_parts(0).unwrap().3, 0);
        assert_eq!(command_code_parts(2).unwrap().3, 2);
        assert_eq!(command_code_parts(3).unwrap().3, 3);
        assert_eq!(command_code_parts(64).unwrap().3, 3);
        assert_eq!(command_code_parts(256 + 2).unwrap().3, 2);
    }

    #[test]
    fn literal_context_uses_zero_second_previous_byte_until_two_bytes_exist() {
        assert_eq!(
            literal_context_for_mode(previous_literal_bytes(b"\r"), 3),
            8
        );
        assert_eq!(
            literal_context_for_mode(previous_literal_bytes(b"\r\n"), 3),
            9
        );
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    fn default_short_distances_are_non_zero() {
        let symbol = kani::any::<u8>();
        kani::assume(symbol < 16);

        let distance = match DistanceRing::new().resolve(usize::from(symbol)) {
            Ok(distance) => distance,
            Err(_) => 0,
        };

        assert!(distance > 0);
    }
}
