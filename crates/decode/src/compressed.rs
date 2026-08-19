use alloc::vec;
use alloc::vec::Vec;

use burli_core::{BurliError, DecompressError, bits::BitReader};

use crate::{dictionary::RawDictionary, huffman::PrefixCode};

const LITERAL_ALPHABET_SIZE: usize = 256;
const COMMAND_ALPHABET_SIZE: usize = 704;
const BLOCK_LENGTH_ALPHABET_SIZE: usize = 26;
const LAST_DISTANCES: [usize; 4] = [16, 15, 11, 4];
const CHUNKED_COPY_MIN_DISTANCE: usize = 8;

#[derive(Clone, Debug)]
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
    output.reserve(needed - output.len());

    let mut header = read_header(reader)?;

    let literal_codes =
        read_prefix_codes(reader, header.literal_tree_count(), LITERAL_ALPHABET_SIZE)?;
    let command_codes = read_prefix_codes(reader, header.commands.types(), COMMAND_ALPHABET_SIZE)?;
    let distance_codes = read_prefix_codes(
        reader,
        header.distance_tree_count(),
        header.distance_alphabet_size,
    )?;
    if distance_policy == DistancePolicy::LocalOnly && literal_codes.len() != 1 {
        return Err(BurliError::Format(
            "concat fragment uses literal context state",
        ));
    }
    let window_size = (1_usize << params.window_bits) - 16;
    decode_meta_block_body(
        reader,
        output,
        needed,
        params.output_base,
        window_size,
        &mut header,
        &literal_codes,
        &command_codes,
        &distance_codes,
        distances,
        params.raw_dictionary,
        distance_policy,
    )
}

/// Validate one compressed meta-block without materializing decoded bytes.
pub(crate) fn validate_meta_block(
    reader: &mut BitReader<'_>,
    len: usize,
    output_base: usize,
    window_size: usize,
    distances: &mut DistanceRing,
) -> Result<(), DecompressError> {
    let mut header = read_header(reader)?;
    let literal_codes =
        read_prefix_codes(reader, header.literal_tree_count(), LITERAL_ALPHABET_SIZE)?;
    let command_codes = read_prefix_codes(reader, header.commands.types(), COMMAND_ALPHABET_SIZE)?;
    let distance_codes = read_prefix_codes(
        reader,
        header.distance_tree_count(),
        header.distance_alphabet_size,
    )?;
    let command_codes_all_non_single = command_codes
        .iter()
        .all(|code| code.single_symbol().is_none());
    let distance_codes_all_non_single = distance_codes
        .iter()
        .all(|code| code.single_symbol().is_none());
    let single_command_block = header.commands.types() == 1;
    let single_distance_block = header.distances.types() == 1;
    let single_distance_tree = distance_codes.len() == 1;
    let single_literal_block = header.literals.types() == 1;
    let single_literal_code = if single_literal_block && literal_codes.len() == 1 {
        Some(&literal_codes[0])
    } else {
        None
    };
    let no_postfix_distances = header.npostfix == 0 && header.ndirect == 0;
    let mut produced = 0_usize;
    let mut previous = (0_u8, 0_u8);

    while produced < len {
        let command_block_type = if single_command_block {
            0
        } else {
            header.commands.current_type_multi(reader)?
        };
        let command = read_command(
            reader,
            &command_codes[command_block_type],
            command_codes_all_non_single,
        )?;
        if !single_command_block {
            header.commands.consume_one_multi();
        }

        if command.insert_len != 0 {
            let end = produced
                .checked_add(command.insert_len)
                .ok_or(BurliError::Format("Brotli literal run length overflow"))?;
            if end > len {
                return Err(BurliError::Format(
                    "Brotli literal run exceeds meta-block size",
                ));
            }
            validate_literals(
                reader,
                command.insert_len,
                &literal_codes,
                &mut header,
                single_literal_code,
                &mut previous,
            )?;
            produced = end;
        }
        if produced == len {
            break;
        }

        if command.reuse_last_distance {
            // Symbol zero reuses the ring entry and does not update it.
        }
        let distance_block_type = if command.reuse_last_distance || single_distance_block {
            0
        } else {
            let block_type = header.distances.current_type(reader)?;
            header.distances.consume_one_multi();
            block_type
        };
        let distance_symbol = if command.reuse_last_distance {
            0
        } else {
            let tree_index = if single_distance_tree {
                0
            } else {
                header.distance_context_map[distance_block_type * 4 + command.distance_context]
            };
            decode_prefix_symbol(
                reader,
                &distance_codes[tree_index],
                distance_codes_all_non_single,
            )? as usize
        };
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
        let global_produced = output_base
            .checked_add(produced)
            .ok_or(BurliError::Format("Brotli output length overflow"))?;
        let max_allowed_distance = window_size.min(global_produced);
        if distance <= max_allowed_distance {
            if distance == 0 {
                return Err(BurliError::Format("invalid Brotli zero distance"));
            }
            let end = produced
                .checked_add(command.copy_len)
                .ok_or(BurliError::Format("Brotli copy length overflow"))?;
            if distance > global_produced || end > len {
                return Err(BurliError::Format("invalid Brotli backward copy"));
            }
            produced = end;
            if distance_symbol != 0 {
                distances.push(distance);
            }
        } else {
            let copied_len = crate::dictionary::validate_lookup(
                distance,
                max_allowed_distance,
                command.copy_len,
            )?;
            let end = produced
                .checked_add(copied_len)
                .ok_or(BurliError::Format("Brotli dictionary copy length overflow"))?;
            if end > len {
                return Err(BurliError::Format(
                    "Brotli dictionary copy exceeds meta-block size",
                ));
            }
            produced = end;
        }
    }
    Ok(())
}

fn validate_literals(
    reader: &mut BitReader<'_>,
    count: usize,
    literal_codes: &[PrefixCode],
    header: &mut CompressedHeader,
    single_literal_code: Option<&PrefixCode>,
    previous: &mut (u8, u8),
) -> Result<(), DecompressError> {
    let single_block = header.literals.types() == 1;
    if single_block {
        if let Some(code) = single_literal_code {
            if let Some(symbol) = code.single_symbol() {
                let literal = symbol as u8;
                for _ in 0..count {
                    *previous = (literal, previous.0);
                }
                return Ok(());
            }
            let max_bits = usize::from(code.max_bits());
            if max_bits != 0
                && count
                    .checked_mul(max_bits)
                    .is_some_and(|bits| bits <= reader.remaining_bits())
            {
                let mut remaining = count;
                while remaining >= 4 {
                    let first = read_literal_trusted(reader, code);
                    let second = read_literal_trusted(reader, code);
                    let third = read_literal_trusted(reader, code);
                    let fourth = read_literal_trusted(reader, code);
                    *previous = (fourth, third);
                    remaining -= 4;
                    let _ = (first, second);
                }
                while remaining != 0 {
                    let literal = read_literal_trusted(reader, code);
                    *previous = (literal, previous.0);
                    remaining -= 1;
                }
                return Ok(());
            }
        }
    }
    for _ in 0..count {
        let block_type = if single_block {
            0
        } else {
            let value = header.literals.current_type_multi(reader)?;
            header.literals.consume_one_multi();
            value
        };
        let context = literal_context(*previous, header, block_type);
        let tree_index = if single_block && single_literal_code.is_some() {
            0
        } else {
            header.literal_context_map[block_type * 64 + context]
        };
        let code = single_literal_code.unwrap_or(&literal_codes[tree_index]);
        let literal = read_literal(reader, code)?;
        *previous = (literal, previous.0);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn decode_meta_block_body(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    needed: usize,
    output_base: usize,
    window_size: usize,
    header: &mut CompressedHeader,
    literal_codes: &[PrefixCode],
    command_codes: &[PrefixCode],
    distance_codes: &[PrefixCode],
    distances: &mut DistanceRing,
    raw_dictionary: RawDictionary<'_>,
    distance_policy: DistancePolicy,
) -> Result<bool, DecompressError> {
    let single_command_block = header.commands.types() == 1;
    let single_distance_block = header.distances.types() == 1;
    let single_distance_tree = distance_codes.len() == 1;
    let command_codes_all_non_single = command_codes
        .iter()
        .all(|code| code.single_symbol().is_none());
    let distance_codes_all_non_single = distance_codes
        .iter()
        .all(|code| code.single_symbol().is_none());
    let single_literal_block = header.literals.types() == 1;
    let single_literal_code = if single_literal_block && literal_codes.len() == 1 {
        Some(&literal_codes[0])
    } else {
        None
    };
    let single_literal_block_max_bits = if single_literal_block && single_literal_code.is_none() {
        Some(
            literal_codes
                .iter()
                .map(|code| usize::from(code.max_bits()))
                .max()
                .unwrap_or(0),
        )
    } else {
        None
    };
    let literal_shape = LiteralDecodeShape {
        single_block_max_bits: single_literal_block_max_bits,
        uniform_context_mode: uniform_literal_context_mode(header),
    };
    let no_postfix_distances = header.npostfix == 0 && header.ndirect == 0;
    let mut has_copy = false;

    while output.len() < needed {
        let command_block_type = if single_command_block {
            0
        } else {
            header.commands.current_type_multi(reader)?
        };
        let command = read_command(
            reader,
            &command_codes[command_block_type],
            command_codes_all_non_single,
        )?;
        if !single_command_block {
            header.commands.consume_one_multi();
        }
        if command.insert_len != 0 {
            if let Some(literal_code) = single_literal_code {
                copy_literals_single_code_checked(
                    reader,
                    output,
                    needed,
                    command.insert_len,
                    literal_code,
                )?;
            } else {
                copy_literals(
                    reader,
                    output,
                    needed,
                    command.insert_len,
                    literal_codes,
                    header,
                    literal_shape,
                )?;
            }
        }
        if output.len() == needed {
            break;
        }
        if output.len() > needed {
            return Err(BurliError::Format("Brotli command exceeds meta-block size"));
        }

        has_copy = true;
        if distance_policy == DistancePolicy::LocalOnly && command.reuse_last_distance {
            return Err(BurliError::Format(
                "concat fragment uses distance-ring state",
            ));
        }

        let distance_symbol = if command.reuse_last_distance {
            0
        } else {
            let distance_block_type = if single_distance_block {
                0
            } else {
                header.distances.current_type(reader)?
            };
            let tree_index = if single_distance_tree {
                0
            } else {
                header.distance_context_map[distance_block_type * 4 + command.distance_context]
            };
            if !single_distance_block {
                header.distances.consume_one();
            }
            decode_prefix_symbol(
                reader,
                &distance_codes[tree_index],
                distance_codes_all_non_single,
            )? as usize
        };
        if distance_policy == DistancePolicy::LocalOnly && distance_symbol < 16 {
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
        copy_from_distance(
            output,
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
            distance_policy,
        )?;
    }

    Ok(has_copy)
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
    single_block_max_bits: Option<usize>,
    uniform_context_mode: Option<u8>,
}

fn copy_from_distance(
    output: &mut Vec<u8>,
    request: CopyRequest,
    distances: &mut DistanceRing,
    raw_dictionary: RawDictionary<'_>,
    distance_policy: DistancePolicy,
) -> Result<(), DecompressError> {
    let produced = output.len();
    let global_produced = request
        .output_base
        .checked_add(produced)
        .ok_or(BurliError::Format("Brotli output length overflow"))?;
    let max_allowed_distance = request.window_size.min(global_produced);
    if distance_policy == DistancePolicy::LocalOnly
        && (request.distance == 0 || request.distance > produced)
    {
        return Err(BurliError::Format(
            "concat fragment uses non-local backward distance",
        ));
    }
    let mut static_dictionary_distance_base = max_allowed_distance;
    if !raw_dictionary.is_empty() {
        let raw_dictionary_distance_end = max_allowed_distance
            .checked_add(raw_dictionary.len())
            .ok_or(BurliError::Format(
                "Brotli raw dictionary distance overflow",
            ))?;
        if request.distance > max_allowed_distance
            && request.distance <= raw_dictionary_distance_end
        {
            crate::dictionary::append_raw_lz77_copy(
                output,
                raw_dictionary,
                request.distance,
                max_allowed_distance,
                request.len,
                request.needed,
            )?;
            if request.push_distance {
                distances.push(request.distance);
            }
            return Ok(());
        }
        static_dictionary_distance_base = raw_dictionary_distance_end;
    }
    if request.distance > static_dictionary_distance_base {
        crate::dictionary::append_lookup(
            output,
            request.distance,
            static_dictionary_distance_base,
            request.len,
            request.needed,
        )?;
        return Ok(());
    }
    if request.distance == 0 || request.distance > produced {
        return Err(BurliError::Format("invalid Brotli backward distance"));
    }

    checked_backward_copy_end(produced, request.needed, request.len)?;

    if request.distance == 1 {
        let byte = output[produced - 1];
        output.resize(produced + request.len, byte);
    } else if request.distance < CHUNKED_COPY_MIN_DISTANCE && request.len >= 8 {
        copy_repeated_pattern(output, request.distance, request.len);
    } else if request.distance < CHUNKED_COPY_MIN_DISTANCE {
        for _ in 0..request.len {
            let src = output.len() - request.distance;
            let byte = output[src];
            output.push(byte);
        }
    } else if request.distance >= request.len {
        let src = produced - request.distance;
        #[cfg(not(feature = "paranoid"))]
        unsafe {
            append_non_overlapping_backward_copy(output, src, request.len);
        }
        #[cfg(feature = "paranoid")]
        append_non_overlapping_backward_copy(output, src, request.len);
    } else if request.distance >= 8 {
        #[cfg(not(feature = "paranoid"))]
        unsafe {
            append_overlapping_backward_copy(output, request.distance, request.len);
        }
        #[cfg(feature = "paranoid")]
        append_overlapping_backward_copy_safe(output, request.distance, request.len);
    } else {
        let mut remaining = request.len;
        while remaining != 0 {
            let src = output.len() - request.distance;
            let chunk = request.distance.min(remaining);
            output.extend_from_within(src..src + chunk);
            remaining -= chunk;
        }
    }
    if request.push_distance {
        distances.push(request.distance);
    }
    Ok(())
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn append_overlapping_backward_copy(output: &mut Vec<u8>, distance: usize, len: usize) {
    // SAFETY: callers prove `8 <= distance < len`, `distance <= output.len()`,
    // and enough output capacity for `len` bytes.
    debug_assert!(distance >= 8 && distance < len);
    output.reserve(len);
    debug_assert!(overlapping_backward_copy_contract(
        output.len(),
        output.capacity(),
        distance,
        len,
    ));

    let mut remaining = len;
    while remaining != 0 {
        let old_len = output.len();
        let chunk = distance.min(remaining);
        // SAFETY: `chunk <= distance`, so source and destination do not
        // overlap. The caller validated the source distance, and reserve made
        // the destination range writable.
        unsafe {
            let ptr = output.as_mut_ptr();
            core::ptr::copy_nonoverlapping(ptr.add(old_len - distance), ptr.add(old_len), chunk);
            output.set_len(old_len + chunk);
        }
        remaining -= chunk;
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn overlapping_backward_copy_contract(
    old_len: usize,
    capacity: usize,
    distance: usize,
    len: usize,
) -> bool {
    distance >= 8
        && distance < len
        && distance <= old_len
        && capacity >= old_len
        && len <= capacity - old_len
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn append_overlapping_backward_copy_safe(output: &mut Vec<u8>, distance: usize, len: usize) {
    let mut remaining = len;
    while remaining != 0 {
        let src = output.len() - distance;
        let chunk = distance.min(remaining);
        output.extend_from_within(src..src + chunk);
        remaining -= chunk;
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn append_non_overlapping_backward_copy(output: &mut Vec<u8>, src: usize, len: usize) {
    // SAFETY: callers prove that `src..src + len` is initialized and that the
    // destination has capacity for `len` bytes without overlapping the source.
    debug_assert!(src <= output.len());
    debug_assert!(len <= output.len() - src);
    if output.capacity() - output.len() < len {
        output.reserve(len);
    }
    debug_assert!(non_overlapping_backward_copy_contract(
        output.len(),
        output.capacity(),
        src,
        len,
    ));

    // SAFETY: callers use this only when `distance >= len`, so the source range
    // ends at or before the old vector end and cannot overlap the destination.
    // The meta-block decoder reserves output capacity up front. This initializes
    // `old_len..old_len + len` exactly once before advancing the vector length.
    let old_len = output.len();
    unsafe {
        let ptr = output.as_mut_ptr();
        core::ptr::copy_nonoverlapping(ptr.add(src), ptr.add(old_len), len);
        output.set_len(old_len + len);
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn non_overlapping_backward_copy_contract(
    old_len: usize,
    capacity: usize,
    src: usize,
    len: usize,
) -> bool {
    src <= old_len && capacity >= old_len && len <= old_len - src && len <= capacity - old_len
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn append_non_overlapping_backward_copy(output: &mut Vec<u8>, src: usize, len: usize) {
    output.extend_from_within(src..src + len);
}

fn copy_repeated_pattern(output: &mut Vec<u8>, distance: usize, len: usize) {
    let src = output.len() - distance;
    let mut pattern = [0_u8; CHUNKED_COPY_MIN_DISTANCE];
    pattern[..distance].copy_from_slice(&output[src..src + distance]);

    let mut remaining = len;
    while remaining >= distance {
        output.extend_from_slice(&pattern[..distance]);
        remaining -= distance;
    }
    if remaining != 0 {
        output.extend_from_slice(&pattern[..remaining]);
    }
}

fn checked_backward_copy_end(
    produced: usize,
    needed: usize,
    len: usize,
) -> Result<usize, DecompressError> {
    if produced > needed || len > needed - produced {
        return Err(BurliError::Format("Brotli copy exceeds meta-block size"));
    }
    let end = produced + len;
    Ok(end)
}

#[derive(Clone, Debug)]
struct CompressedHeader {
    literals: BlockCategory,
    commands: BlockCategory,
    distances: BlockCategory,
    npostfix: u8,
    ndirect: usize,
    context_modes: Vec<u8>,
    literal_context_map: Vec<usize>,
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

    #[inline(always)]
    fn current_type_multi(&mut self, reader: &mut BitReader<'_>) -> Result<usize, DecompressError> {
        debug_assert!(self.block_types != 1);
        if self.remaining == 0 {
            self.switch(reader)?;
        }
        Ok(self.current_type)
    }

    #[inline(always)]
    fn current_type(&mut self, reader: &mut BitReader<'_>) -> Result<usize, DecompressError> {
        if self.block_types == 1 {
            return Ok(0);
        }
        self.current_type_multi(reader)
    }

    #[inline(always)]
    fn consume_one_multi(&mut self) {
        debug_assert!(self.block_types != 1);
        self.remaining -= 1;
    }

    #[inline(always)]
    fn consume_one(&mut self) {
        if self.block_types != 1 {
            self.consume_one_multi();
        }
    }

    #[cold]
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

impl CompressedHeader {
    fn literal_tree_count(&self) -> usize {
        self.literal_context_map.iter().copied().max().unwrap_or(0) + 1
    }

    fn distance_tree_count(&self) -> usize {
        self.distance_context_map.iter().copied().max().unwrap_or(0) + 1
    }
}

fn uniform_literal_context_mode(header: &CompressedHeader) -> Option<u8> {
    let (&first, rest) = header.context_modes.split_first()?;
    rest.iter().all(|&mode| mode == first).then_some(first)
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
        literal_context_map,
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

fn read_prefix_codes(
    reader: &mut BitReader<'_>,
    count: usize,
    alphabet_size: usize,
) -> Result<Vec<PrefixCode>, DecompressError> {
    let mut codes = Vec::with_capacity(count);
    for _ in 0..count {
        codes.push(PrefixCode::read(reader, alphabet_size)?);
    }
    Ok(codes)
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

fn read_command(
    reader: &mut BitReader<'_>,
    command_code: &PrefixCode,
    known_non_single: bool,
) -> Result<Command, DecompressError> {
    let code = usize::from(decode_prefix_symbol(reader, command_code, known_non_single)? & 0x0fff);
    debug_assert!(code < COMMAND_ALPHABET_SIZE);
    let prefix = COMMAND_PREFIXES[code];
    let insert_extra_bits = prefix.insert_extra_bits();
    let insert_extra = if insert_extra_bits == 0 {
        0
    } else {
        reader.read_bits(insert_extra_bits)? as usize
    };
    let copy_extra_bits = prefix.copy_extra_bits();
    let copy_extra = if copy_extra_bits == 0 {
        0
    } else {
        reader.read_bits(copy_extra_bits)? as usize
    };
    let insert_len = prefix.insert_base() + insert_extra;
    let copy_len = prefix.copy_base() + copy_extra;

    Ok(Command {
        insert_len,
        copy_len,
        reuse_last_distance: prefix.reuse_last_distance(),
        distance_context: prefix.distance_context(),
    })
}

#[inline(always)]
fn decode_prefix_symbol(
    reader: &mut BitReader<'_>,
    code: &PrefixCode,
    known_non_single: bool,
) -> Result<u16, DecompressError> {
    if known_non_single {
        code.decode_non_single(reader)
    } else {
        code.decode(reader)
    }
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

fn copy_literals(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    needed: usize,
    count: usize,
    literal_codes: &[PrefixCode],
    header: &mut CompressedHeader,
    shape: LiteralDecodeShape,
) -> Result<(), DecompressError> {
    let produced = output.len();
    if produced > needed || count > needed - produced {
        return Err(BurliError::Format(
            "Brotli literal run exceeds meta-block size",
        ));
    }

    if header.literals.types() == 1 {
        if literal_codes.len() == 1 {
            return copy_literals_single_code(reader, output, count, &literal_codes[0]);
        }
        return copy_literals_single_block(
            reader,
            output,
            count,
            literal_codes,
            &header.literal_context_map[..64],
            header.context_modes[0],
            shape.single_block_max_bits.unwrap_or(0),
        );
    }

    if let Some(mode) = shape.uniform_context_mode {
        return copy_literals_multi_block_uniform_mode(
            reader,
            output,
            count,
            literal_codes,
            header,
            mode,
        );
    }

    let mut previous = previous_literal_bytes(output);
    for _ in 0..count {
        let literal_block_type = header.literals.current_type_multi(reader)?;
        let context = literal_context(previous, header, literal_block_type);
        let tree_index = header.literal_context_map[literal_block_type * 64 + context];
        let literal = read_literal(reader, &literal_codes[tree_index])?;
        header.literals.consume_one_multi();
        output.push(literal);
        previous = (literal, previous.0);
    }
    Ok(())
}

fn copy_literals_multi_block_uniform_mode(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    count: usize,
    literal_codes: &[PrefixCode],
    header: &mut CompressedHeader,
    mode: u8,
) -> Result<(), DecompressError> {
    let mut previous = previous_literal_bytes(output);
    match mode {
        0 => {
            for _ in 0..count {
                let literal_block_type = header.literals.current_type_multi(reader)?;
                let context = usize::from(previous.0 & 0x3f);
                let tree_index = header.literal_context_map[literal_block_type * 64 + context];
                let literal = read_literal(reader, &literal_codes[tree_index])?;
                header.literals.consume_one_multi();
                output.push(literal);
                previous = (literal, previous.0);
            }
        }
        1 => {
            for _ in 0..count {
                let literal_block_type = header.literals.current_type_multi(reader)?;
                let context = usize::from(previous.0 >> 2);
                let tree_index = header.literal_context_map[literal_block_type * 64 + context];
                let literal = read_literal(reader, &literal_codes[tree_index])?;
                header.literals.consume_one_multi();
                output.push(literal);
                previous = (literal, previous.0);
            }
        }
        2 => {
            for _ in 0..count {
                let literal_block_type = header.literals.current_type_multi(reader)?;
                let context = crate::context_lookup::CONTEXT_PAIR_LOOKUP[0]
                    [(usize::from(previous.0) << 8) | usize::from(previous.1)];
                let tree_index =
                    header.literal_context_map[literal_block_type * 64 + usize::from(context)];
                let literal = read_literal(reader, &literal_codes[tree_index])?;
                header.literals.consume_one_multi();
                output.push(literal);
                previous = (literal, previous.0);
            }
        }
        3 => {
            for _ in 0..count {
                let literal_block_type = header.literals.current_type_multi(reader)?;
                let context = crate::context_lookup::CONTEXT_PAIR_LOOKUP[1]
                    [(usize::from(previous.0) << 8) | usize::from(previous.1)];
                let tree_index =
                    header.literal_context_map[literal_block_type * 64 + usize::from(context)];
                let literal = read_literal(reader, &literal_codes[tree_index])?;
                header.literals.consume_one_multi();
                output.push(literal);
                previous = (literal, previous.0);
            }
        }
        _ => return Err(BurliError::Format("invalid literal context mode")),
    }
    Ok(())
}

fn copy_literals_single_code(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    count: usize,
    code: &PrefixCode,
) -> Result<(), DecompressError> {
    if let Some(symbol) = code.single_symbol() {
        output.resize(output.len() + count, symbol as u8);
        return Ok(());
    }
    let max_bits = usize::from(code.max_bits());
    if max_bits != 0
        && count
            .checked_mul(max_bits)
            .is_some_and(|bits| bits <= reader.remaining_bits())
    {
        #[cfg(not(feature = "paranoid"))]
        unsafe {
            copy_literals_single_code_trusted_fast(reader, output, count, code);
        }
        #[cfg(feature = "paranoid")]
        copy_literals_single_code_trusted_fast(reader, output, count, code);
        return Ok(());
    }
    for _ in 0..count {
        output.push(code.decode_non_single(reader)? as u8);
    }
    Ok(())
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
unsafe fn copy_literals_single_code_trusted_fast(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    count: usize,
    code: &PrefixCode,
) {
    output.reserve(count);
    debug_assert!(literal_bulk_write_contract(
        output.len(),
        output.capacity(),
        count,
    ));

    let old_len = output.len();
    // SAFETY: `reserve(count)` makes `old_len..old_len + count` writable.
    // The trusted-fast decode path has already proven enough input bits for all
    // literals, so this loop cannot return early and initializes each slot once.
    unsafe {
        let ptr = output.as_mut_ptr().add(old_len);
        let mut index = 0;
        while index + 4 <= count {
            ptr.add(index)
                .write(code.decode_non_single_trusted_fast(reader) as u8);
            ptr.add(index + 1)
                .write(code.decode_non_single_trusted_fast(reader) as u8);
            ptr.add(index + 2)
                .write(code.decode_non_single_trusted_fast(reader) as u8);
            ptr.add(index + 3)
                .write(code.decode_non_single_trusted_fast(reader) as u8);
            index += 4;
        }
        while index < count {
            ptr.add(index)
                .write(code.decode_non_single_trusted_fast(reader) as u8);
            index += 1;
        }
        output.set_len(old_len + count);
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn copy_literals_single_code_trusted_fast(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    count: usize,
    code: &PrefixCode,
) {
    for _ in 0..count {
        output.push(code.decode_non_single_trusted_fast(reader) as u8);
    }
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn literal_bulk_write_contract(old_len: usize, capacity: usize, count: usize) -> bool {
    old_len <= capacity && count <= capacity - old_len
}

fn copy_literals_single_code_checked(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    needed: usize,
    count: usize,
    code: &PrefixCode,
) -> Result<(), DecompressError> {
    let produced = output.len();
    if produced > needed || count > needed - produced {
        return Err(BurliError::Format(
            "Brotli literal run exceeds meta-block size",
        ));
    }
    copy_literals_single_code(reader, output, count, code)
}

fn copy_literals_single_block(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    count: usize,
    literal_codes: &[PrefixCode],
    context_map: &[usize],
    mode: u8,
    max_bits: usize,
) -> Result<(), DecompressError> {
    if count
        .checked_mul(max_bits)
        .is_some_and(|bits| bits <= reader.remaining_bits())
    {
        copy_literals_single_block_trusted(reader, output, count, literal_codes, context_map, mode);
        return Ok(());
    }

    let mut previous = previous_literal_bytes(output);
    match mode {
        0 => {
            for _ in 0..count {
                let tree_index = context_map[usize::from(previous.0 & 0x3f)];
                let literal = read_literal(reader, &literal_codes[tree_index])?;
                output.push(literal);
                previous = (literal, previous.0);
            }
        }
        1 => {
            for _ in 0..count {
                let tree_index = context_map[usize::from(previous.0 >> 2)];
                let literal = read_literal(reader, &literal_codes[tree_index])?;
                output.push(literal);
                previous = (literal, previous.0);
            }
        }
        2 => {
            for _ in 0..count {
                let context = crate::context_lookup::CONTEXT_PAIR_LOOKUP[0]
                    [(usize::from(previous.0) << 8) | usize::from(previous.1)];
                let tree_index = context_map[usize::from(context)];
                let literal = read_literal(reader, &literal_codes[tree_index])?;
                output.push(literal);
                previous = (literal, previous.0);
            }
        }
        3 => {
            for _ in 0..count {
                let context = crate::context_lookup::CONTEXT_PAIR_LOOKUP[1]
                    [(usize::from(previous.0) << 8) | usize::from(previous.1)];
                let tree_index = context_map[usize::from(context)];
                let literal = read_literal(reader, &literal_codes[tree_index])?;
                output.push(literal);
                previous = (literal, previous.0);
            }
        }
        _ => return Err(BurliError::Format("invalid literal context mode")),
    }
    Ok(())
}

fn copy_literals_single_block_trusted(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    count: usize,
    literal_codes: &[PrefixCode],
    context_map: &[usize],
    mode: u8,
) {
    let mut previous = previous_literal_bytes(output);
    match mode {
        0 => {
            for _ in 0..count {
                let tree_index = context_map[usize::from(previous.0 & 0x3f)];
                let literal = read_literal_trusted(reader, &literal_codes[tree_index]);
                output.push(literal);
                previous = (literal, previous.0);
            }
        }
        1 => {
            for _ in 0..count {
                let tree_index = context_map[usize::from(previous.0 >> 2)];
                let literal = read_literal_trusted(reader, &literal_codes[tree_index]);
                output.push(literal);
                previous = (literal, previous.0);
            }
        }
        2 => {
            for _ in 0..count {
                let context = crate::context_lookup::CONTEXT_PAIR_LOOKUP[0]
                    [(usize::from(previous.0) << 8) | usize::from(previous.1)];
                let tree_index = context_map[usize::from(context)];
                let literal = read_literal_trusted(reader, &literal_codes[tree_index]);
                output.push(literal);
                previous = (literal, previous.0);
            }
        }
        3 => {
            for _ in 0..count {
                let context = crate::context_lookup::CONTEXT_PAIR_LOOKUP[1]
                    [(usize::from(previous.0) << 8) | usize::from(previous.1)];
                let tree_index = context_map[usize::from(context)];
                let literal = read_literal_trusted(reader, &literal_codes[tree_index]);
                output.push(literal);
                previous = (literal, previous.0);
            }
        }
        _ => unreachable!(),
    }
}

#[inline(always)]
fn read_literal(reader: &mut BitReader<'_>, code: &PrefixCode) -> Result<u8, DecompressError> {
    Ok(code.decode(reader)? as u8)
}

#[inline(always)]
fn read_literal_trusted(reader: &mut BitReader<'_>, code: &PrefixCode) -> u8 {
    if let Some(symbol) = code.single_symbol() {
        symbol as u8
    } else {
        code.decode_non_single_trusted_fast(reader) as u8
    }
}

#[inline(always)]
fn literal_context(previous: (u8, u8), header: &CompressedHeader, block_type: usize) -> usize {
    literal_context_for_mode(previous, header.context_modes[block_type])
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
        2 => {
            crate::context_lookup::CONTEXT_PAIR_LOOKUP[0][(usize::from(p1) << 8) | usize::from(p2)]
        }
        _ => {
            debug_assert_eq!(mode, 3);
            crate::context_lookup::CONTEXT_PAIR_LOOKUP[1][(usize::from(p1) << 8) | usize::from(p2)]
        }
    }
    .into()
}

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
    let dextra = reader.read_bits(ndistbits as u8)? as usize;
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
    let dextra = reader.read_bits(ndistbits as u8)? as usize;
    let offset = ((2 + (adjusted & 1)) << ndistbits) - 4;
    Ok(offset + dextra + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use burli_core::bits::BitWriter;

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

    #[test]
    fn distance_symbol_zero_does_not_update_ring() {
        let mut output = b"0123456789abcdef".to_vec();
        let mut distances = DistanceRing::new();

        copy_from_distance(
            &mut output,
            CopyRequest {
                needed: 20,
                window_size: 1 << 16,
                output_base: 0,
                distance: 4,
                len: 4,
                push_distance: false,
            },
            &mut distances,
            RawDictionary::empty(),
            DistancePolicy::Standard,
        )
        .unwrap();

        let mut reader = BitReader::new(&[]);
        assert_eq!(read_distance(&mut reader, 1, 0, 0, &distances).unwrap(), 11);
    }

    #[test]
    fn distance_one_copy_repeats_last_byte() {
        let mut output = b"aaaaab".to_vec();
        let mut distances = DistanceRing::new();

        copy_from_distance(
            &mut output,
            CopyRequest {
                needed: 10,
                window_size: 1 << 16,
                output_base: 0,
                distance: 1,
                len: 4,
                push_distance: true,
            },
            &mut distances,
            RawDictionary::empty(),
            DistancePolicy::Standard,
        )
        .unwrap();

        assert_eq!(output, b"aaaaabbbbb");
        let mut reader = BitReader::new(&[]);
        assert_eq!(read_distance(&mut reader, 0, 0, 0, &distances).unwrap(), 1);
    }

    #[test]
    fn small_distance_copy_repeats_source_pattern() {
        let mut output = b"abcdef".to_vec();
        let mut distances = DistanceRing::new();

        copy_from_distance(
            &mut output,
            CopyRequest {
                needed: 14,
                window_size: 1 << 16,
                output_base: 0,
                distance: 3,
                len: 8,
                push_distance: true,
            },
            &mut distances,
            RawDictionary::empty(),
            DistancePolicy::Standard,
        )
        .unwrap();

        assert_eq!(output, b"abcdefdefdefde");
        let mut reader = BitReader::new(&[]);
        assert_eq!(read_distance(&mut reader, 0, 0, 0, &distances).unwrap(), 3);
    }

    #[test]
    fn backward_copy_handles_non_overlapping_range() {
        let mut output = b"abcdefghijklmnop".to_vec();
        let mut distances = DistanceRing::new();

        copy_from_distance(
            &mut output,
            CopyRequest {
                needed: 20,
                window_size: 1 << 16,
                output_base: 0,
                distance: 16,
                len: 4,
                push_distance: true,
            },
            &mut distances,
            RawDictionary::empty(),
            DistancePolicy::Standard,
        )
        .unwrap();

        assert_eq!(output, b"abcdefghijklmnopabcd");
        let mut reader = BitReader::new(&[]);
        assert_eq!(read_distance(&mut reader, 0, 0, 0, &distances).unwrap(), 16);
    }

    #[test]
    fn non_overlapping_backward_copy_matches_safe_copy() {
        let mut output = b"abcdefghijklmnop".to_vec();
        let mut expected = output.clone();

        output.reserve(8);
        #[cfg(not(feature = "paranoid"))]
        unsafe {
            append_non_overlapping_backward_copy(&mut output, 4, 8);
        }
        #[cfg(feature = "paranoid")]
        append_non_overlapping_backward_copy(&mut output, 4, 8);
        expected.extend_from_within(4..12);

        assert_eq!(output, expected);
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
        let mut output = b"prefix".to_vec();

        #[cfg(not(feature = "paranoid"))]
        unsafe {
            copy_literals_single_code_trusted_fast(&mut reader, &mut output, 6, &code);
        }
        #[cfg(feature = "paranoid")]
        copy_literals_single_code_trusted_fast(&mut reader, &mut output, 6, &code);

        assert_eq!(output, b"prefix\0\x01\0\x01\x01\0");
    }

    #[test]
    fn backward_copy_doubles_large_overlapping_range() {
        let mut output = b"abcdefghijklmnop".to_vec();
        let mut distances = DistanceRing::new();

        copy_from_distance(
            &mut output,
            CopyRequest {
                needed: 56,
                window_size: 1 << 16,
                output_base: 0,
                distance: 16,
                len: 40,
                push_distance: true,
            },
            &mut distances,
            RawDictionary::empty(),
            DistancePolicy::Standard,
        )
        .unwrap();

        assert_eq!(
            output,
            b"abcdefghijklmnopabcdefghijklmnopabcdefghijklmnopabcdefgh"
        );
        let mut reader = BitReader::new(&[]);
        assert_eq!(read_distance(&mut reader, 0, 0, 0, &distances).unwrap(), 16);
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

    #[kani::proof]
    fn backward_copy_bound_check_caps_end_at_needed() {
        let produced = kani::any::<u8>();
        let distance = kani::any::<u8>();
        let len = kani::any::<u8>();
        kani::assume((1..=8).contains(&produced));
        kani::assume((1..=produced).contains(&distance));
        kani::assume(len <= 8);

        let produced = usize::from(produced);
        let request = CopyRequest {
            needed: produced + usize::from(len),
            window_size: 16,
            output_base: 0,
            distance: usize::from(distance),
            len: usize::from(len),
            push_distance: true,
        };

        let end = match checked_backward_copy_end(produced, request.needed, request.len) {
            Ok(end) => end,
            Err(_) => usize::MAX,
        };

        assert_eq!(end, request.needed);
        assert!(end <= request.needed);
    }

    #[kani::proof]
    fn non_overlapping_backward_copy_branch_satisfies_unsafe_contract() {
        let produced = kani::any::<u8>();
        let needed = kani::any::<u8>();
        let capacity = kani::any::<u8>();
        let distance = kani::any::<u8>();
        let len = kani::any::<u8>();
        let produced = usize::from(produced);
        let needed = usize::from(needed);
        let capacity = usize::from(capacity);
        let distance = usize::from(distance);
        let len = usize::from(len);
        kani::assume(produced <= needed);
        kani::assume(needed <= capacity);
        kani::assume(distance != 0);
        kani::assume(distance <= produced);
        kani::assume(len <= needed - produced);
        kani::assume(distance >= len);

        let src = produced - distance;

        assert!(non_overlapping_backward_copy_contract(
            produced, capacity, src, len,
        ));
    }

    #[kani::proof]
    fn overlapping_backward_copy_branch_satisfies_unsafe_contract() {
        let old_len = usize::from(kani::any::<u8>());
        let capacity = usize::from(kani::any::<u8>());
        let distance = usize::from(kani::any::<u8>());
        let len = usize::from(kani::any::<u8>());
        kani::assume(distance >= 8);
        kani::assume(distance <= old_len);
        kani::assume(distance < len);
        kani::assume(old_len <= capacity);
        kani::assume(len <= capacity - old_len);

        assert!(overlapping_backward_copy_contract(
            old_len, capacity, distance, len,
        ));
    }

    #[kani::proof]
    fn literal_bulk_write_branch_satisfies_unsafe_contract() {
        let old_len = usize::from(kani::any::<u8>());
        let capacity = usize::from(kani::any::<u8>());
        let count = usize::from(kani::any::<u8>());
        kani::assume(old_len <= capacity);
        kani::assume(count <= capacity - old_len);

        assert!(literal_bulk_write_contract(old_len, capacity, count));
        assert!(old_len + count <= capacity);
    }
}
