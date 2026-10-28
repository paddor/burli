//! Compressed meta-blocks decoded in steps, for decoders that get their
//! input in pieces.
//!
//! A step reads from the input buffered so far. A step that runs out of
//! input fails with the end-of-input error and leaves the state of its last
//! checkpoint, so the next step goes on from there with more input. Only the
//! part that ran out decodes again: the header, one prefix code, or in the
//! command loop one command header, literal, distance, or block switch.
//!
//! The command loop runs like the one-shot loop while more than
//! [`STEP_BITS`] input bits remain. Below that, it takes a checkpoint
//! before each command header, distance, block switch, and literal that
//! may run out. It also takes one when it stops for output room, and
//! within literal runs that run past the input.

use alloc::boxed::Box;
use alloc::vec::Vec;

use burli_core::{BurliError, DecompressError, bits::BitReader};

use super::{
    COMMAND_ALPHABET_SIZE, CodeGroup, Command, CompressedHeader, CopyRequest, DistanceRing,
    LITERAL_ALPHABET_SIZE, LiteralBlock, LiteralDecodeShape, MetaBlockBody, OUTPUT_SLACK,
    WIDE_ROOT_MIN_LEN, copy_from_distance, copy_literal_run, copy_literals_single_code,
    decode_prefix_symbol, distance_trees, grow_room, read_command, read_distance,
    read_distance_no_postfix_with_ring, read_header,
};
use crate::dictionary::RawDictionary;
use crate::huffman::{ROOT_SIZE, WIDE_ROOT_SIZE};

/// More input bits than one command header, distance, or block switch
/// reads. A block switch reads at most 54 bits, and a command header 63
/// more.
const STEP_BITS: usize = 128;

/// The error of a read past the input end.
pub(crate) const END_OF_INPUT: DecompressError =
    BurliError::Format("unexpected end of Brotli input");

pub(crate) fn is_end_of_input(error: &DecompressError) -> bool {
    *error == END_OF_INPUT
}

/// How a step ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Step {
    /// The step read part of the header.
    More,
    /// The command loop stopped at the requested output position.
    Stopped,
    /// The meta-block is decoded.
    Done,
}

/// A compressed meta-block being decoded.
pub(crate) struct MetaBlock {
    len: usize,
    is_last: bool,
    start: usize,
    needed: usize,
    room: usize,
    output_base: usize,
    window_size: usize,
    stage: Stage,
}

enum Stage {
    Header,
    Codes(Box<Codes>),
}

/// The header and prefix codes, and the command loop state.
struct Codes {
    header: CompressedHeader,
    /// Literal, command, and distance codes.
    groups: [CodeGroup; 3],
    /// The next code to read, as group and code index, until all are read.
    next: Option<(usize, usize)>,
    cursor: Cursor,
}

/// Command loop state at a checkpoint.
#[derive(Clone, Copy, Debug)]
struct Cursor {
    pos: usize,
    command_remaining: usize,
    literal_remaining: usize,
    distance_remaining: usize,
    /// Current and previous block types of literals, commands, and
    /// distances.
    block_types: [(usize, usize); 3],
    distances: DistanceRing,
    pending: Pending,
}

/// The part of a command still to decode.
#[derive(Clone, Copy, Debug)]
enum Pending {
    /// The next command header.
    Header,
    /// `left` literals of `command`.
    Literals { command: Command, left: usize },
    /// The distance and copy of `command`.
    Distance { command: Command },
}

impl Pending {
    /// Output bytes the rest of the command writes.
    const fn len(self) -> usize {
        match self {
            Self::Header => 0,
            Self::Literals { command, left } => left + command.copy_len,
            Self::Distance { command } => command.copy_len,
        }
    }
}

impl MetaBlock {
    /// Starts a meta-block of `len` bytes after the output held so far.
    pub(crate) fn new(
        len: usize,
        is_last: bool,
        output: &mut Vec<u8>,
        output_base: usize,
        window_bits: u8,
        max_output_size: usize,
    ) -> Result<Self, DecompressError> {
        let start = output.len();
        let needed = start
            .checked_add(len)
            .ok_or(BurliError::Format("Brotli output length overflow"))?;
        let global_needed = output_base
            .checked_add(needed)
            .ok_or(BurliError::Format("Brotli output length overflow"))?;
        if global_needed > max_output_size {
            return Err(BurliError::OutputLimitExceeded {
                limit: max_output_size,
                needed: global_needed,
            });
        }
        let room = needed + OUTPUT_SLACK;
        if output.capacity() < room {
            if is_last {
                output.reserve_exact(room - start);
            } else {
                output.reserve(room - start);
            }
        }
        Ok(Self {
            len,
            is_last,
            start,
            needed,
            room,
            output_base,
            window_size: (1_usize << window_bits) - 16,
            stage: Stage::Header,
        })
    }

    pub(crate) const fn is_last(&self) -> bool {
        self.is_last
    }

    /// End of the output decoded so far. After a failed step that did not
    /// run out of input, it is where decoding failed.
    pub(crate) fn decoded_end(&self) -> usize {
        match &self.stage {
            Stage::Header => self.start,
            Stage::Codes(codes) => codes.cursor.pos,
        }
    }

    /// Decodes from `encoded` at `bit_pos`, and moves `bit_pos` to the last
    /// checkpoint. The command loop stops at the first command that starts
    /// at or past `stop_at`. After [`Step::Done`], `output` ends at the
    /// meta-block end and `distances` holds the ring after it.
    pub(crate) fn step(
        &mut self,
        encoded: &[u8],
        bit_pos: &mut usize,
        output: &mut Vec<u8>,
        distances: &mut DistanceRing,
        raw_dictionary: RawDictionary<'_>,
        stop_at: usize,
    ) -> Result<Step, DecompressError> {
        let codes = match &mut self.stage {
            Stage::Header => {
                let mut reader = BitReader::with_bit_pos(encoded, *bit_pos)?;
                let header = read_header(&mut reader)?;
                *bit_pos = reader.consumed_bits();
                let groups = [0, 1, 2].map(|group| group_shape(&header, group, self.len));
                let groups = [
                    CodeGroup::new(groups[0].0, groups[0].1, groups[0].2)?,
                    CodeGroup::new(groups[1].0, groups[1].1, groups[1].2)?,
                    CodeGroup::new(groups[2].0, groups[2].1, groups[2].2)?,
                ];
                let cursor = Cursor {
                    pos: self.start,
                    command_remaining: header.commands.remaining,
                    literal_remaining: header.literals.remaining,
                    distance_remaining: header.distances.remaining,
                    block_types: block_types(&header),
                    distances: *distances,
                    pending: Pending::Header,
                };
                self.stage = Stage::Codes(Box::new(Codes {
                    header,
                    groups,
                    next: Some((0, 0)),
                    cursor,
                }));
                return Ok(Step::More);
            }
            Stage::Codes(codes) => codes,
        };

        if let Some((group, index)) = codes.next {
            let (count, alphabet_size, _) = group_shape(&codes.header, group, self.len);
            let mut reader = BitReader::with_bit_pos(encoded, *bit_pos)?;
            codes.groups[group].read_code(&mut reader, index, alphabet_size)?;
            *bit_pos = reader.consumed_bits();
            codes.next = if index + 1 < count {
                Some((group, index + 1))
            } else if group < 2 {
                Some((group + 1, 0))
            } else {
                None
            };
            return Ok(Step::More);
        }

        let Codes {
            header,
            groups: [literal_codes, command_codes, distance_codes],
            cursor,
            ..
        } = &mut **codes;
        let body: MetaBlockBody<'_> = MetaBlockBody {
            start: self.start,
            needed: self.needed,
            room: self.room,
            output_base: self.output_base,
            window_size: self.window_size,
            literal_codes,
            command_codes,
            distance_codes,
            raw_dictionary,
            single_literal_code: None,
        };
        let single = header.literals.types() == 1 && literal_codes.len() == 1;
        let narrow = body.with_single_literal_code(single.then(|| literal_codes.tables().table(0)));
        let result = match literal_codes.wide_table() {
            Some(wide) if single => decode_body::<WIDE_ROOT_SIZE>(
                encoded,
                bit_pos,
                output,
                header,
                cursor,
                body.with_single_literal_code(Some(wide)),
                stop_at,
            ),
            _ => {
                decode_body::<ROOT_SIZE>(encoded, bit_pos, output, header, cursor, narrow, stop_at)
            }
        };
        if result == Ok(Step::Done) {
            output.truncate(self.needed);
            *distances = cursor.distances;
        }
        result
    }
}

/// Count, alphabet size, and wide-root flag of the prefix codes of
/// `group`: literal, command, or distance codes.
fn group_shape(header: &CompressedHeader, group: usize, len: usize) -> (usize, usize, bool) {
    match group {
        0 => (
            header.literal_tree_count,
            LITERAL_ALPHABET_SIZE,
            len >= WIDE_ROOT_MIN_LEN,
        ),
        1 => (header.commands.types(), COMMAND_ALPHABET_SIZE, false),
        _ => (
            header.distance_tree_count,
            header.distance_alphabet_size,
            false,
        ),
    }
}

fn block_types(header: &CompressedHeader) -> [(usize, usize); 3] {
    [&header.literals, &header.commands, &header.distances]
        .map(|category| (category.current_type, category.previous_type))
}

fn restore_block_types(header: &mut CompressedHeader, block_types: [(usize, usize); 3]) {
    let categories = [
        &mut header.literals,
        &mut header.commands,
        &mut header.distances,
    ];
    for (category, (current, previous)) in categories.into_iter().zip(block_types) {
        category.current_type = current;
        category.previous_type = previous;
    }
}

/// Runs the command loop from `cursor` until the meta-block ends, the
/// output reaches `stop_at` at a command boundary, or the input runs out.
/// Mirrors the one-shot command loop, with its checks in the same order.
/// The reader is a local and never has its address taken outside inlined
/// code, so its fields stay in registers.
#[inline(never)]
fn decode_body<const N: usize>(
    encoded: &[u8],
    bit_pos: &mut usize,
    output: &mut Vec<u8>,
    header: &mut CompressedHeader,
    cursor: &mut Cursor,
    body: MetaBlockBody<'_, N>,
    stop_at: usize,
) -> Result<Step, DecompressError> {
    let MetaBlockBody {
        needed,
        room,
        output_base,
        window_size,
        literal_codes,
        command_codes,
        distance_codes,
        raw_dictionary,
        single_literal_code,
        ..
    } = body;
    let literal_tables = literal_codes.tables();
    let command_tables = command_codes.tables();
    let distance_tables = distance_codes.tables();
    let literal_shape = LiteralDecodeShape {
        max_bits: usize::from(literal_codes.max_bits()),
        single_symbol: literal_codes.single_symbol(0),
    };
    let no_postfix_distances = header.npostfix == 0 && header.ndirect == 0;

    let mut local_reader = BitReader::with_bit_pos(encoded, *bit_pos)?;
    let reader = &mut local_reader;
    let mut c = *cursor;
    restore_block_types(header, c.block_types);
    let mut command_code = command_tables.table(header.commands.current_type);
    let mut distance_codes_by_context =
        distance_trees(header, distance_tables, header.distances.current_type);
    let mut literal_block = LiteralBlock::new(
        &header.literal_context_map,
        &header.context_modes,
        &header.literal_trivial_trees,
        header.literals.current_type,
    );
    let mut out = grow_room(output, c.pos + c.pending.len() + OUTPUT_SLACK, room);

    // Stores the loop state as the checkpoint to go on from, with the rest
    // of the command `pending`.
    macro_rules! checkpoint {
        ($pending:expr) => {{
            c.pending = $pending;
            c.block_types = block_types(header);
            *cursor = c;
            *bit_pos = reader.consumed_bits();
        }};
    }
    // Returns an error. Output up to a failure other than the input end is
    // decoded, so the checkpoint moves there.
    macro_rules! tri {
        ($result:expr) => {
            match $result {
                Ok(value) => value,
                Err(error) => {
                    if !is_end_of_input(&error) {
                        cursor.pos = c.pos;
                    }
                    return Err(error);
                }
            }
        };
    }

    // The command cut short at the checkpoint, then whole commands. The
    // command stays in locals; only checkpoints store it.
    let mut resume = c.pending;
    loop {
        let (command, mut left) = match resume {
            Pending::Header => {
                if c.pos >= needed {
                    break;
                }
                if c.pos >= stop_at {
                    checkpoint!(Pending::Header);
                    return Ok(Step::Stopped);
                }
                if reader.remaining_bits() < STEP_BITS {
                    checkpoint!(Pending::Header);
                }
                if c.command_remaining == 0 {
                    let block_type = tri!(header.commands.next_block(reader));
                    c.command_remaining = header.commands.remaining;
                    command_code = command_tables.table(block_type);
                }
                c.command_remaining -= 1;
                let command = tri!(read_command(reader, command_code));
                let reach = c.pos + command.insert_len + command.copy_len + OUTPUT_SLACK;
                if reach > out.len() {
                    out = grow_room(output, reach, room);
                }
                if command.insert_len > needed - c.pos {
                    tri!(Err::<(), _>(BurliError::Format(
                        "Brotli literal run exceeds meta-block size",
                    )));
                }
                (command, command.insert_len)
            }
            Pending::Literals { command, left } => (command, left),
            Pending::Distance { command } => (command, 0),
        };
        resume = Pending::Header;

        while left != 0 {
            if single_literal_code.is_none() && c.literal_remaining == 0 {
                if reader.remaining_bits() < STEP_BITS {
                    checkpoint!(Pending::Literals { command, left });
                }
                let block_type = tri!(header.literals.next_block(reader));
                c.literal_remaining = header.literals.remaining;
                literal_block = LiteralBlock::new(
                    &header.literal_context_map,
                    &header.context_modes,
                    &header.literal_trivial_trees,
                    block_type,
                );
            }
            let run = if single_literal_code.is_some() {
                left
            } else {
                left.min(c.literal_remaining)
            };
            // Literals whose longest codes the input holds.
            let remaining_bits = reader.remaining_bits();
            let fits = if run * literal_shape.max_bits <= remaining_bits {
                run
            } else {
                remaining_bits / literal_shape.max_bits
            };
            let count = if fits == 0 {
                // One literal that may run out of input.
                checkpoint!(Pending::Literals { command, left });
                1
            } else {
                fits
            };
            if let Some(table) = single_literal_code {
                tri!(copy_literals_single_code(
                    reader,
                    out,
                    &mut c.pos,
                    count,
                    table,
                    literal_shape.single_symbol,
                    literal_shape.max_bits,
                ));
            } else {
                tri!(copy_literal_run(
                    reader,
                    out,
                    &mut c.pos,
                    count,
                    literal_codes,
                    literal_tables,
                    literal_block,
                    literal_shape.max_bits,
                ));
                c.literal_remaining -= count;
            }
            left -= count;
            if count < run {
                // The input runs out within the run: keep what it decoded.
                checkpoint!(Pending::Literals { command, left });
            }
        }
        if c.pos == needed {
            break;
        }

        if reader.remaining_bits() < STEP_BITS {
            checkpoint!(Pending::Distance { command });
        }
        let distance_symbol = if command.reuse_last_distance {
            0
        } else {
            if c.distance_remaining == 0 {
                let block_type = tri!(header.distances.next_block(reader));
                c.distance_remaining = header.distances.remaining;
                distance_codes_by_context = distance_trees(header, distance_tables, block_type);
            }
            c.distance_remaining -= 1;
            let code = distance_codes_by_context[command.distance_context & 3];
            tri!(decode_prefix_symbol(reader, code)) as usize
        };
        let distance = if no_postfix_distances {
            tri!(read_distance_no_postfix_with_ring(
                reader,
                distance_symbol,
                &c.distances
            ))
        } else {
            tri!(read_distance(
                reader,
                distance_symbol,
                header.npostfix,
                header.ndirect,
                &c.distances,
            ))
        };
        tri!(copy_from_distance::<false>(
            out,
            &mut c.pos,
            CopyRequest {
                needed,
                window_size,
                output_base,
                distance,
                len: command.copy_len,
                push_distance: distance_symbol != 0,
            },
            &mut c.distances,
            raw_dictionary,
        ));
    }

    checkpoint!(Pending::Header);
    Ok(Step::Done)
}
