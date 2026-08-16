use alloc::{vec, vec::Vec};

use burli_core::{
    BurliError, CompressError, Options,
    bits::{BitWriter, MAX_BITS_PER_OP},
    format::MIN_BLOCK_BITS,
};

mod load;
mod q0;
mod q1;
mod q2;
mod q3;
mod q4;
mod q5;
mod sparse;
mod static_dictionary_hash;
mod tune;

const MAX_LITERAL_ONLY_QUALITY: u8 = 5;
const MAX_META_BLOCK_SIZE: usize = 1 << 24;
const MIN_MATCH_BYTES: usize = 4;
const LITERAL_ALPHABET_SIZE: usize = 256;
const COMMAND_ALPHABET_SIZE: usize = 704;
const CODE_LENGTH_ALPHABET_SIZE: usize = 18;
const MAX_CODE_BITS: u8 = 15;
const FAST_CODE_BITS: u8 = 14;
const CODE_LENGTH_ORDER: [u8; 18] = [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];
const MAX_SIMPLE_PREFIX_SYMBOLS: usize = 4;
const INITIAL_LAST_DISTANCE: usize = 4;
const STATIC_CODE_LENGTH_DEPTH: [u8; CODE_LENGTH_ALPHABET_SIZE] =
    [4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 0, 4, 4];
const STATIC_CODE_LENGTH_BITS: [u16; CODE_LENGTH_ALPHABET_SIZE] =
    [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 15, 31, 0, 11, 7];

#[derive(Clone, Debug, Default)]
pub(crate) struct Workspace {
    q0: q0::Workspace,
    q1: q1::Workspace,
    q2: q2::Workspace,
    q3: q3::Workspace,
    q4: q4::Workspace,
    q5: q5::Workspace,
    token_prefix: PrefixCodeScratch,
}

#[derive(Clone, Debug, Default)]
struct PrefixCodeScratch {
    used: Vec<(u16, usize)>,
    nodes: Vec<HuffmanNode>,
    leaves: Vec<(usize, usize)>,
    parent_queue: Vec<usize>,
    lengths: Vec<u8>,
    tree: Vec<u16>,
}

impl PrefixCodeScratch {
    fn reserve_for(&mut self, frequency_symbols: usize, tree_symbols: usize) {
        self.used
            .reserve(frequency_symbols.saturating_sub(self.used.capacity()));
        self.nodes.reserve(
            frequency_symbols
                .saturating_mul(2)
                .saturating_sub(1)
                .saturating_sub(self.nodes.capacity()),
        );
        self.leaves
            .reserve(frequency_symbols.saturating_sub(self.leaves.capacity()));
        self.parent_queue.reserve(
            frequency_symbols
                .saturating_sub(1)
                .saturating_sub(self.parent_queue.capacity()),
        );
        self.lengths
            .reserve(frequency_symbols.saturating_sub(self.lengths.capacity()));
        self.tree
            .reserve(tree_symbols.saturating_sub(self.tree.capacity()));
    }
}

pub fn compress_with_options(input: &[u8], options: &Options) -> Result<Vec<u8>, CompressError> {
    let mut workspace = Workspace::default();
    compress_with_options_workspace(input, options, &mut workspace)
}

impl Workspace {
    fn reset_stream(&mut self) {
        self.q2.reset();
        self.q3.reset();
        self.q4.reset();
        self.q5.reset();
    }
}

pub(crate) fn compress_with_options_workspace(
    input: &[u8],
    options: &Options,
    workspace: &mut Workspace,
) -> Result<Vec<u8>, CompressError> {
    let mut writer = BitWriter::with_capacity(max_literal_only_size(input.len()));
    let mut output = Vec::new();
    compress_into_with_options_workspace(input, options, workspace, &mut writer, &mut output)?;
    Ok(output)
}

pub(crate) fn compress_into_with_options_workspace(
    input: &[u8],
    options: &Options,
    workspace: &mut Workspace,
    writer: &mut BitWriter,
    output: &mut Vec<u8>,
) -> Result<usize, CompressError> {
    if options.quality_value() > MAX_LITERAL_ONLY_QUALITY {
        return Err(BurliError::Unsupported(
            "only q0..q5 Brotli encoding is implemented yet",
        ));
    }
    workspace.reset_stream();
    if options.quality_value() == 0 && !input.is_empty() && input.len() <= 256 {
        let before = output.len();
        let uncompressed = crate::metablock::compress_uncompressed_with_options(input, options)?;
        output.extend_from_slice(&uncompressed);
        return Ok(output.len() - before);
    }

    writer.clear();
    writer.reserve(max_literal_only_size(input.len()));
    crate::metablock::write_window_bits(writer, options.window_bits_value())?;
    if input.is_empty() {
        crate::metablock::write_last_empty_meta_block(writer)?;
        return Ok(writer.finish_into(output));
    }

    write_compressed_meta_blocks(writer, input, options, workspace)?;
    crate::metablock::write_last_empty_meta_block(writer)?;

    let compressed_len = writer.finished_len();
    if compressed_len < input.len() {
        return Ok(writer.finish_into(output));
    }

    let uncompressed_options = options.clone().quality(0)?;
    let uncompressed =
        crate::metablock::compress_uncompressed_with_options(input, &uncompressed_options)?;
    if uncompressed.len() < compressed_len {
        writer.clear();
        let before = output.len();
        output.extend_from_slice(&uncompressed);
        Ok(output.len() - before)
    } else {
        Ok(writer.finish_into(output))
    }
}

pub(crate) fn q0_store_stats(
    input: &[u8],
    options: &Options,
) -> Result<crate::diagnostics::Q0StoreStats, CompressError> {
    let mut stats = crate::diagnostics::Q0StoreStats {
        input_bytes: input.len(),
        ..crate::diagnostics::Q0StoreStats::default()
    };
    let plan = EncoderPlan::from_options(input.len(), options)?;
    if plan.path != EncoderPath::FastOnePass {
        return Ok(stats);
    }

    let allow_cross = input.len() <= plan.block_size;
    let mut input_base = 0_usize;
    for chunk in input.chunks(plan.block_size) {
        stats.blocks += 1;
        let decision = sparse::decision(chunk);
        let Some(sample) = decision.sample else {
            input_base += chunk.len();
            continue;
        };

        stats.sampled_blocks += 1;
        stats.sampled_positions += sample.len;
        stats.sampled_load_bytes += sample.len * 8;
        stats.duplicate_6_count += sample.duplicate_6_count;
        stats.sampled_match_bytes += sample.duplicate_6_count * 6;
        stats.zero_count += sample.zero_count;
        stats.printable_count += sample.printable_count;
        stats.max_sample_miss_streak = stats.max_sample_miss_streak.max(sample.max_miss_streak);

        if decision.store_uncompressed {
            stats.store_candidate_blocks += 1;
        }

        if decision.store_uncompressed && sparse::q0_store_block(input_base, allow_cross) {
            stats.stored_blocks += 1;
            stats.stored_bytes += chunk.len();
            let full_probe_positions = chunk.len().saturating_sub(7);
            stats.skipped_probe_positions += full_probe_positions.saturating_sub(sample.len);
        }
        input_base += chunk.len();
    }

    Ok(stats)
}

#[cfg(feature = "std")]
pub(crate) fn write_stream_header(
    writer: &mut BitWriter,
    options: &Options,
) -> Result<(), CompressError> {
    if options.quality_value() > MAX_LITERAL_ONLY_QUALITY {
        return Err(BurliError::Unsupported(
            "only q0..q5 Brotli encoding is implemented yet",
        ));
    }
    crate::metablock::write_window_bits(writer, options.window_bits_value())
}

#[cfg(feature = "std")]
pub(crate) fn write_stream_chunk_with_workspace(
    writer: &mut BitWriter,
    input: &[u8],
    input_base: usize,
    allow_cross_collector_shortcuts: bool,
    options: &Options,
    workspace: &mut Workspace,
) -> Result<(), CompressError> {
    if input.is_empty() {
        return Ok(());
    }
    if options.quality_value() > MAX_LITERAL_ONLY_QUALITY {
        return Err(BurliError::Unsupported(
            "only q0..q5 Brotli encoding is implemented yet",
        ));
    }
    let plan = EncoderPlan::from_options(input.len(), options)?;
    plan.write_meta_block_with_workspace(
        writer,
        input,
        input_base,
        allow_cross_collector_shortcuts,
        workspace,
    )
}

pub(crate) fn encode_literal_fragment_with_options(
    input: &[u8],
    options: &Options,
) -> Result<(Vec<u8>, usize), CompressError> {
    if options.quality_value() > MAX_LITERAL_ONLY_QUALITY {
        return Err(BurliError::Unsupported(
            "only q0..q5 Brotli concat fragments are implemented yet",
        ));
    }

    let mut writer = BitWriter::with_capacity(max_literal_only_size(input.len()));
    let block_size = options
        .block_bits_value()
        .map_or(MAX_META_BLOCK_SIZE, |bits| 1_usize << bits)
        .min(MAX_META_BLOCK_SIZE);
    for chunk in input.chunks(block_size) {
        write_compressed_literal_meta_block(&mut writer, chunk)?;
    }
    let bit_len = writer.written_bits();
    Ok((writer.into_bytes(), bit_len))
}

pub(crate) fn encode_concat_fragment_with_options(
    input: &[u8],
    options: &Options,
) -> Result<(Vec<u8>, usize, bool), CompressError> {
    if options.quality_value() > MAX_LITERAL_ONLY_QUALITY {
        return Err(BurliError::Unsupported(
            "only q0..q5 Brotli concat fragments are implemented yet",
        ));
    }
    if input.is_empty() {
        return Ok((Vec::new(), 0, false));
    }

    let mut writer = BitWriter::with_capacity(max_literal_only_size(input.len()));
    let mut workspace = Workspace::default();
    workspace.reset_stream();
    let plan = EncoderPlan::from_options(input.len(), options)?;
    let mut has_copy = false;

    for chunk in input.chunks(plan.block_size) {
        has_copy |= write_concat_meta_block(
            &mut writer,
            chunk,
            plan.max_backward_distance.min(chunk.len()),
            options.quality_value(),
            &mut workspace,
        )?;
    }

    let bit_len = writer.written_bits();
    Ok((writer.into_bytes(), bit_len, has_copy))
}

pub(crate) fn write_stream_header_to_writer(
    writer: &mut BitWriter,
    options: &Options,
) -> Result<(), CompressError> {
    if options.quality_value() > MAX_LITERAL_ONLY_QUALITY {
        return Err(BurliError::Unsupported(
            "only q0..q5 Brotli concat streams are implemented yet",
        ));
    }
    crate::metablock::write_window_bits(writer, options.window_bits_value())
}

pub(crate) fn write_last_empty_meta_block_to_writer(
    writer: &mut BitWriter,
) -> Result<(), CompressError> {
    crate::metablock::write_last_empty_meta_block(writer)
}

fn max_literal_only_size(input_len: usize) -> usize {
    input_len
        .saturating_add(input_len / 1024)
        .saturating_add(256)
}

fn write_compressed_meta_blocks(
    writer: &mut BitWriter,
    input: &[u8],
    options: &Options,
    workspace: &mut Workspace,
) -> Result<(), CompressError> {
    let plan = EncoderPlan::from_options(input.len(), options)?;
    let allow_cross_collector_shortcuts = input.len() <= plan.block_size;

    let mut input_base = 0_usize;
    for chunk in input.chunks(plan.block_size) {
        plan.write_meta_block_with_workspace(
            writer,
            chunk,
            input_base,
            allow_cross_collector_shortcuts,
            workspace,
        )?;
        input_base = input_base
            .checked_add(chunk.len())
            .ok_or(BurliError::Format("Brotli input position overflow"))?;
    }

    Ok(())
}

fn write_concat_meta_block(
    writer: &mut BitWriter,
    input: &[u8],
    max_backward_distance: usize,
    quality: u8,
    workspace: &mut Workspace,
) -> Result<bool, CompressError> {
    if input.len() < MIN_MATCH_BYTES {
        write_compressed_literal_meta_block(writer, input)?;
        return Ok(false);
    }

    let mut tokens = match quality {
        0 => q2::collect_without_dictionary_no_lazy_sparse_tail_no_last_distance_probe(
            input,
            max_backward_distance,
            &mut workspace.q2,
        ),
        1 => q2::collect_without_dictionary_no_lazy_sparse_tail(
            input,
            max_backward_distance,
            &mut workspace.q2,
        ),
        2 => {
            q2::collect_without_dictionary_no_lazy(input, max_backward_distance, &mut workspace.q2)
        }
        3 => {
            q2::collect_without_dictionary_one_lazy(input, max_backward_distance, &mut workspace.q2)
        }
        _ => q2::collect_without_dictionary(input, max_backward_distance, &mut workspace.q2),
    };

    sanitize_concat_tokens(&mut tokens)?;
    if !tokens.iter().any(|token| token.is_copy()) {
        write_compressed_literal_meta_block(writer, input)?;
        return Ok(false);
    }

    let symbol_limit = if quality <= 1 {
        tune::Q1_DELAYED_SYMBOLS
    } else {
        tune::MAX_DELAYED_SYMBOLS
    };
    write_token_batches_with_symbol_limit(writer, input, &tokens, symbol_limit)?;
    Ok(true)
}

fn sanitize_concat_tokens(tokens: &mut [Token]) -> Result<(), CompressError> {
    for token in tokens {
        if !token.is_copy() {
            continue;
        }

        let copy_position = token
            .insert_start
            .checked_add(token.insert_len)
            .ok_or(BurliError::Format("Brotli concat token position overflow"))?;
        if token.distance == 0 || token.distance > copy_position {
            return Err(BurliError::Format(
                "Brotli concat token uses non-local distance",
            ));
        }
        token.distance_code = None;
        token.use_last_distance = false;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EncoderPlan {
    path: EncoderPath,
    block_size: usize,
    max_backward_distance: usize,
    q1_fast_literal_prefix: bool,
}

impl EncoderPlan {
    fn from_options(input_len: usize, options: &Options) -> Result<Self, CompressError> {
        let quality = options.quality_value();
        if quality > MAX_LITERAL_ONLY_QUALITY {
            return Err(BurliError::Unsupported(
                "only q0..q5 Brotli encoding is implemented yet",
            ));
        }

        let max_backward_distance = (1_usize << options.window_bits_value()) - 16;
        let block_size = match options.block_bits_value() {
            Some(bits) => 1_usize << bits,
            None if quality == 0 && input_len <= max_backward_distance => MAX_META_BLOCK_SIZE,
            None if quality == 0 => 1_usize << 18,
            None if quality == 1 && input_len <= max_backward_distance => MAX_META_BLOCK_SIZE,
            None if quality == 1 => max_backward_distance + 16,
            None if quality == 2 => ((max_backward_distance + 16) << 1).min(MAX_META_BLOCK_SIZE),
            None if quality == 3 => ((max_backward_distance + 16) << 1).min(MAX_META_BLOCK_SIZE),
            None if quality == 4 => ((max_backward_distance + 16) << 1).min(MAX_META_BLOCK_SIZE),
            None if quality == 5 => ((max_backward_distance + 16) << 1).min(MAX_META_BLOCK_SIZE),
            None => 1_usize << MIN_BLOCK_BITS,
        }
        .min(MAX_META_BLOCK_SIZE);

        Ok(Self {
            path: EncoderPath::for_quality(quality),
            block_size,
            max_backward_distance,
            q1_fast_literal_prefix: quality != 1 || input_len <= max_backward_distance,
        })
    }

    fn write_meta_block_with_workspace(
        self,
        writer: &mut BitWriter,
        input: &[u8],
        input_base: usize,
        allow_cross_collector_shortcuts: bool,
        workspace: &mut Workspace,
    ) -> Result<(), CompressError> {
        let local_max_backward_distance = self.max_backward_distance.min(input.len());
        let global_max_backward_distance = self.max_backward_distance;
        if input.len() < self.path.min_match_len() {
            return write_compressed_literal_meta_block(writer, input);
        }

        if self.path == EncoderPath::FastOnePass {
            if input.len() <= tune::Q0_STATIC_ENTROPY_MAX_INPUT
                && input.len() > tune::Q0_DIRECT_MAX_INPUT
            {
                let tokens = q2::collect_without_dictionary_no_lazy(
                    input,
                    local_max_backward_distance,
                    &mut workspace.q2,
                );
                if !tokens.iter().any(|token| token.is_copy()) {
                    if input.len() <= 512 {
                        return write_fast_compressed_literal_meta_block(writer, input);
                    }
                    return write_compressed_literal_meta_block(writer, input);
                }
                return write_token_batches_with_symbol_limit(
                    writer,
                    input,
                    &tokens,
                    tune::MAX_DELAYED_SYMBOLS,
                );
            }

            if input.len() > tune::Q0_DIRECT_MAX_INPUT {
                let sparse_decision = sparse::decision(input);
                if sparse_decision.store_uncompressed {
                    if sparse::q0_store_block(input_base, allow_cross_collector_shortcuts) {
                        return crate::metablock::write_uncompressed_meta_block(writer, input);
                    }
                    let has_copy = {
                        let batch = q1::collect_with_64k_sparse_stride(
                            input,
                            local_max_backward_distance,
                            &mut workspace.q1,
                        );
                        batch.has_copy()
                    };
                    if !has_copy {
                        return write_compressed_literal_meta_block(writer, input);
                    }
                    return q0_write_collected(
                        writer,
                        input,
                        &mut workspace.q1,
                        self.q1_fast_literal_prefix,
                        q0_write_route(input.len(), sparse_decision.sample),
                    );
                }

                let has_copy = {
                    let batch = q0_collect_by_size(
                        input,
                        local_max_backward_distance,
                        sparse_decision.sample,
                        workspace,
                    );
                    batch.has_copy()
                };
                if !has_copy {
                    return write_compressed_literal_meta_block(writer, input);
                }
                return q0_write_collected(
                    writer,
                    input,
                    &mut workspace.q1,
                    self.q1_fast_literal_prefix,
                    q0_write_route(input.len(), sparse_decision.sample),
                );
            }

            let has_copy = {
                let batch = q0::collect(input, local_max_backward_distance, &mut workspace.q0)?;
                batch.has_copy()
            };
            if !has_copy {
                return write_compressed_literal_meta_block(writer, input);
            }
            return q0::write(writer, input, input.len(), &mut workspace.q0);
        }

        if self.write_sparse_binary_meta_block(
            writer,
            input,
            input_base,
            local_max_backward_distance,
            global_max_backward_distance,
            workspace,
        )? {
            return Ok(());
        }

        if self.path == EncoderPath::FastTwoPass {
            if input.len() <= tune::Q1_STATIC_ENTROPY_MAX_INPUT {
                let tokens = q2::collect_without_dictionary_no_lazy(
                    input,
                    local_max_backward_distance,
                    &mut workspace.q2,
                );
                if !tokens.iter().any(|token| token.is_copy()) {
                    return write_compressed_literal_meta_block(writer, input);
                }
                return write_recomputed_token_batches_with_symbol_limit(
                    writer,
                    input,
                    &tokens,
                    tune::Q1_DELAYED_SYMBOLS,
                    &mut workspace.token_prefix,
                );
            }

            if !allow_cross_collector_shortcuts {
                if input.len() >= tune::Q1_LONG_INPUT_MIN {
                    let has_copy = {
                        let batch =
                            q1::collect(input, local_max_backward_distance, &mut workspace.q1)?;
                        batch.has_copy()
                    };
                    if !has_copy {
                        return write_compressed_literal_meta_block(writer, input);
                    }
                    return q1::write(
                        writer,
                        input,
                        input.len(),
                        &mut workspace.q1,
                        self.q1_fast_literal_prefix,
                    );
                }
                let tokens = if q1_large_markup_lazy_is_likely_safe(input) {
                    q2::collect_without_dictionary_one_lazy(
                        input,
                        local_max_backward_distance,
                        &mut workspace.q2,
                    )
                } else if q1_no_cross_fast_writer_is_likely_safe(input) {
                    let batch = q1::collect(input, local_max_backward_distance, &mut workspace.q1)?;
                    if !batch.has_copy() {
                        return write_compressed_literal_meta_block(writer, input);
                    }
                    return q1::write(
                        writer,
                        input,
                        input.len(),
                        &mut workspace.q1,
                        self.q1_fast_literal_prefix,
                    );
                } else if q1_no_cross_one_lazy_is_likely_safe(input) {
                    q2::collect_without_dictionary_one_lazy(
                        input,
                        local_max_backward_distance,
                        &mut workspace.q2,
                    )
                } else if q1_no_cross_sparse_tail_no_last_is_likely_safe(input) {
                    q2::collect_without_dictionary_no_lazy_sparse_tail_no_last_distance_probe(
                        input,
                        local_max_backward_distance,
                        &mut workspace.q2,
                    )
                } else {
                    q2::collect_without_dictionary_no_lazy_sparse_tail(
                        input,
                        local_max_backward_distance,
                        &mut workspace.q2,
                    )
                };
                if !tokens.iter().any(|token| token.is_copy()) {
                    return write_compressed_literal_meta_block(writer, input);
                }
                return write_token_batches_with_symbol_limit(
                    writer,
                    input,
                    &tokens,
                    tune::Q1_DELAYED_SYMBOLS,
                );
            }

            let has_copy = {
                let batch = if input.len() >= tune::Q1_LONG_INPUT_MIN {
                    q1::collect_with_64k_medium_skip(
                        input,
                        local_max_backward_distance,
                        &mut workspace.q1,
                    )
                } else {
                    q1::collect(input, local_max_backward_distance, &mut workspace.q1)?
                };
                batch.has_copy()
            };
            if !has_copy {
                return write_compressed_literal_meta_block(writer, input);
            }
            return q1::write(
                writer,
                input,
                input.len(),
                &mut workspace.q1,
                self.q1_fast_literal_prefix,
            );
        }

        if self.path == EncoderPath::StaticEntropy {
            if allow_cross_collector_shortcuts
                && (tune::Q2_MEDIUM_H3_MIN_INPUT..=tune::Q2_MEDIUM_H3_MAX_INPUT)
                    .contains(&input.len())
            {
                let tokens = if input.len() <= tune::Q2_FAST_H3_MAX_INPUT {
                    q3::collect_fast_sweep_no_lazy(
                        input,
                        local_max_backward_distance,
                        &mut workspace.q3,
                    )
                } else if input.len() <= tune::Q2_SWEEP1_H3_MAX_INPUT {
                    q3::collect_fast_sweep(input, local_max_backward_distance, &mut workspace.q3)
                } else {
                    q3::collect(input, local_max_backward_distance, &mut workspace.q3)
                };
                if !tokens.iter().any(|token| token.is_copy()) {
                    return write_compressed_literal_meta_block(writer, input);
                }
                return write_regular_token_batches_with_symbol_limit(
                    writer,
                    input,
                    &tokens,
                    tune::MAX_DELAYED_SYMBOLS,
                );
            }

            let tokens = if input.len() < tune::Q2_STATIC_NO_DICTIONARY_MAX_INPUT {
                q2::collect_without_dictionary(
                    input,
                    local_max_backward_distance,
                    &mut workspace.q2,
                )
            } else {
                q2::collect(
                    input,
                    input_base,
                    global_max_backward_distance,
                    &mut workspace.q2,
                )
            };
            if !tokens.iter().any(|token| token.is_copy()) {
                return write_compressed_literal_meta_block(writer, input);
            }
            return write_token_batches_with_symbol_limit(
                writer,
                input,
                &tokens,
                tune::MAX_DELAYED_SYMBOLS,
            );
        }

        if self.path == EncoderPath::RegularNoSplit {
            let use_medium_sweep1 = (tune::Q3_MEDIUM_SWEEP1_MIN_INPUT
                ..=tune::Q3_MEDIUM_SWEEP1_MAX_INPUT)
                .contains(&input.len());
            let tokens = if input.len() <= tune::Q3_FAST_SWEEP_MAX_INPUT || use_medium_sweep1 {
                q3::collect_fast_sweep(input, local_max_backward_distance, &mut workspace.q3)
            } else {
                q3::collect(input, local_max_backward_distance, &mut workspace.q3)
            };
            if !tokens.iter().any(|token| token.is_copy()) {
                return write_compressed_literal_meta_block(writer, input);
            }
            return write_regular_token_batches_with_symbol_limit(
                writer,
                input,
                &tokens,
                tune::MAX_DELAYED_SYMBOLS,
            );
        }

        if self.path == EncoderPath::RegularSplit {
            if allow_cross_collector_shortcuts && input.len() <= tune::Q4_TINY_CONTEXT_MAX_INPUT {
                let tokens = q5::collect(
                    input,
                    input_base,
                    global_max_backward_distance,
                    &mut workspace.q5,
                );
                if !tokens.iter().any(|token| token.is_copy()) {
                    return write_compressed_literal_meta_block(writer, input);
                }
                return write_regular_token_batches_with_symbol_limit(
                    writer,
                    input,
                    &tokens,
                    tune::Q5_DELAYED_SYMBOLS,
                );
            }
            let tokens = q4::collect(
                input,
                input_base,
                global_max_backward_distance,
                &mut workspace.q4,
            );
            if !tokens.iter().any(|token| token.is_copy()) {
                return write_compressed_literal_meta_block(writer, input);
            }
            return write_regular_token_batches_with_symbol_limit(
                writer,
                input,
                &tokens,
                tune::Q4_DELAYED_SYMBOLS,
            );
        }

        if self.path == EncoderPath::ContextModeled {
            let tokens = q5::collect(
                input,
                input_base,
                global_max_backward_distance,
                &mut workspace.q5,
            );
            if !tokens.iter().any(|token| token.is_copy()) {
                return write_compressed_literal_meta_block(writer, input);
            }
            return write_regular_token_batches_with_symbol_limit(
                writer,
                input,
                &tokens,
                tune::Q5_DELAYED_SYMBOLS,
            );
        }

        unreachable!("all scoped encoder paths are handled above")
    }

    fn write_sparse_binary_meta_block(
        self,
        writer: &mut BitWriter,
        input: &[u8],
        _input_base: usize,
        local_max_backward_distance: usize,
        _global_max_backward_distance: usize,
        workspace: &mut Workspace,
    ) -> Result<bool, CompressError> {
        let sparse_decision = sparse::decision(input);
        if !sparse_decision.store_uncompressed {
            return Ok(false);
        }

        match self.path {
            EncoderPath::FastTwoPass => {
                write_split_q1_sparse_binary_meta_blocks(
                    writer,
                    input,
                    local_max_backward_distance,
                    &mut workspace.q1,
                    self.q1_fast_literal_prefix,
                )?;
            }
            EncoderPath::StaticEntropy => {
                let tokens = sparse::collect_tokens(
                    input,
                    local_max_backward_distance,
                    tune::Q2_LOW_COMPRESS_SPARSE_STRIDE,
                );
                if !tokens.iter().any(|token| token.is_copy()) {
                    write_compressed_literal_meta_block(writer, input)?;
                    return Ok(true);
                }
                write_token_batches_with_symbol_limit(
                    writer,
                    input,
                    &tokens,
                    tune::MAX_DELAYED_SYMBOLS,
                )?;
            }
            EncoderPath::RegularNoSplit => {
                let tokens = sparse::collect_tokens(
                    input,
                    local_max_backward_distance,
                    tune::Q3_LOW_COMPRESS_SPARSE_STRIDE,
                );
                if !tokens.iter().any(|token| token.is_copy()) {
                    write_compressed_literal_meta_block(writer, input)?;
                    return Ok(true);
                }
                write_regular_token_batches_with_symbol_limit(
                    writer,
                    input,
                    &tokens,
                    tune::MAX_DELAYED_SYMBOLS,
                )?;
            }
            EncoderPath::RegularSplit => {
                let tokens = sparse::collect_tokens(
                    input,
                    local_max_backward_distance,
                    tune::Q4_LOW_COMPRESS_SPARSE_STRIDE,
                );
                if !tokens.iter().any(|token| token.is_copy()) {
                    write_compressed_literal_meta_block(writer, input)?;
                    return Ok(true);
                }
                write_regular_token_batches_with_symbol_limit(
                    writer,
                    input,
                    &tokens,
                    tune::LOW_COMPRESS_DELAYED_SYMBOLS,
                )?;
            }
            EncoderPath::ContextModeled => {
                let tokens = sparse::collect_tokens(
                    input,
                    local_max_backward_distance,
                    tune::Q5_LOW_COMPRESS_SPARSE_STRIDE,
                );
                if !tokens.iter().any(|token| token.is_copy()) {
                    write_compressed_literal_meta_block(writer, input)?;
                    return Ok(true);
                }
                write_regular_token_batches_with_symbol_limit(
                    writer,
                    input,
                    &tokens,
                    tune::LOW_COMPRESS_DELAYED_SYMBOLS,
                )?;
            }
            EncoderPath::FastOnePass => unreachable!("q0 sparse path is handled separately"),
        }
        Ok(true)
    }
}

fn write_split_q1_sparse_binary_meta_blocks(
    writer: &mut BitWriter,
    input: &[u8],
    local_max_backward_distance: usize,
    workspace: &mut q1::Workspace,
    fast_literal_prefix: bool,
) -> Result<(), CompressError> {
    for (block_index, chunk) in input.chunks(tune::Q1_LOW_COMPRESS_BLOCK_SIZE).enumerate() {
        let block_in_group = block_index & tune::Q1_LOW_COMPRESS_STORE_BLOCK_MASK;
        if (tune::Q1_LOW_COMPRESS_STORE_BLOCKS & (1 << block_in_group)) != 0 {
            crate::metablock::write_uncompressed_meta_block(writer, chunk)?;
            continue;
        }

        let has_copy = {
            let batch = q1::collect_with_64k_sparse_stride(
                chunk,
                local_max_backward_distance.min(chunk.len()),
                workspace,
            );
            batch.has_copy()
        };
        if !has_copy {
            write_compressed_literal_meta_block(writer, chunk)?;
            continue;
        }
        q1::write(writer, chunk, chunk.len(), workspace, fast_literal_prefix)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EncoderPath {
    FastOnePass,
    FastTwoPass,
    StaticEntropy,
    RegularNoSplit,
    RegularSplit,
    ContextModeled,
}

impl EncoderPath {
    const fn for_quality(quality: u8) -> Self {
        match quality {
            0 => Self::FastOnePass,
            1 => Self::FastTwoPass,
            2 => Self::StaticEntropy,
            3 => Self::RegularNoSplit,
            4 => Self::RegularSplit,
            _ => Self::ContextModeled,
        }
    }

    const fn min_match_len(self) -> usize {
        match self {
            Self::FastOnePass => 5,
            Self::FastTwoPass => 4,
            Self::StaticEntropy => min_match_len(2),
            Self::RegularNoSplit => min_match_len(3),
            Self::RegularSplit => min_match_len(4),
            Self::ContextModeled => min_match_len(5),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Token {
    insert_start: usize,
    insert_len: usize,
    copy_len: usize,
    copy_len_code: usize,
    distance: usize,
    distance_code: Option<u16>,
    use_last_distance: bool,
}

impl Token {
    const fn is_copy(self) -> bool {
        self.copy_len != 0
    }

    const fn block_len(self) -> usize {
        self.insert_len + self.copy_len
    }

    const fn copy_len_code(self) -> usize {
        if self.copy_len_code == 0 {
            self.copy_len
        } else {
            self.copy_len_code
        }
    }
}

#[derive(Clone, Debug)]
struct PreparedBatch {
    prepared: Vec<PreparedToken>,
    literal_frequencies: [usize; LITERAL_ALPHABET_SIZE],
    command_frequencies: [usize; COMMAND_ALPHABET_SIZE],
    distance_frequencies: [usize; 64],
    has_distance: bool,
}

impl PreparedBatch {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            prepared: Vec::with_capacity(capacity),
            literal_frequencies: [0; LITERAL_ALPHABET_SIZE],
            command_frequencies: [0; COMMAND_ALPHABET_SIZE],
            distance_frequencies: [0; 64],
            has_distance: false,
        }
    }

    fn push(&mut self, input: &[u8], token: Token) -> Result<(), CompressError> {
        let prepared_token = PreparedToken::new(token)?;
        let token = prepared_token.token;
        for &literal in &input[token.insert_start..token.insert_start + token.insert_len] {
            self.literal_frequencies[usize::from(literal)] += 1;
        }
        self.command_frequencies[usize::from(prepared_token.command_symbol)] += 1;
        if let Some(distance) = prepared_token.distance {
            self.distance_frequencies[usize::from(distance.symbol)] += 1;
            self.has_distance = true;
        }
        self.prepared.push(prepared_token);
        Ok(())
    }

    fn ensure_distance_frequencies(&mut self) {
        if !self.has_distance {
            self.distance_frequencies[0] = 1;
        }
    }
}

#[derive(Clone, Debug)]
struct TokenBatchFrequencies {
    literal_frequencies: [usize; LITERAL_ALPHABET_SIZE],
    command_frequencies: [usize; COMMAND_ALPHABET_SIZE],
    distance_frequencies: [usize; 64],
    has_distance: bool,
}

impl TokenBatchFrequencies {
    fn new() -> Self {
        Self {
            literal_frequencies: [0; LITERAL_ALPHABET_SIZE],
            command_frequencies: [0; COMMAND_ALPHABET_SIZE],
            distance_frequencies: [0; 64],
            has_distance: false,
        }
    }

    fn push(&mut self, input: &[u8], token: Token) -> Result<(), CompressError> {
        let prepared_token = PreparedToken::new(token)?;
        let token = prepared_token.token;
        for &literal in &input[token.insert_start..token.insert_start + token.insert_len] {
            self.literal_frequencies[usize::from(literal)] += 1;
        }
        self.command_frequencies[usize::from(prepared_token.command_symbol)] += 1;
        if let Some(distance) = prepared_token.distance {
            self.distance_frequencies[usize::from(distance.symbol)] += 1;
            self.has_distance = true;
        }
        Ok(())
    }

    fn ensure_distance_frequencies(&mut self) {
        if !self.has_distance {
            self.distance_frequencies[0] = 1;
        }
    }
}

#[derive(Clone, Debug)]
struct StaticEntropyBatch {
    prepared: Vec<PreparedToken>,
    literal_frequencies: [usize; LITERAL_ALPHABET_SIZE],
}

impl StaticEntropyBatch {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            prepared: Vec::with_capacity(capacity),
            literal_frequencies: [0; LITERAL_ALPHABET_SIZE],
        }
    }

    fn push(&mut self, input: &[u8], token: Token) -> Result<(), CompressError> {
        let prepared_token = PreparedToken::new(token)?;
        let token = prepared_token.token;
        for &literal in &input[token.insert_start..token.insert_start + token.insert_len] {
            self.literal_frequencies[usize::from(literal)] += 1;
        }
        self.prepared.push(prepared_token);
        Ok(())
    }
}

fn token_supports_last_distance(token: Token) -> bool {
    let Ok(insert) = insert_length_code(token.insert_len) else {
        return false;
    };
    let Ok(copy) = copy_length_code(token.copy_len) else {
        return false;
    };
    insert.code < 8 && copy.code < 16
}

fn write_token_batches_with_symbol_limit(
    writer: &mut BitWriter,
    input: &[u8],
    tokens: &[Token],
    symbol_limit: usize,
) -> Result<(), CompressError> {
    let mut start = 0;
    while start < tokens.len() {
        let (local_end, block_len) =
            q2_token_batch_span_with_symbol_limit(&tokens[start..], symbol_limit);
        let end = local_end + start;
        write_token_batch_q2_with_len(writer, input, &tokens[start..end], block_len)?;
        start = end;
    }
    Ok(())
}

fn write_recomputed_token_batches_with_symbol_limit(
    writer: &mut BitWriter,
    input: &[u8],
    tokens: &[Token],
    symbol_limit: usize,
    prefix: &mut PrefixCodeScratch,
) -> Result<(), CompressError> {
    let mut start = 0;
    while start < tokens.len() {
        let (local_end, block_len) =
            q2_token_batch_span_with_symbol_limit(&tokens[start..], symbol_limit);
        let end = local_end + start;
        write_token_batch_q2_recomputed_with_len(
            writer,
            input,
            &tokens[start..end],
            block_len,
            prefix,
        )?;
        start = end;
    }
    Ok(())
}

fn write_regular_token_batches_with_symbol_limit(
    writer: &mut BitWriter,
    input: &[u8],
    tokens: &[Token],
    symbol_limit: usize,
) -> Result<(), CompressError> {
    if q2_token_batch_end_with_symbol_limit(tokens, symbol_limit) == tokens.len() {
        debug_assert_eq!(
            tokens.iter().map(|token| token.block_len()).sum::<usize>(),
            input.len()
        );
        return write_token_batch_with_len(writer, input, tokens, input.len());
    }

    let mut start = 0;
    while start < tokens.len() {
        let end = q2_token_batch_end_with_symbol_limit(&tokens[start..], symbol_limit) + start;
        write_token_batch(writer, input, &tokens[start..end])?;
        start = end;
    }
    Ok(())
}

fn q2_token_batch_end_with_symbol_limit(tokens: &[Token], symbol_limit: usize) -> usize {
    q2_token_batch_span_with_symbol_limit(tokens, symbol_limit).0
}

fn q2_token_batch_span_with_symbol_limit(tokens: &[Token], symbol_limit: usize) -> (usize, usize) {
    let mut symbols = 0_usize;
    let mut block_len = 0_usize;

    for (index, &token) in tokens.iter().enumerate() {
        if index != 0 && symbols >= symbol_limit {
            return (index, block_len);
        }
        symbols = symbols.saturating_add(token.insert_len).saturating_add(1);
        block_len = block_len.saturating_add(token.block_len());
    }

    (tokens.len(), block_len)
}

fn push_unique(symbols: &mut Vec<u16>, symbol: u16) {
    if !symbols.contains(&symbol) {
        symbols.push(symbol);
    }
}

const fn min_match_len(quality: u8) -> usize {
    match quality {
        0 => 5,
        1 => 64,
        2 => 48,
        3 => 32,
        4 => 24,
        _ => 16,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Q0CollectRoute {
    FastNoLastDistance,
    NoLastDistance,
    DefaultSkip,
    MediumNoLastDistance,
    MediumSkip,
    K64FastSkip,
    K64MediumSkip,
    K64U16Skip,
    K32U16Skip,
    K32DenseSkip,
    K32FasterSkip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Q0WriteRoute {
    Standard,
    BalancedCommand,
    FastCommand,
    BalancedLiteralCommand,
    PackedLiteralBody,
}

fn q0_collect_route(input_len: usize, sample: Option<sparse::Sample>) -> Q0CollectRoute {
    match input_len {
        0..=tune::Q0_COLLECT_FAST_NO_LAST_MAX_INPUT => Q0CollectRoute::FastNoLastDistance,
        _ if input_len <= tune::Q0_COLLECT_NO_LAST_MAX_INPUT => Q0CollectRoute::NoLastDistance,
        _ if input_len <= tune::Q0_COLLECT_DEFAULT_MAX_INPUT => Q0CollectRoute::DefaultSkip,
        _ if input_len <= tune::Q0_COLLECT_MEDIUM_NO_LAST_MAX_INPUT => {
            Q0CollectRoute::MediumNoLastDistance
        }
        _ if input_len <= tune::Q0_COLLECT_MEDIUM_MAX_INPUT => Q0CollectRoute::MediumSkip,
        _ if input_len <= 64 * 1024 && q0_dense_sample(sample) => Q0CollectRoute::K32U16Skip,
        _ if input_len <= tune::Q0_COLLECT_SAMPLED_MAX_INPUT && q0_dense_sample(sample) => {
            Q0CollectRoute::K32DenseSkip
        }
        _ if input_len <= tune::Q0_COLLECT_SAMPLED_MAX_INPUT && q0_low_dup_sample(sample) => {
            Q0CollectRoute::K64MediumSkip
        }
        _ if input_len <= 64 * 1024 => Q0CollectRoute::K64U16Skip,
        _ if input_len <= tune::Q0_COLLECT_SAMPLED_MAX_INPUT => Q0CollectRoute::K64FastSkip,
        tune::Q0_COLLECT_HUGE_MIN_INPUT.. => Q0CollectRoute::K64MediumSkip,
        _ => Q0CollectRoute::K32FasterSkip,
    }
}

fn q0_write_route(input_len: usize, sample: Option<sparse::Sample>) -> Q0WriteRoute {
    match input_len {
        _ if input_len > tune::Q0_STATIC_ENTROPY_MAX_INPUT
            && input_len <= tune::Q0_WRITE_BALANCED_LITERAL_MAX_INPUT =>
        {
            Q0WriteRoute::BalancedLiteralCommand
        }
        _ if input_len <= tune::Q0_WRITE_PACKED_LITERAL_MAX_INPUT => {
            Q0WriteRoute::PackedLiteralBody
        }
        _ if input_len <= tune::Q0_WRITE_FAST_COMMAND_MAX_INPUT => Q0WriteRoute::FastCommand,
        _ if input_len > tune::Q0_COLLECT_MEDIUM_MAX_INPUT
            && input_len <= tune::Q0_WRITE_SAMPLED_MAX_INPUT
            && q0_dense_sample(sample) =>
        {
            Q0WriteRoute::PackedLiteralBody
        }
        _ if input_len > tune::Q0_WRITE_SAMPLED_MAX_INPUT && !q0_dense_sample(sample) => {
            Q0WriteRoute::BalancedCommand
        }
        _ => Q0WriteRoute::Standard,
    }
}

fn q0_dense_sample(sample: Option<sparse::Sample>) -> bool {
    sample.is_some_and(|sample| sample.duplicate_6_count >= tune::Q0_DENSE_DUP6_MIN)
}

fn q0_low_dup_sample(sample: Option<sparse::Sample>) -> bool {
    sample.is_some_and(|sample| sample.duplicate_6_count <= tune::Q0_LOW_DUP6_MAX)
}

fn q0_collect_by_size<'a>(
    input: &[u8],
    max_backward_distance: usize,
    sample: Option<sparse::Sample>,
    workspace: &'a mut Workspace,
) -> &'a q1::Batch {
    match q0_collect_route(input.len(), sample) {
        Q0CollectRoute::FastNoLastDistance => {
            q1::collect_q0_2k_fast_no_last(input, max_backward_distance, &mut workspace.q1)
        }
        Q0CollectRoute::NoLastDistance => {
            q1::collect_q0_4k_no_last(input, max_backward_distance, &mut workspace.q1)
        }
        Q0CollectRoute::DefaultSkip => {
            q1::collect_q0_8k_default(input, max_backward_distance, &mut workspace.q1)
        }
        Q0CollectRoute::MediumNoLastDistance => {
            q1::collect_q0_16k_medium_no_last(input, max_backward_distance, &mut workspace.q1)
        }
        Q0CollectRoute::MediumSkip => {
            q1::collect_q0_32k_medium(input, max_backward_distance, &mut workspace.q1)
        }
        Q0CollectRoute::K64FastSkip => {
            q1::collect_with_64k_fast_skip(input, max_backward_distance, &mut workspace.q1)
        }
        Q0CollectRoute::K64MediumSkip => {
            q1::collect_with_64k_medium_skip(input, max_backward_distance, &mut workspace.q1)
        }
        Q0CollectRoute::K64U16Skip => {
            q1::collect_with_64k_u16_skip(input, max_backward_distance, &mut workspace.q1)
        }
        Q0CollectRoute::K32U16Skip => {
            q1::collect_with_32k_u16_skip(input, max_backward_distance, &mut workspace.q1)
        }
        Q0CollectRoute::K32DenseSkip => {
            q1::collect_with_32k_dense_skip(input, max_backward_distance, &mut workspace.q1)
        }
        Q0CollectRoute::K32FasterSkip => {
            q1::collect_with_32k_faster_skip(input, max_backward_distance, &mut workspace.q1)
        }
    }
}

fn q0_write_collected(
    writer: &mut BitWriter,
    input: &[u8],
    workspace: &mut q1::Workspace,
    fast_literal_prefix: bool,
    route: Q0WriteRoute,
) -> Result<(), CompressError> {
    match route {
        Q0WriteRoute::Standard => {
            q1::write_q0(writer, input, input.len(), workspace, fast_literal_prefix)
        }
        Q0WriteRoute::BalancedCommand => q1::write_q0_balanced_command_prefixes(
            writer,
            input,
            input.len(),
            workspace,
            fast_literal_prefix,
        ),
        Q0WriteRoute::FastCommand => q1::write_q0_fast_command_prefixes(
            writer,
            input,
            input.len(),
            workspace,
            fast_literal_prefix,
        ),
        Q0WriteRoute::BalancedLiteralCommand => {
            q1::write_q0_balanced_literal_command_prefixes(writer, input, input.len(), workspace)
        }
        Q0WriteRoute::PackedLiteralBody => q1::write_q0_packed_literal_body(
            writer,
            input,
            input.len(),
            workspace,
            fast_literal_prefix,
        ),
    }
}

fn q1_large_markup_lazy_is_likely_safe(input: &[u8]) -> bool {
    let mut lt_count = 0_usize;
    let mut gt_count = 0_usize;
    for &byte in input.iter().take(tune::Q1_LARGE_MARKUP_SAMPLE_BYTES) {
        lt_count += usize::from(byte == b'<');
        gt_count += usize::from(byte == b'>');
    }
    lt_count >= tune::Q1_LARGE_MARKUP_MIN_LT && gt_count >= tune::Q1_LARGE_MARKUP_MIN_GT
}

fn q1_no_cross_one_lazy_is_likely_safe(input: &[u8]) -> bool {
    let sample = &input[..input.len().min(tune::Q1_CONTENT_SAMPLE_BYTES)];
    if sample.is_empty() {
        return false;
    }

    let mut whitespace = 0_usize;
    let mut ascii_printable = 0_usize;
    let mut high = 0_usize;
    let mut zero = 0_usize;
    let mut alpha = 0_usize;
    for &byte in sample {
        whitespace += usize::from(matches!(byte, b' ' | b'\n' | b'\r' | b'\t'));
        ascii_printable +=
            usize::from((32..=126).contains(&byte) || matches!(byte, b'\n' | b'\r' | b'\t'));
        high += usize::from(byte >= 128);
        zero += usize::from(byte == 0);
        alpha += usize::from(byte.is_ascii_alphabetic());
    }

    let len = sample.len();
    let tabular_text = ascii_printable * 100 >= len * tune::Q1_TABULAR_PRINTABLE_PCT
        && whitespace * 100 >= len * tune::Q1_TABULAR_WHITESPACE_PCT
        && alpha * 100 < len * tune::Q1_TABULAR_ALPHA_MAX_PCT;
    let zero_high_mixed = zero * 100 >= len * tune::Q1_ZERO_HIGH_ZERO_PCT
        && high * 100 >= len * tune::Q1_ZERO_HIGH_HIGH_PCT;
    tabular_text || zero_high_mixed
}

fn q1_no_cross_sparse_tail_no_last_is_likely_safe(input: &[u8]) -> bool {
    let sample = &input[..input.len().min(tune::Q1_CONTENT_SAMPLE_BYTES)];
    if sample.is_empty() {
        return false;
    }

    let mut high = 0_usize;
    let mut zero = 0_usize;
    for &byte in sample {
        high += usize::from(byte >= 128);
        zero += usize::from(byte == 0);
    }

    let len = sample.len();
    zero * 100 >= len * tune::Q1_ZERO_HIGH_ZERO_PCT
        && high * 100 < len * tune::Q1_ZERO_HIGH_HIGH_PCT
}

fn q1_no_cross_fast_writer_is_likely_safe(input: &[u8]) -> bool {
    let sample = &input[..input.len().min(tune::Q1_CONTENT_SAMPLE_BYTES)];
    if sample.is_empty() {
        return false;
    }

    let mut whitespace = 0_usize;
    let mut ascii_printable = 0_usize;
    let mut high = 0_usize;
    let mut zero = 0_usize;
    let mut alpha = 0_usize;
    let mut angle = 0_usize;
    for &byte in sample {
        whitespace += usize::from(matches!(byte, b' ' | b'\n' | b'\r' | b'\t'));
        ascii_printable +=
            usize::from((32..=126).contains(&byte) || matches!(byte, b'\n' | b'\r' | b'\t'));
        high += usize::from(byte >= 128);
        zero += usize::from(byte == 0);
        alpha += usize::from(byte.is_ascii_alphabetic());
        angle += usize::from(matches!(byte, b'<' | b'>'));
    }

    let len = sample.len();
    if zero * 100 >= len * tune::Q1_ZERO_HIGH_ZERO_PCT {
        return false;
    }
    if ascii_printable * 100 >= len * tune::Q1_TABULAR_PRINTABLE_PCT
        && whitespace * 100 >= len * tune::Q1_FAST_WRITER_WHITESPACE_PCT
    {
        return false;
    }
    if ascii_printable * 100 >= len * tune::Q1_FAST_WRITER_TEXT_PRINTABLE_PCT
        && high * 100 < len
        && alpha * 100 >= len * tune::Q1_FAST_WRITER_TEXT_ALPHA_PCT
        && whitespace * 100 >= len * tune::Q1_FAST_WRITER_TEXT_WHITESPACE_PCT
        && angle * 100 < len * tune::Q1_FAST_WRITER_TEXT_ANGLE_MAX_PCT
    {
        return false;
    }

    true
}

fn q2_tiny_balanced_literal_prefix_is_likely_safe(input: &[u8]) -> bool {
    let tiny_input =
        (tune::Q2_TINY_PREFIX_MIN_INPUT..=tune::Q2_TINY_PREFIX_MAX_INPUT).contains(&input.len());
    let tiny_comment = tiny_input && input.starts_with(b"/*!");
    let tiny_css = tiny_input && input.starts_with(b"@");
    tiny_comment || tiny_css
}

#[inline(always)]
fn is_match5(input: &[u8], previous: usize, pos: usize) -> bool {
    input[previous..previous + MIN_MATCH_BYTES] == input[pos..pos + MIN_MATCH_BYTES]
        && input[previous + 4] == input[pos + 4]
}

#[inline(always)]
fn hash_word_q0(word: u64, shift: usize) -> usize {
    (((word << 24).wrapping_mul(0x1e35_a7bd)) >> shift) as usize
}

#[inline(always)]
fn next_hash_word(word: u64, next_byte: u8) -> u64 {
    (word >> 8) | (u64::from(next_byte) << 56)
}

#[inline(always)]
fn read_u64_le(input: &[u8], pos: usize) -> u64 {
    load::read_u64_le_trusted(input, pos)
}

#[inline(always)]
fn read_u32_le(input: &[u8], pos: usize) -> u32 {
    load::read_u32_le_trusted(input, pos)
}

#[inline(always)]
fn match_len(input: &[u8], previous: usize, pos: usize, max_len: usize) -> usize {
    let mut len = 0;
    if max_len >= 8 {
        let diff = read_u64_le(input, previous) ^ read_u64_le(input, pos);
        if diff != 0 {
            return diff.trailing_zeros() as usize / 8;
        }
        len = 8;
    }
    if max_len >= 16 {
        let diff = read_u64_le(input, previous + 8) ^ read_u64_le(input, pos + 8);
        if diff != 0 {
            return 8 + diff.trailing_zeros() as usize / 8;
        }
        len = 16;
    }
    let previous_bytes = &input[previous..previous + max_len];
    let pos_bytes = &input[pos..pos + max_len];
    while len + 8 <= max_len {
        let diff = read_u64_le(previous_bytes, len) ^ read_u64_le(pos_bytes, len);
        if diff != 0 {
            return len + diff.trailing_zeros() as usize / 8;
        }
        len += 8;
    }
    while len < max_len && previous_bytes[len] == pos_bytes[len] {
        len += 1;
    }
    len
}

fn write_compressed_literal_meta_block(
    writer: &mut BitWriter,
    input: &[u8],
) -> Result<(), CompressError> {
    if input.is_empty() || input.len() > MAX_META_BLOCK_SIZE {
        return Err(BurliError::Format("invalid compressed Brotli block size"));
    }

    let insert = insert_length_code(input.len())?;
    let command_symbol = command_symbol_for_insert(insert.code)?;

    write_meta_block_len(writer, input.len())?;
    write_block_and_context_header(writer)?;
    let mut literal_frequencies = vec![0_usize; LITERAL_ALPHABET_SIZE];
    for &literal in input {
        literal_frequencies[usize::from(literal)] += 1;
    }
    let literal_codes =
        write_prefix_code_from_frequencies(writer, LITERAL_ALPHABET_SIZE, &literal_frequencies)?;
    let literal_code_map = symbol_code_map(&literal_codes, LITERAL_ALPHABET_SIZE);
    write_simple_prefix_code_single(writer, COMMAND_ALPHABET_SIZE, command_symbol)?;
    write_simple_prefix_code_single(writer, 64, 0)?;
    writer.write_bits_trusted(insert.extra_bits, insert.extra);
    for &literal in input {
        write_literal(writer, &literal_code_map, literal)?;
    }
    Ok(())
}

fn write_fast_compressed_literal_meta_block(
    writer: &mut BitWriter,
    input: &[u8],
) -> Result<(), CompressError> {
    if input.is_empty() || input.len() > MAX_META_BLOCK_SIZE {
        return Err(BurliError::Format("invalid compressed Brotli block size"));
    }

    let insert = insert_length_code(input.len())?;
    let command_symbol = command_symbol_for_insert(insert.code)?;

    write_meta_block_len(writer, input.len())?;
    write_block_and_context_header(writer)?;
    let mut literal_frequencies = [0_usize; LITERAL_ALPHABET_SIZE];
    for &literal in input {
        literal_frequencies[usize::from(literal)] += 1;
    }
    let mut prefix = PrefixCodeScratch::default();
    prefix.reserve_for(LITERAL_ALPHABET_SIZE, LITERAL_ALPHABET_SIZE);
    let literal_code_map = write_fast_dense_prefix_code_array_from_frequencies_with_scratch(
        writer,
        &literal_frequencies,
        &mut prefix,
    )?;
    write_simple_prefix_code_single(writer, COMMAND_ALPHABET_SIZE, command_symbol)?;
    write_simple_prefix_code_single(writer, 64, 0)?;
    writer.write_bits_trusted(insert.extra_bits, insert.extra);
    write_literals_dense(writer, input, &literal_code_map)?;
    Ok(())
}

fn write_token_batch(
    writer: &mut BitWriter,
    input: &[u8],
    tokens: &[Token],
) -> Result<(), CompressError> {
    let block_len = tokens.iter().map(|token| token.block_len()).sum::<usize>();
    write_token_batch_with_len(writer, input, tokens, block_len)
}

fn write_token_batch_q2_with_len(
    writer: &mut BitWriter,
    input: &[u8],
    tokens: &[Token],
    block_len: usize,
) -> Result<(), CompressError> {
    if tokens.len() <= 1024 {
        return write_static_entropy_token_batch(writer, input, tokens, block_len);
    }
    write_token_batch_with_len(writer, input, tokens, block_len)
}

fn write_token_batch_q2_recomputed_with_len(
    writer: &mut BitWriter,
    input: &[u8],
    tokens: &[Token],
    block_len: usize,
    prefix: &mut PrefixCodeScratch,
) -> Result<(), CompressError> {
    if tokens.len() <= 1024 {
        return write_static_entropy_token_batch(writer, input, tokens, block_len);
    }
    write_token_batch_recomputed_with_len(writer, input, tokens, block_len, prefix)
}

fn write_token_batch_with_len(
    writer: &mut BitWriter,
    input: &[u8],
    tokens: &[Token],
    block_len: usize,
) -> Result<(), CompressError> {
    if block_len == 0 || block_len > MAX_META_BLOCK_SIZE {
        return Err(BurliError::Format("invalid compressed Brotli block size"));
    }
    let mut batch = PreparedBatch::with_capacity(tokens.len());
    for &token in tokens {
        batch.push(input, token)?;
    }
    batch.ensure_distance_frequencies();
    write_prepared_token_batch_with_len(writer, input, &batch, block_len)
}

fn write_token_batch_recomputed_with_len(
    writer: &mut BitWriter,
    input: &[u8],
    tokens: &[Token],
    block_len: usize,
    prefix: &mut PrefixCodeScratch,
) -> Result<(), CompressError> {
    if block_len == 0 || block_len > MAX_META_BLOCK_SIZE {
        return Err(BurliError::Format("invalid compressed Brotli block size"));
    }
    let mut frequencies = TokenBatchFrequencies::new();
    for &token in tokens {
        frequencies.push(input, token)?;
    }
    frequencies.ensure_distance_frequencies();

    write_meta_block_len(writer, block_len)?;
    write_block_and_context_header(writer)?;
    prefix.reserve_for(COMMAND_ALPHABET_SIZE, COMMAND_ALPHABET_SIZE);
    let literal_code_map = write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits(
        writer,
        &frequencies.literal_frequencies,
        prefix,
        MAX_CODE_BITS,
    )?;
    let command_code_map = write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits(
        writer,
        &frequencies.command_frequencies,
        prefix,
        MAX_CODE_BITS,
    )?;
    let distance_code_map = write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits(
        writer,
        &frequencies.distance_frequencies,
        prefix,
        MAX_CODE_BITS,
    )?;

    let mut pending_bits = 0_u64;
    let mut pending_width = 0_u8;
    for &token in tokens {
        let prepared_token = PreparedToken::new(token)?;
        let token = prepared_token.token;
        let insert = prepared_token.insert;
        let copy = prepared_token.copy;
        let command_code = command_code_map[usize::from(prepared_token.command_symbol)];
        debug_assert!(command_code.len != u8::MAX);
        append_pending_bits(
            writer,
            &mut pending_bits,
            &mut pending_width,
            command_code.len,
            u64::from(command_code.bits),
        );
        append_pending_bits(
            writer,
            &mut pending_bits,
            &mut pending_width,
            insert.extra_bits,
            insert.extra,
        );
        if let Some(copy) = copy {
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                copy.extra_bits,
                copy.extra,
            );
        }

        for &literal in &input[token.insert_start..token.insert_start + token.insert_len] {
            let literal_code = literal_code_map[usize::from(literal)];
            debug_assert!(literal_code.len != u8::MAX);
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                literal_code.len,
                u64::from(literal_code.bits),
            );
        }

        if let Some(distance) = prepared_token.distance {
            let distance_code = distance_code_map[usize::from(distance.symbol)];
            debug_assert!(distance_code.len != u8::MAX);
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                distance_code.len,
                u64::from(distance_code.bits),
            );
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                distance.extra_bits,
                distance.extra,
            );
        }
    }
    if pending_width != 0 {
        writer.write_bits_trusted_fits(pending_width, pending_bits);
    }

    Ok(())
}

fn write_static_entropy_token_batch(
    writer: &mut BitWriter,
    input: &[u8],
    tokens: &[Token],
    block_len: usize,
) -> Result<(), CompressError> {
    if block_len == 0 || block_len > MAX_META_BLOCK_SIZE {
        return Err(BurliError::Format("invalid compressed Brotli block size"));
    }

    let mut batch = StaticEntropyBatch::with_capacity(tokens.len());
    for &token in tokens {
        batch.push(input, token)?;
    }

    write_meta_block_len(writer, block_len)?;
    write_block_and_context_header(writer)?;
    let literal_code_map = if block_len <= 1024 {
        let mut prefix = PrefixCodeScratch::default();
        prefix.reserve_for(LITERAL_ALPHABET_SIZE, LITERAL_ALPHABET_SIZE);
        if q2_tiny_balanced_literal_prefix_is_likely_safe(input) {
            write_balanced_fast_dense_prefix_code_array_from_frequencies_with_scratch(
                writer,
                &batch.literal_frequencies,
                &mut prefix,
            )?
        } else {
            write_fast_dense_prefix_code_array_from_frequencies_with_scratch(
                writer,
                &batch.literal_frequencies,
                &mut prefix,
            )?
        }
    } else {
        let literal_codes = write_prefix_code_from_frequencies(
            writer,
            LITERAL_ALPHABET_SIZE,
            &batch.literal_frequencies,
        )?;
        dense_symbol_code_map_from_symbol_codes::<LITERAL_ALPHABET_SIZE>(&literal_codes)?
    };
    write_static_command_and_distance_prefix_codes(writer);

    let mut pending_bits = 0_u64;
    let mut pending_width = 0_u8;
    for prepared_token in &batch.prepared {
        let token = prepared_token.token;
        let insert = prepared_token.insert;
        let copy = prepared_token.copy;
        let command_code = static_command_code(prepared_token.command_symbol)?;
        append_pending_bits(
            writer,
            &mut pending_bits,
            &mut pending_width,
            command_code.len,
            u64::from(command_code.bits),
        );
        append_pending_bits(
            writer,
            &mut pending_bits,
            &mut pending_width,
            insert.extra_bits,
            insert.extra,
        );
        if let Some(copy) = copy {
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                copy.extra_bits,
                copy.extra,
            );
        }

        for &literal in &input[token.insert_start..token.insert_start + token.insert_len] {
            let literal_code = literal_code_map[usize::from(literal)];
            debug_assert!(literal_code.len != u8::MAX);
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                literal_code.len,
                u64::from(literal_code.bits),
            );
        }

        if let Some(distance) = prepared_token.distance {
            let distance_code = static_distance_code(distance.symbol)?;
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                distance_code.len,
                u64::from(distance_code.bits),
            );
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                distance.extra_bits,
                distance.extra,
            );
        }
    }
    if pending_width != 0 {
        writer.write_bits_trusted_fits(pending_width, pending_bits);
    }

    Ok(())
}

fn write_static_command_and_distance_prefix_codes(writer: &mut BitWriter) {
    writer.write_bits_trusted_fits(56, 0x0092_6244_1630_7003);
    writer.write_bits_trusted_fits(3, 0);
    write_static_distance_prefix_code(writer);
}

fn write_static_distance_prefix_code(writer: &mut BitWriter) {
    writer.write_bits_trusted_fits(28, 0x0369_dc03);
}

fn write_q1_internal_balanced_command_static_distance_prefix_codes(
    writer: &mut BitWriter,
    command_frequencies: &[usize; 128],
    scratch: &mut PrefixCodeScratch,
) -> Result<[DenseSymbolCode; 128], CompressError> {
    scratch.used.clear();
    for (symbol, &frequency) in command_frequencies[..64].iter().enumerate() {
        if frequency != 0 {
            scratch.used.push((symbol as u16, frequency));
        }
    }

    scratch.lengths.clear();
    scratch.lengths.resize(64, 0);
    match scratch.used.len() {
        0 => scratch.lengths[0] = 1,
        1 => scratch.lengths[usize::from(scratch.used[0].0)] = 1,
        _ => balanced_code_lengths_into(64, &mut scratch.used, MAX_CODE_BITS, &mut scratch.lengths),
    }
    let mut internal_command_lengths = [0_u8; 64];
    internal_command_lengths.copy_from_slice(&scratch.lengths[..64]);

    let mut full_command_lengths = [0_u8; COMMAND_ALPHABET_SIZE];
    for (code, &len) in internal_command_lengths.iter().enumerate() {
        full_command_lengths[q1_internal_command_symbol(code)] = len;
    }

    let mut internal_map = q1_internal_command_code_map_from_lengths(&internal_command_lengths);
    scratch.lengths.clear();
    scratch.lengths.extend_from_slice(&full_command_lengths);
    write_fast_complex_prefix_code_lengths_with_scratch(writer, scratch)?;
    write_static_distance_prefix_code(writer);
    for symbol in 0..64 {
        internal_map[64 + symbol] = static_distance_code(symbol as u16)?;
    }

    Ok(internal_map)
}

fn write_prepared_token_batch_with_len(
    writer: &mut BitWriter,
    input: &[u8],
    batch: &PreparedBatch,
    block_len: usize,
) -> Result<(), CompressError> {
    if block_len == 0 || block_len > MAX_META_BLOCK_SIZE {
        return Err(BurliError::Format("invalid compressed Brotli block size"));
    }

    write_meta_block_len(writer, block_len)?;
    write_block_and_context_header(writer)?;
    let mut prefix = PrefixCodeScratch::default();
    prefix.reserve_for(COMMAND_ALPHABET_SIZE, COMMAND_ALPHABET_SIZE);
    let literal_code_map = write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits(
        writer,
        &batch.literal_frequencies,
        &mut prefix,
        MAX_CODE_BITS,
    )?;
    let command_code_map = write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits(
        writer,
        &batch.command_frequencies,
        &mut prefix,
        MAX_CODE_BITS,
    )?;
    let distance_code_map = write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits(
        writer,
        &batch.distance_frequencies,
        &mut prefix,
        MAX_CODE_BITS,
    )?;

    let mut pending_bits = 0_u64;
    let mut pending_width = 0_u8;
    for prepared_token in &batch.prepared {
        let token = prepared_token.token;
        let insert = prepared_token.insert;
        let copy = prepared_token.copy;
        let command_code = command_code_map[usize::from(prepared_token.command_symbol)];
        debug_assert!(command_code.len != u8::MAX);
        append_pending_bits(
            writer,
            &mut pending_bits,
            &mut pending_width,
            command_code.len,
            u64::from(command_code.bits),
        );
        append_pending_bits(
            writer,
            &mut pending_bits,
            &mut pending_width,
            insert.extra_bits,
            insert.extra,
        );
        if let Some(copy) = copy {
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                copy.extra_bits,
                copy.extra,
            );
        }

        for &literal in &input[token.insert_start..token.insert_start + token.insert_len] {
            let literal_code = literal_code_map[usize::from(literal)];
            debug_assert!(literal_code.len != u8::MAX);
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                literal_code.len,
                u64::from(literal_code.bits),
            );
        }

        if let Some(distance) = prepared_token.distance {
            let distance_code = distance_code_map[usize::from(distance.symbol)];
            debug_assert!(distance_code.len != u8::MAX);
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                distance_code.len,
                u64::from(distance_code.bits),
            );
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                distance.extra_bits,
                distance.extra,
            );
        }
    }
    if pending_width != 0 {
        writer.write_bits_trusted_fits(pending_width, pending_bits);
    }

    Ok(())
}

fn static_command_code(symbol: u16) -> Result<DenseSymbolCode, CompressError> {
    let symbol = usize::from(symbol);
    if symbol < 448 {
        return Ok(DenseSymbolCode {
            len: 9,
            bits: reverse_bits_u16(symbol as u16, 9),
        });
    }
    if symbol < COMMAND_ALPHABET_SIZE {
        return Ok(DenseSymbolCode {
            len: 11,
            bits: reverse_bits_u16((1792 + symbol - 448) as u16, 11),
        });
    }
    Err(BurliError::Format("Brotli command symbol exceeds alphabet"))
}

fn static_distance_code(symbol: u16) -> Result<DenseSymbolCode, CompressError> {
    if usize::from(symbol) >= 64 {
        return Err(BurliError::Format(
            "Brotli distance symbol exceeds alphabet",
        ));
    }
    Ok(DenseSymbolCode {
        len: 6,
        bits: reverse_bits_u16(symbol, 6),
    })
}

#[derive(Clone, Copy, Debug)]
struct PreparedToken {
    token: Token,
    insert: InsertLengthCode,
    copy: Option<CopyLengthCode>,
    command_symbol: u16,
    distance: Option<DistanceCode>,
}

impl PreparedToken {
    #[inline(always)]
    fn new(token: Token) -> Result<Self, CompressError> {
        let insert = insert_length_code(token.insert_len)?;
        let copy = if token.is_copy() {
            Some(copy_length_code(token.copy_len_code())?)
        } else {
            None
        };
        let command_symbol = if let Some(copy) = copy {
            command_symbol_for_insert_copy(insert.code, copy.code, token.use_last_distance)?
        } else {
            command_symbol_for_insert(insert.code)?
        };
        let distance = if token.is_copy() && !token.use_last_distance {
            if let Some(distance_code) = token.distance_code {
                Some(DistanceCode {
                    symbol: distance_code,
                    extra_bits: 0,
                    extra: 0,
                })
            } else {
                Some(distance_code(token.distance)?)
            }
        } else {
            None
        };

        Ok(Self {
            token,
            insert,
            copy,
            command_symbol,
            distance,
        })
    }
}

fn write_block_and_context_header(writer: &mut BitWriter) -> Result<(), CompressError> {
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)?;
    writer.write_bits(2, 0)?;
    writer.write_bits(4, 0)?;
    writer.write_bits(2, 0)?;
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)
}

fn write_meta_block_len(writer: &mut BitWriter, len: usize) -> Result<(), CompressError> {
    if len == 0 || len > MAX_META_BLOCK_SIZE {
        return Err(BurliError::Format("invalid compressed Brotli block size"));
    }

    let len_minus_one = len - 1;
    let significant_bits = if len == 1 {
        1
    } else {
        usize::BITS - len_minus_one.leading_zeros()
    };
    let nibbles = if significant_bits < 16 {
        4
    } else {
        significant_bits.div_ceil(4) as usize
    };
    debug_assert!((4..=6).contains(&nibbles));

    writer.write_bits(1, 0)?;
    writer.write_bits(2, (nibbles - 4) as u64)?;
    writer.write_bits((nibbles * 4) as u8, len_minus_one as u64)?;
    writer.write_bits(1, 0)
}

fn write_var_len_u8(writer: &mut BitWriter, value: usize) -> Result<(), CompressError> {
    if value == 0 {
        return writer.write_bits(1, 0);
    }
    if value == 1 {
        writer.write_bits(1, 1)?;
        return writer.write_bits(3, 0);
    }

    let width = usize::BITS - (value - 1).leading_zeros();
    if width > 8 {
        return Err(BurliError::Format("Brotli varlen u8 value exceeds range"));
    }
    writer.write_bits(1, 1)?;
    writer.write_bits(3, u64::from(width))?;
    writer.write_bits(width as u8, (value - (1_usize << width)) as u64)
}

fn write_code_length_code_len(writer: &mut BitWriter, len: u8) -> Result<(), CompressError> {
    let (width, bits) = match len {
        0 => (2, 0),
        1 => (4, 7),
        2 => (3, 3),
        3 => (2, 2),
        4 => (2, 1),
        5 => (4, 15),
        _ => return Err(BurliError::Format("unsupported Brotli code length code")),
    };
    writer.write_bits_trusted_fits(width, bits);
    Ok(())
}

fn write_prefix_code_from_frequencies(
    writer: &mut BitWriter,
    alphabet_size: usize,
    frequencies: &[usize],
) -> Result<Vec<SymbolCode>, CompressError> {
    write_prefix_code_from_frequencies_with_max_bits(
        writer,
        alphabet_size,
        frequencies,
        MAX_CODE_BITS,
    )
}

fn write_prefix_code_from_frequencies_with_max_bits(
    writer: &mut BitWriter,
    alphabet_size: usize,
    frequencies: &[usize],
    max_bits: u8,
) -> Result<Vec<SymbolCode>, CompressError> {
    if frequencies.len() != alphabet_size {
        return Err(BurliError::Format("Brotli prefix alphabet size mismatch"));
    }

    let mut used = frequencies
        .iter()
        .enumerate()
        .filter_map(|(symbol, &frequency)| (frequency != 0).then_some((symbol as u16, frequency)))
        .collect::<Vec<_>>();
    if used.is_empty() {
        return write_simple_prefix_code_symbols(writer, alphabet_size, &[0]);
    }
    if used.len() <= MAX_SIMPLE_PREFIX_SYMBOLS {
        let symbols = used.iter().map(|&(symbol, _)| symbol).collect::<Vec<_>>();
        return write_simple_prefix_code_symbols(writer, alphabet_size, &symbols);
    }

    let lengths = huffman_code_lengths(frequencies, max_bits)
        .unwrap_or_else(|| balanced_code_lengths(alphabet_size, &mut used, max_bits));

    write_complex_prefix_code_lengths(writer, &lengths)?;
    Ok(symbol_codes_from_lengths(&lengths))
}

fn write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits<const N: usize>(
    writer: &mut BitWriter,
    frequencies: &[usize; N],
    scratch: &mut PrefixCodeScratch,
    max_bits: u8,
) -> Result<[DenseSymbolCode; N], CompressError> {
    scratch.used.clear();
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency != 0 {
            scratch.used.push((symbol as u16, frequency));
        }
    }

    let mut map = [MISSING_DENSE_SYMBOL_CODE; N];
    if scratch.used.is_empty() {
        write_simple_dense_prefix_code(writer, &[0], &mut map)?;
        return Ok(map);
    }
    if scratch.used.len() <= MAX_SIMPLE_PREFIX_SYMBOLS {
        let mut symbols = [0_u16; MAX_SIMPLE_PREFIX_SYMBOLS];
        for (index, &(symbol, _)) in scratch.used.iter().enumerate() {
            symbols[index] = symbol;
        }
        write_simple_dense_prefix_code(writer, &symbols[..scratch.used.len()], &mut map)?;
        return Ok(map);
    }

    code_lengths_from_current_used_with_scratch(N, max_bits, scratch);

    fill_dense_symbol_code_map_from_lengths(&scratch.lengths, &mut map);
    write_complex_prefix_code_lengths_with_scratch(writer, scratch)?;
    Ok(map)
}

fn write_fast_dense_prefix_code_array_from_frequencies_with_scratch<const N: usize>(
    writer: &mut BitWriter,
    frequencies: &[usize; N],
    scratch: &mut PrefixCodeScratch,
) -> Result<[DenseSymbolCode; N], CompressError> {
    scratch.used.clear();
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency != 0 {
            scratch.used.push((symbol as u16, frequency));
        }
    }

    let mut map = [MISSING_DENSE_SYMBOL_CODE; N];
    if scratch.used.is_empty() {
        write_simple_dense_prefix_code(writer, &[0], &mut map)?;
        return Ok(map);
    }
    if scratch.used.len() <= MAX_SIMPLE_PREFIX_SYMBOLS {
        let mut symbols = [0_u16; MAX_SIMPLE_PREFIX_SYMBOLS];
        for (index, &(symbol, _)) in scratch.used.iter().enumerate() {
            symbols[index] = symbol;
        }
        write_simple_dense_prefix_code(writer, &symbols[..scratch.used.len()], &mut map)?;
        return Ok(map);
    }

    code_lengths_from_current_used_with_scratch(N, FAST_CODE_BITS, scratch);

    fill_dense_symbol_code_map_from_lengths(&scratch.lengths, &mut map);
    write_fast_complex_prefix_code_lengths_with_scratch(writer, scratch)?;
    Ok(map)
}

fn write_balanced_fast_dense_prefix_code_array_from_frequencies_with_scratch<const N: usize>(
    writer: &mut BitWriter,
    frequencies: &[usize; N],
    scratch: &mut PrefixCodeScratch,
) -> Result<[DenseSymbolCode; N], CompressError> {
    scratch.used.clear();
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency != 0 {
            scratch.used.push((symbol as u16, frequency));
        }
    }

    let mut map = [MISSING_DENSE_SYMBOL_CODE; N];
    if scratch.used.is_empty() {
        write_simple_dense_prefix_code(writer, &[0], &mut map)?;
        return Ok(map);
    }
    if scratch.used.len() <= MAX_SIMPLE_PREFIX_SYMBOLS {
        let mut symbols = [0_u16; MAX_SIMPLE_PREFIX_SYMBOLS];
        for (index, &(symbol, _)) in scratch.used.iter().enumerate() {
            symbols[index] = symbol;
        }
        write_simple_dense_prefix_code(writer, &symbols[..scratch.used.len()], &mut map)?;
        return Ok(map);
    }

    scratch.lengths.clear();
    scratch.lengths.resize(N, 0);
    balanced_code_lengths_into(N, &mut scratch.used, FAST_CODE_BITS, &mut scratch.lengths);

    fill_dense_symbol_code_map_from_lengths(&scratch.lengths, &mut map);
    write_fast_complex_prefix_code_lengths_with_scratch(writer, scratch)?;
    Ok(map)
}

fn code_lengths_from_current_used_with_scratch(
    alphabet_size: usize,
    max_bits: u8,
    scratch: &mut PrefixCodeScratch,
) {
    scratch.lengths.clear();
    scratch.lengths.resize(alphabet_size, 0);
    if scratch.used.is_empty() {
        scratch.lengths[0] = 1;
        return;
    }
    if scratch.used.len() == 1 {
        scratch.lengths[usize::from(scratch.used[0].0)] = 1;
        return;
    }

    if huffman_code_lengths_from_current_used_with_scratch(max_bits, scratch) {
        return;
    }

    scratch.lengths.clear();
    scratch.lengths.resize(alphabet_size, 0);
    balanced_code_lengths_into(
        alphabet_size,
        &mut scratch.used,
        max_bits,
        &mut scratch.lengths,
    );
}

fn code_lengths_from_dense_frequencies_with_scratch<const N: usize>(
    frequencies: &[usize; N],
    max_bits: u8,
    scratch: &mut PrefixCodeScratch,
) {
    scratch.used.clear();
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency != 0 {
            scratch.used.push((symbol as u16, frequency));
        }
    }
    code_lengths_from_current_used_with_scratch(N, max_bits, scratch);
}

fn code_lengths_from_frequencies(
    alphabet_size: usize,
    frequencies: &[usize],
    max_bits: u8,
) -> Result<Vec<u8>, CompressError> {
    if frequencies.len() != alphabet_size {
        return Err(BurliError::Format("Brotli prefix alphabet size mismatch"));
    }

    let mut used = frequencies
        .iter()
        .enumerate()
        .filter_map(|(symbol, &frequency)| (frequency != 0).then_some((symbol as u16, frequency)))
        .collect::<Vec<_>>();
    if used.is_empty() {
        let mut lengths = vec![0_u8; alphabet_size];
        lengths[0] = 1;
        return Ok(lengths);
    }
    if used.len() == 1 {
        let mut lengths = vec![0_u8; alphabet_size];
        lengths[usize::from(used[0].0)] = 1;
        return Ok(lengths);
    }

    Ok(huffman_code_lengths(frequencies, max_bits)
        .unwrap_or_else(|| balanced_code_lengths(alphabet_size, &mut used, max_bits)))
}

fn balanced_code_lengths(alphabet_size: usize, used: &mut [(u16, usize)], max_bits: u8) -> Vec<u8> {
    used.sort_unstable_by(
        |&(left_symbol, left_frequency), &(right_symbol, right_frequency)| {
            right_frequency
                .cmp(&left_frequency)
                .then_with(|| left_symbol.cmp(&right_symbol))
        },
    );

    let mut lengths = vec![0_u8; alphabet_size];
    let base_bits = ceil_log2(used.len()).unwrap().min(max_bits);
    let short_count = (1_usize << base_bits) - used.len();
    for (rank, &(symbol, _)) in used.iter().enumerate() {
        lengths[usize::from(symbol)] = if rank < short_count {
            base_bits - 1
        } else {
            base_bits
        };
    }
    lengths
}

fn balanced_code_lengths_into(
    alphabet_size: usize,
    used: &mut [(u16, usize)],
    max_bits: u8,
    lengths: &mut [u8],
) {
    debug_assert_eq!(lengths.len(), alphabet_size);
    used.sort_unstable_by(
        |&(left_symbol, left_frequency), &(right_symbol, right_frequency)| {
            right_frequency
                .cmp(&left_frequency)
                .then_with(|| left_symbol.cmp(&right_symbol))
        },
    );

    lengths.fill(0);
    let base_bits = ceil_log2(used.len()).unwrap().min(max_bits);
    let short_count = (1_usize << base_bits) - used.len();
    for (rank, &(symbol, _)) in used.iter().enumerate() {
        lengths[usize::from(symbol)] = if rank < short_count {
            base_bits - 1
        } else {
            base_bits
        };
    }
}

#[derive(Clone, Debug)]
struct HuffmanNode {
    frequency: u64,
    min_symbol: u16,
    parent: Option<usize>,
}

fn huffman_code_lengths(frequencies: &[usize], max_bits: u8) -> Option<Vec<u8>> {
    let mut nodes = Vec::new();
    let mut leaves = Vec::new();

    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        let index = nodes.len();
        nodes.push(HuffmanNode {
            frequency: frequency as u64,
            min_symbol: symbol as u16,
            parent: None,
        });
        leaves.push((symbol, index));
    }

    if leaves.len() <= 1 {
        return None;
    }

    leaves.sort_unstable_by(|&(_, left), &(_, right)| compare_huffman_nodes(&nodes, left, right));
    let mut leaf_head = 0;
    let mut parent_queue = Vec::with_capacity(leaves.len() - 1);
    let mut parent_head = 0;
    let mut remaining = leaves.len();

    while remaining > 1 {
        let first = pop_huffman_queue(
            &nodes,
            &leaves,
            &mut leaf_head,
            &parent_queue,
            &mut parent_head,
        )?;
        let second = pop_huffman_queue(
            &nodes,
            &leaves,
            &mut leaf_head,
            &parent_queue,
            &mut parent_head,
        )?;
        let parent = nodes.len();
        nodes.push(HuffmanNode {
            frequency: nodes[first].frequency + nodes[second].frequency,
            min_symbol: nodes[first].min_symbol.min(nodes[second].min_symbol),
            parent: None,
        });
        nodes[first].parent = Some(parent);
        nodes[second].parent = Some(parent);
        parent_queue.push(parent);
        remaining -= 1;
    }

    let mut lengths = vec![0_u8; frequencies.len()];
    for (symbol, node_index) in leaves {
        let mut depth = 0_u8;
        let mut cursor = node_index;
        while let Some(parent) = nodes[cursor].parent {
            depth = depth.checked_add(1)?;
            cursor = parent;
        }
        if depth == 0 || depth > max_bits {
            return None;
        }
        lengths[symbol] = depth;
    }

    Some(lengths)
}

fn huffman_code_lengths_with_scratch(
    frequencies: &[usize],
    max_bits: u8,
    scratch: &mut PrefixCodeScratch,
) -> bool {
    scratch.nodes.clear();
    scratch.leaves.clear();
    scratch.parent_queue.clear();
    scratch.lengths.clear();
    scratch.lengths.resize(frequencies.len(), 0);

    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        let index = scratch.nodes.len();
        scratch.nodes.push(HuffmanNode {
            frequency: frequency as u64,
            min_symbol: symbol as u16,
            parent: None,
        });
        scratch.leaves.push((symbol, index));
    }

    if scratch.leaves.len() <= 1 {
        return false;
    }

    {
        let nodes = &scratch.nodes;
        scratch
            .leaves
            .sort_unstable_by(|&(_, left), &(_, right)| compare_huffman_nodes(nodes, left, right));
    }

    let mut leaf_head = 0;
    let mut parent_head = 0;
    let mut remaining = scratch.leaves.len();

    while remaining > 1 {
        let Some(first) = pop_huffman_queue(
            &scratch.nodes,
            &scratch.leaves,
            &mut leaf_head,
            &scratch.parent_queue,
            &mut parent_head,
        ) else {
            return false;
        };
        let Some(second) = pop_huffman_queue(
            &scratch.nodes,
            &scratch.leaves,
            &mut leaf_head,
            &scratch.parent_queue,
            &mut parent_head,
        ) else {
            return false;
        };
        let parent = scratch.nodes.len();
        scratch.nodes.push(HuffmanNode {
            frequency: scratch.nodes[first].frequency + scratch.nodes[second].frequency,
            min_symbol: scratch.nodes[first]
                .min_symbol
                .min(scratch.nodes[second].min_symbol),
            parent: None,
        });
        scratch.nodes[first].parent = Some(parent);
        scratch.nodes[second].parent = Some(parent);
        scratch.parent_queue.push(parent);
        remaining -= 1;
    }

    for &(symbol, node_index) in &scratch.leaves {
        let mut depth = 0_u8;
        let mut cursor = node_index;
        while let Some(parent) = scratch.nodes[cursor].parent {
            let Some(next_depth) = depth.checked_add(1) else {
                return false;
            };
            depth = next_depth;
            cursor = parent;
        }
        if depth == 0 || depth > max_bits {
            return false;
        }
        scratch.lengths[symbol] = depth;
    }

    true
}

fn huffman_code_lengths_from_current_used_with_scratch(
    max_bits: u8,
    scratch: &mut PrefixCodeScratch,
) -> bool {
    scratch.nodes.clear();
    scratch.leaves.clear();
    scratch.parent_queue.clear();

    for &(symbol, frequency) in &scratch.used {
        let index = scratch.nodes.len();
        scratch.nodes.push(HuffmanNode {
            frequency: frequency as u64,
            min_symbol: symbol,
            parent: None,
        });
        scratch.leaves.push((usize::from(symbol), index));
    }

    if scratch.leaves.len() <= 1 {
        return false;
    }

    {
        let nodes = &scratch.nodes;
        scratch
            .leaves
            .sort_unstable_by(|&(_, left), &(_, right)| compare_huffman_nodes(nodes, left, right));
    }

    let mut leaf_head = 0;
    let mut parent_head = 0;
    let mut remaining = scratch.leaves.len();

    while remaining > 1 {
        let Some(first) = pop_huffman_queue(
            &scratch.nodes,
            &scratch.leaves,
            &mut leaf_head,
            &scratch.parent_queue,
            &mut parent_head,
        ) else {
            return false;
        };
        let Some(second) = pop_huffman_queue(
            &scratch.nodes,
            &scratch.leaves,
            &mut leaf_head,
            &scratch.parent_queue,
            &mut parent_head,
        ) else {
            return false;
        };
        let parent = scratch.nodes.len();
        scratch.nodes.push(HuffmanNode {
            frequency: scratch.nodes[first].frequency + scratch.nodes[second].frequency,
            min_symbol: scratch.nodes[first]
                .min_symbol
                .min(scratch.nodes[second].min_symbol),
            parent: None,
        });
        scratch.nodes[first].parent = Some(parent);
        scratch.nodes[second].parent = Some(parent);
        scratch.parent_queue.push(parent);
        remaining -= 1;
    }

    for &(symbol, node_index) in &scratch.leaves {
        let mut depth = 0_u8;
        let mut cursor = node_index;
        while let Some(parent) = scratch.nodes[cursor].parent {
            let Some(next_depth) = depth.checked_add(1) else {
                return false;
            };
            depth = next_depth;
            cursor = parent;
        }
        if depth == 0 || depth > max_bits {
            return false;
        }
        scratch.lengths[symbol] = depth;
    }

    true
}

fn compare_huffman_nodes(nodes: &[HuffmanNode], left: usize, right: usize) -> core::cmp::Ordering {
    nodes[left]
        .frequency
        .cmp(&nodes[right].frequency)
        .then_with(|| nodes[left].min_symbol.cmp(&nodes[right].min_symbol))
}

fn pop_huffman_queue(
    nodes: &[HuffmanNode],
    leaf_queue: &[(usize, usize)],
    leaf_head: &mut usize,
    parent_queue: &[usize],
    parent_head: &mut usize,
) -> Option<usize> {
    let leaf = leaf_queue.get(*leaf_head).map(|&(_, index)| index);
    let parent = parent_queue.get(*parent_head).copied();

    match (leaf, parent) {
        (Some(leaf), Some(parent)) => {
            if compare_huffman_nodes(nodes, leaf, parent).is_le() {
                *leaf_head += 1;
                Some(leaf)
            } else {
                *parent_head += 1;
                Some(parent)
            }
        }
        (Some(leaf), None) => {
            *leaf_head += 1;
            Some(leaf)
        }
        (None, Some(parent)) => {
            *parent_head += 1;
            Some(parent)
        }
        (None, None) => None,
    }
}

fn write_complex_prefix_code_lengths(
    writer: &mut BitWriter,
    lengths: &[u8],
) -> Result<(), CompressError> {
    let tree = encode_code_length_tree(lengths)?;
    let mut tree_frequencies = [0_usize; CODE_LENGTH_ALPHABET_SIZE];
    for &entry in &tree {
        tree_frequencies[usize::from(code_length_tree_symbol(entry))] += 1;
    }

    let code_length_lengths =
        code_lengths_from_frequencies(CODE_LENGTH_ALPHABET_SIZE, &tree_frequencies, 5)?;
    write_code_length_code_lengths(writer, &code_length_lengths, &tree_frequencies)?;

    let single_code = single_non_zero_symbol(&tree_frequencies);
    let mut code_length_code_map = [MISSING_DENSE_SYMBOL_CODE; CODE_LENGTH_ALPHABET_SIZE];
    fill_dense_symbol_code_map_from_lengths(&code_length_lengths, &mut code_length_code_map);
    for &entry in &tree {
        let symbol = code_length_tree_symbol(entry);
        let extra_bits = code_length_tree_extra_bits(entry);
        let mut width = 0;
        let mut bits = 0;
        if single_code != Some(usize::from(symbol)) {
            let code = code_length_code_map[usize::from(symbol)];
            debug_assert!(code.len != u8::MAX);
            width = code.len;
            bits = u64::from(code.bits);
        }
        match symbol {
            16 => {
                bits |= u64::from(extra_bits) << width;
                width += 2;
            }
            17 => {
                bits |= u64::from(extra_bits) << width;
                width += 3;
            }
            _ => {}
        }
        writer.write_bits_trusted_fits(width, bits);
    }
    Ok(())
}

fn write_fast_complex_prefix_code_lengths_with_scratch(
    writer: &mut BitWriter,
    scratch: &mut PrefixCodeScratch,
) -> Result<(), CompressError> {
    encode_fast_code_length_tree_into(&scratch.lengths, &mut scratch.tree)?;
    writer.write_bits_trusted_fits(40, 0x00ff_5555_5554);
    let mut pending_bits = 0_u64;
    let mut pending_width = 0_u8;
    for &entry in &scratch.tree {
        let symbol = code_length_tree_symbol(entry);
        let extra_bits = code_length_tree_extra_bits(entry);
        match symbol {
            0..=14 => {
                let len = STATIC_CODE_LENGTH_DEPTH[usize::from(symbol)];
                let bits = STATIC_CODE_LENGTH_BITS[usize::from(symbol)];
                append_pending_bits(
                    writer,
                    &mut pending_bits,
                    &mut pending_width,
                    len,
                    u64::from(bits),
                );
            }
            16 => {
                let len = STATIC_CODE_LENGTH_DEPTH[16];
                let bits = STATIC_CODE_LENGTH_BITS[16] | (u16::from(extra_bits) << len);
                append_pending_bits(
                    writer,
                    &mut pending_bits,
                    &mut pending_width,
                    len + 2,
                    u64::from(bits),
                );
            }
            17 => {
                let len = STATIC_CODE_LENGTH_DEPTH[17];
                let bits = STATIC_CODE_LENGTH_BITS[17] | (u16::from(extra_bits) << len);
                append_pending_bits(
                    writer,
                    &mut pending_bits,
                    &mut pending_width,
                    len + 3,
                    u64::from(bits),
                );
            }
            _ => return Err(BurliError::Format("invalid Brotli code length symbol")),
        }
    }
    if pending_width != 0 {
        writer.write_bits_trusted_fits(pending_width, pending_bits);
    }
    Ok(())
}

fn encode_fast_code_length_tree_into(
    lengths: &[u8],
    tree: &mut Vec<u16>,
) -> Result<(), CompressError> {
    let trimmed_len = lengths
        .iter()
        .rposition(|&len| len != 0)
        .map_or(0, |index| index + 1);
    if trimmed_len == 0 {
        return Err(BurliError::Format("empty Brotli Huffman code"));
    }

    tree.clear();
    tree.reserve(trimmed_len);
    let mut previous_value = 8_u8;
    let mut index = 0;
    while index < trimmed_len {
        let value = lengths[index];
        if value > FAST_CODE_BITS {
            return Err(BurliError::Format("Brotli Huffman code length exceeds 14"));
        }
        let mut repetitions = 1;
        while index + repetitions < trimmed_len && lengths[index + repetitions] == value {
            repetitions += 1;
        }
        if value == 0 {
            push_zero_code_length_repetitions(repetitions, tree);
        } else {
            push_code_length_repetitions(previous_value, value, repetitions, tree);
            previous_value = value;
        }
        index += repetitions;
    }
    Ok(())
}

fn write_complex_prefix_code_lengths_with_scratch(
    writer: &mut BitWriter,
    scratch: &mut PrefixCodeScratch,
) -> Result<(), CompressError> {
    encode_code_length_tree_into(&scratch.lengths, &mut scratch.tree)?;
    write_encoded_code_length_tree(writer, scratch)
}

fn write_complex_prefix_code_lengths_from_lengths_with_scratch(
    writer: &mut BitWriter,
    lengths: &[u8],
    scratch: &mut PrefixCodeScratch,
) -> Result<(), CompressError> {
    encode_code_length_tree_into(lengths, &mut scratch.tree)?;
    write_encoded_code_length_tree(writer, scratch)
}

fn write_encoded_code_length_tree(
    writer: &mut BitWriter,
    scratch: &mut PrefixCodeScratch,
) -> Result<(), CompressError> {
    let mut tree_frequencies = [0_usize; CODE_LENGTH_ALPHABET_SIZE];
    for &entry in &scratch.tree {
        tree_frequencies[usize::from(code_length_tree_symbol(entry))] += 1;
    }

    code_lengths_from_frequencies_with_scratch(
        CODE_LENGTH_ALPHABET_SIZE,
        &tree_frequencies,
        5,
        scratch,
    )?;
    write_code_length_code_lengths(writer, &scratch.lengths, &tree_frequencies)?;

    let single_code = single_non_zero_symbol(&tree_frequencies);
    let mut code_length_code_map = [MISSING_DENSE_SYMBOL_CODE; CODE_LENGTH_ALPHABET_SIZE];
    fill_dense_symbol_code_map_from_lengths(&scratch.lengths, &mut code_length_code_map);
    for &entry in &scratch.tree {
        let symbol = code_length_tree_symbol(entry);
        let extra_bits = code_length_tree_extra_bits(entry);
        let mut width = 0;
        let mut bits = 0;
        if single_code != Some(usize::from(symbol)) {
            let code = code_length_code_map[usize::from(symbol)];
            debug_assert!(code.len != u8::MAX);
            width = code.len;
            bits = u64::from(code.bits);
        }
        match symbol {
            16 => {
                bits |= u64::from(extra_bits) << width;
                width += 2;
            }
            17 => {
                bits |= u64::from(extra_bits) << width;
                width += 3;
            }
            _ => {}
        }
        writer.write_bits_trusted_fits(width, bits);
    }
    Ok(())
}

fn write_q1_internal_command_prefix_codes(
    writer: &mut BitWriter,
    command_frequencies: &[usize; 128],
    scratch: &mut PrefixCodeScratch,
) -> Result<[DenseSymbolCode; 128], CompressError> {
    let mut internal_command_frequencies = [0_usize; 64];
    internal_command_frequencies.copy_from_slice(&command_frequencies[..64]);
    code_lengths_from_dense_frequencies_with_scratch(
        &internal_command_frequencies,
        MAX_CODE_BITS,
        scratch,
    );
    let mut internal_command_lengths = [0_u8; 64];
    internal_command_lengths.copy_from_slice(&scratch.lengths[..64]);

    let mut full_command_lengths = [0_u8; COMMAND_ALPHABET_SIZE];
    for (code, &len) in internal_command_lengths.iter().enumerate() {
        full_command_lengths[q1_internal_command_symbol(code)] = len;
    }

    let mut internal_map = q1_internal_command_code_map_from_lengths(&internal_command_lengths);
    write_complex_prefix_code_lengths_from_lengths_with_scratch(
        writer,
        &full_command_lengths,
        scratch,
    )?;

    let mut distance_frequencies = [0_usize; 64];
    distance_frequencies.copy_from_slice(&command_frequencies[64..]);
    let distance_map = write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits(
        writer,
        &distance_frequencies,
        scratch,
        14,
    )?;

    internal_map[64..].copy_from_slice(&distance_map);

    Ok(internal_map)
}

fn write_q1_internal_fast_command_prefix_codes(
    writer: &mut BitWriter,
    command_frequencies: &[usize; 128],
    scratch: &mut PrefixCodeScratch,
) -> Result<[DenseSymbolCode; 128], CompressError> {
    let mut internal_command_frequencies = [0_usize; 64];
    internal_command_frequencies.copy_from_slice(&command_frequencies[..64]);
    code_lengths_from_dense_frequencies_with_scratch(
        &internal_command_frequencies,
        MAX_CODE_BITS,
        scratch,
    );
    let mut internal_command_lengths = [0_u8; 64];
    internal_command_lengths.copy_from_slice(&scratch.lengths[..64]);

    let mut full_command_lengths = [0_u8; COMMAND_ALPHABET_SIZE];
    for (code, &len) in internal_command_lengths.iter().enumerate() {
        full_command_lengths[q1_internal_command_symbol(code)] = len;
    }

    let mut internal_map = q1_internal_command_code_map_from_lengths(&internal_command_lengths);
    scratch.lengths.clear();
    scratch.lengths.extend_from_slice(&full_command_lengths);
    write_fast_complex_prefix_code_lengths_with_scratch(writer, scratch)?;

    let mut distance_frequencies = [0_usize; 64];
    distance_frequencies.copy_from_slice(&command_frequencies[64..]);
    let distance_map = write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits(
        writer,
        &distance_frequencies,
        scratch,
        14,
    )?;

    internal_map[64..].copy_from_slice(&distance_map);

    Ok(internal_map)
}

fn q1_internal_command_code_map_from_lengths(
    internal_command_lengths: &[u8; 64],
) -> [DenseSymbolCode; 128] {
    let mut remapped_lengths = [0_u8; 64];
    remapped_lengths[..24].copy_from_slice(&internal_command_lengths[24..48]);
    remapped_lengths[24..32].copy_from_slice(&internal_command_lengths[..8]);
    remapped_lengths[32..40].copy_from_slice(&internal_command_lengths[48..56]);
    remapped_lengths[40..48].copy_from_slice(&internal_command_lengths[8..16]);
    remapped_lengths[48..56].copy_from_slice(&internal_command_lengths[56..64]);
    remapped_lengths[56..64].copy_from_slice(&internal_command_lengths[16..24]);

    let mut remapped_map = [MISSING_DENSE_SYMBOL_CODE; 64];
    fill_dense_symbol_code_map_from_lengths(&remapped_lengths, &mut remapped_map);

    let mut internal_map = [MISSING_DENSE_SYMBOL_CODE; 128];
    internal_map[..8].copy_from_slice(&remapped_map[24..32]);
    internal_map[8..16].copy_from_slice(&remapped_map[40..48]);
    internal_map[16..24].copy_from_slice(&remapped_map[56..64]);
    internal_map[24..48].copy_from_slice(&remapped_map[..24]);
    internal_map[48..56].copy_from_slice(&remapped_map[32..40]);
    internal_map[56..64].copy_from_slice(&remapped_map[48..56]);
    internal_map
}

fn q1_internal_command_symbol(code: usize) -> usize {
    match code {
        0..=7 => 128 + code * 8,
        8..=15 => 256 + (code - 8) * 8,
        16..=23 => 448 + (code - 16) * 8,
        24..=31 => code - 24,
        32..=39 => 64 + (code - 32),
        40..=47 => 128 + (code - 40),
        48..=55 => 192 + (code - 48),
        56..=63 => 384 + (code - 56),
        _ => unreachable!(),
    }
}

fn code_lengths_from_frequencies_with_scratch(
    alphabet_size: usize,
    frequencies: &[usize],
    max_bits: u8,
    scratch: &mut PrefixCodeScratch,
) -> Result<(), CompressError> {
    if frequencies.len() != alphabet_size {
        return Err(BurliError::Format("Brotli prefix alphabet size mismatch"));
    }

    scratch.used.clear();
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency != 0 {
            scratch.used.push((symbol as u16, frequency));
        }
    }

    scratch.lengths.clear();
    scratch.lengths.resize(alphabet_size, 0);
    if scratch.used.is_empty() {
        scratch.lengths[0] = 1;
        return Ok(());
    }
    if scratch.used.len() == 1 {
        scratch.lengths[usize::from(scratch.used[0].0)] = 1;
        return Ok(());
    }

    if huffman_code_lengths_with_scratch(frequencies, max_bits, scratch) {
        return Ok(());
    }

    scratch.lengths.clear();
    scratch.lengths.resize(alphabet_size, 0);
    balanced_code_lengths_into(
        alphabet_size,
        &mut scratch.used,
        max_bits,
        &mut scratch.lengths,
    );
    Ok(())
}

fn encode_code_length_tree(lengths: &[u8]) -> Result<Vec<u16>, CompressError> {
    let mut tree = Vec::new();
    encode_code_length_tree_into(lengths, &mut tree)?;
    Ok(tree)
}

fn encode_code_length_tree_into(lengths: &[u8], tree: &mut Vec<u16>) -> Result<(), CompressError> {
    let trimmed_len = lengths
        .iter()
        .rposition(|&len| len != 0)
        .map_or(0, |index| index + 1);
    if trimmed_len == 0 {
        return Err(BurliError::Format("empty Brotli Huffman code"));
    }

    let (use_rle_for_non_zero, use_rle_for_zero) = if trimmed_len > 50 {
        choose_code_length_rle(lengths, trimmed_len)
    } else {
        (false, false)
    };
    tree.clear();
    tree.reserve(trimmed_len);
    let mut previous_value = 8_u8;
    let mut index = 0;

    while index < trimmed_len {
        let value = lengths[index];
        if value > MAX_CODE_BITS {
            return Err(BurliError::Format("Brotli Huffman code length exceeds 15"));
        }
        let mut reps = 1;
        if (value == 0 && use_rle_for_zero) || (value != 0 && use_rle_for_non_zero) {
            while index + reps < trimmed_len && lengths[index + reps] == value {
                reps += 1;
            }
        }
        if value == 0 {
            push_zero_code_length_repetitions(reps, tree);
        } else {
            push_code_length_repetitions(previous_value, value, reps, tree);
            previous_value = value;
        }
        index += reps;
    }

    Ok(())
}

fn choose_code_length_rle(lengths: &[u8], len: usize) -> (bool, bool) {
    let mut total_reps_zero = 0_usize;
    let mut total_reps_non_zero = 0_usize;
    let mut count_reps_zero = 1_usize;
    let mut count_reps_non_zero = 1_usize;
    let mut index = 0;

    while index < len {
        let value = lengths[index];
        let mut reps = 1;
        while index + reps < len && lengths[index + reps] == value {
            reps += 1;
        }
        if reps >= 3 && value == 0 {
            total_reps_zero += reps;
            count_reps_zero += 1;
        }
        if reps >= 4 && value != 0 {
            total_reps_non_zero += reps;
            count_reps_non_zero += 1;
        }
        index += reps;
    }

    (
        total_reps_non_zero > count_reps_non_zero * 2,
        total_reps_zero > count_reps_zero * 2,
    )
}

fn push_code_length_repetitions(
    previous_value: u8,
    value: u8,
    mut repetitions: usize,
    tree: &mut Vec<u16>,
) {
    if previous_value != value {
        tree.push(pack_code_length_tree_entry(value, 0));
        repetitions -= 1;
    }
    if repetitions == 7 {
        tree.push(pack_code_length_tree_entry(value, 0));
        repetitions -= 1;
    }
    if repetitions < 3 {
        for _ in 0..repetitions {
            tree.push(pack_code_length_tree_entry(value, 0));
        }
        return;
    }

    let start = tree.len();
    repetitions -= 3;
    loop {
        tree.push(pack_code_length_tree_entry(16, (repetitions & 0x03) as u8));
        repetitions >>= 2;
        if repetitions == 0 {
            break;
        }
        repetitions -= 1;
    }
    tree[start..].reverse();
}

fn push_zero_code_length_repetitions(mut repetitions: usize, tree: &mut Vec<u16>) {
    if repetitions == 11 {
        tree.push(pack_code_length_tree_entry(0, 0));
        repetitions -= 1;
    }
    if repetitions < 3 {
        for _ in 0..repetitions {
            tree.push(pack_code_length_tree_entry(0, 0));
        }
        return;
    }

    let start = tree.len();
    repetitions -= 3;
    loop {
        tree.push(pack_code_length_tree_entry(17, (repetitions & 0x07) as u8));
        repetitions >>= 3;
        if repetitions == 0 {
            break;
        }
        repetitions -= 1;
    }
    tree[start..].reverse();
}

fn pack_code_length_tree_entry(symbol: u8, extra_bits: u8) -> u16 {
    u16::from(symbol) | (u16::from(extra_bits) << 8)
}

fn code_length_tree_symbol(entry: u16) -> u8 {
    (entry & 0xff) as u8
}

fn code_length_tree_extra_bits(entry: u16) -> u8 {
    (entry >> 8) as u8
}

fn write_code_length_code_lengths(
    writer: &mut BitWriter,
    code_length_lengths: &[u8],
    tree_frequencies: &[usize; CODE_LENGTH_ALPHABET_SIZE],
) -> Result<(), CompressError> {
    let num_codes = tree_frequencies
        .iter()
        .filter(|&&frequency| frequency != 0)
        .count();
    let mut codes_to_store = CODE_LENGTH_ORDER.len();
    if num_codes > 1 {
        while codes_to_store > 0
            && code_length_lengths[usize::from(CODE_LENGTH_ORDER[codes_to_store - 1])] == 0
        {
            codes_to_store -= 1;
        }
    }

    let mut skip = 0;
    if code_length_lengths[usize::from(CODE_LENGTH_ORDER[0])] == 0
        && code_length_lengths[usize::from(CODE_LENGTH_ORDER[1])] == 0
    {
        skip = 2;
        if code_length_lengths[usize::from(CODE_LENGTH_ORDER[2])] == 0 {
            skip = 3;
        }
    }

    writer.write_bits_trusted_fits(2, skip as u64);
    for &symbol in CODE_LENGTH_ORDER.iter().take(codes_to_store).skip(skip) {
        write_code_length_code_len(writer, code_length_lengths[usize::from(symbol)])?;
    }
    Ok(())
}

fn single_non_zero_symbol(frequencies: &[usize]) -> Option<usize> {
    let mut single = None;
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        if single.is_some() {
            return None;
        }
        single = Some(symbol);
    }
    single
}

fn ceil_log2(value: usize) -> Result<u8, CompressError> {
    if value == 0 {
        return Err(BurliError::Format("invalid Brotli prefix symbol count"));
    }
    if value == 1 {
        return Ok(0);
    }
    Ok((usize::BITS - (value - 1).leading_zeros()) as u8)
}

fn write_literal(
    writer: &mut BitWriter,
    codes: &[Option<SymbolCode>],
    literal: u8,
) -> Result<(), CompressError> {
    let code = symbol_code(codes, u16::from(literal))?;
    writer.write_bits_trusted(code.len, u64::from(code.bits));
    Ok(())
}

fn write_literals_dense(
    writer: &mut BitWriter,
    input: &[u8],
    codes: &[DenseSymbolCode; LITERAL_ALPHABET_SIZE],
) -> Result<(), CompressError> {
    let mut pending_bits = 0_u64;
    let mut pending_width = 0_u8;
    for &literal in input {
        let code = codes[usize::from(literal)];
        if code.len == u8::MAX {
            return Err(BurliError::Format("missing Brotli prefix symbol"));
        }
        append_pending_bits(
            writer,
            &mut pending_bits,
            &mut pending_width,
            code.len,
            u64::from(code.bits),
        );
    }
    if pending_width != 0 {
        writer.write_bits_trusted_fits(pending_width, pending_bits);
    }
    Ok(())
}

#[inline(always)]
fn append_pending_bits(
    writer: &mut BitWriter,
    pending_bits: &mut u64,
    pending_width: &mut u8,
    width: u8,
    bits: u64,
) {
    debug_assert!(width <= MAX_BITS_PER_OP);
    debug_assert!(width != 0 || bits == 0);
    debug_assert!(width == 0 || bits < (1_u64 << width));
    if *pending_width + width > MAX_BITS_PER_OP {
        writer.write_bits_trusted_nonzero_fits(*pending_width, *pending_bits);
        *pending_bits = 0;
        *pending_width = 0;
    }
    *pending_bits |= bits << *pending_width;
    *pending_width += width;
}

#[derive(Clone, Copy, Debug)]
struct InsertLengthCode {
    code: usize,
    extra_bits: u8,
    extra: u64,
}

fn insert_length_code(len: usize) -> Result<InsertLengthCode, CompressError> {
    let code = match len {
        0..=5 => len,
        6..=9 => 6 + (len - 6) / 2,
        10..=17 => 8 + (len - 10) / 4,
        18..=33 => 10 + (len - 18) / 8,
        34..=65 => 12 + (len - 34) / 16,
        66..=129 => 14 + (len - 66) / 32,
        130..=193 => 16,
        194..=321 => 17,
        322..=577 => 18,
        578..=1089 => 19,
        1090..=2113 => 20,
        2114..=6209 => 21,
        6210..=22593 => 22,
        22594..=MAX_META_BLOCK_SIZE => 23,
        _ => return Err(BurliError::Format("Brotli insert length exceeds range")),
    };
    let (base, extra_bits) = insert_length_prefix(code)?;
    Ok(InsertLengthCode {
        code,
        extra_bits,
        extra: (len - base) as u64,
    })
}

fn insert_length_prefix(code: usize) -> Result<(usize, u8), CompressError> {
    match code {
        0..=5 => Ok((code, 0)),
        6..=7 => Ok((6 + (code - 6) * 2, 1)),
        8..=9 => Ok((10 + (code - 8) * 4, 2)),
        10..=11 => Ok((18 + (code - 10) * 8, 3)),
        12..=13 => Ok((34 + (code - 12) * 16, 4)),
        14..=15 => Ok((66 + (code - 14) * 32, 5)),
        16 => Ok((130, 6)),
        17 => Ok((194, 7)),
        18 => Ok((322, 8)),
        19 => Ok((578, 9)),
        20 => Ok((1090, 10)),
        21 => Ok((2114, 12)),
        22 => Ok((6210, 14)),
        23 => Ok((22594, 24)),
        _ => Err(BurliError::Format("invalid Brotli insert length code")),
    }
}

#[derive(Clone, Copy, Debug)]
struct CopyLengthCode {
    code: usize,
    extra_bits: u8,
    extra: u64,
}

fn copy_length_code(len: usize) -> Result<CopyLengthCode, CompressError> {
    let code = match len {
        2..=9 => len - 2,
        10..=13 => 8 + (len - 10) / 2,
        14..=21 => 10 + (len - 14) / 4,
        22..=37 => 12 + (len - 22) / 8,
        38..=69 => 14 + (len - 38) / 16,
        70..=101 => 16,
        102..=133 => 17,
        134..=197 => 18,
        198..=325 => 19,
        326..=581 => 20,
        582..=1093 => 21,
        1094..=2117 => 22,
        2118..=MAX_META_BLOCK_SIZE => 23,
        _ => return Err(BurliError::Format("Brotli copy length exceeds range")),
    };
    let (base, extra_bits) = copy_length_prefix(code)?;
    Ok(CopyLengthCode {
        code,
        extra_bits,
        extra: (len - base) as u64,
    })
}

fn copy_length_prefix(code: usize) -> Result<(usize, u8), CompressError> {
    match code {
        0..=7 => Ok((code + 2, 0)),
        8..=9 => Ok((10 + (code - 8) * 2, 1)),
        10..=11 => Ok((14 + (code - 10) * 4, 2)),
        12..=13 => Ok((22 + (code - 12) * 8, 3)),
        14..=15 => Ok((38 + (code - 14) * 16, 4)),
        16 => Ok((70, 5)),
        17 => Ok((102, 5)),
        18 => Ok((134, 6)),
        19 => Ok((198, 7)),
        20 => Ok((326, 8)),
        21 => Ok((582, 9)),
        22 => Ok((1094, 10)),
        23 => Ok((2118, 24)),
        _ => Err(BurliError::Format("invalid Brotli copy length code")),
    }
}

fn command_symbol_for_insert(insert_code: usize) -> Result<u16, CompressError> {
    let symbol = match insert_code {
        0..=7 => insert_code * 8,
        8..=15 => 256 + (insert_code - 8) * 8,
        16..=23 => 448 + (insert_code - 16) * 8,
        _ => return Err(BurliError::Format("invalid Brotli insert length code")),
    };
    Ok(symbol as u16)
}

fn command_symbol_for_insert_copy(
    insert_code: usize,
    copy_code: usize,
    use_last_distance: bool,
) -> Result<u16, CompressError> {
    let insert_group = insert_code / 8;
    let copy_group = copy_code / 8;
    let insert_low = insert_code % 8;
    let copy_low = copy_code % 8;
    let cell = match (insert_group, copy_group) {
        (0, 0) => 2,
        (0, 1) => 3,
        (1, 0) => 4,
        (1, 1) => 5,
        (0, 2) => 6,
        (2, 0) => 7,
        (1, 2) => 8,
        (2, 1) => 9,
        (2, 2) => 10,
        _ => return Err(BurliError::Format("invalid Brotli command length code")),
    };
    let cell = if use_last_distance {
        match cell {
            2 => 0,
            3 => 1,
            _ => return Err(BurliError::Format("invalid Brotli last-distance command")),
        }
    } else {
        cell
    };
    Ok((cell * 64 + insert_low * 8 + copy_low) as u16)
}

#[derive(Clone, Copy, Debug)]
struct DistanceCode {
    symbol: u16,
    extra_bits: u8,
    extra: u64,
}

fn distance_code(distance: usize) -> Result<DistanceCode, CompressError> {
    if distance == 0 {
        return Err(BurliError::Format("invalid Brotli zero distance"));
    }

    let d = distance + 3;
    let bits = (usize::BITS - d.leading_zeros() - 1) as usize;
    if bits == 0 || bits > 24 {
        return Err(BurliError::Format("Brotli distance exceeds range"));
    }
    let extra_bits = bits - 1;
    let parity = (d >> extra_bits) & 1;
    let base = (2 + parity) << extra_bits;
    Ok(DistanceCode {
        symbol: (16 + 2 * (extra_bits - 1) + parity) as u16,
        extra_bits: extra_bits as u8,
        extra: (d - base) as u64,
    })
}

#[cfg(kani)]
fn reverse_bits(value: u8, width: u8) -> u8 {
    let mut reversed = 0;
    for bit in 0..width {
        reversed <<= 1;
        reversed |= (value >> bit) & 1;
    }
    reversed
}

fn reverse_bits_u16(value: u16, width: u8) -> u16 {
    let mut reversed = 0;
    for bit in 0..width {
        reversed <<= 1;
        reversed |= (value >> bit) & 1;
    }
    reversed
}

fn write_simple_prefix_code_single(
    writer: &mut BitWriter,
    alphabet_size: usize,
    symbol: u16,
) -> Result<(), CompressError> {
    let alphabet_bits = alphabet_bits(alphabet_size);
    if usize::from(symbol) >= alphabet_size {
        return Err(BurliError::Format("Brotli prefix symbol exceeds alphabet"));
    }

    writer.write_bits(2, 1)?;
    writer.write_bits(2, 0)?;
    writer.write_bits(alphabet_bits, u64::from(symbol))
}

#[derive(Clone, Copy, Debug)]
struct SymbolCode {
    symbol: u16,
    len: u8,
    bits: u16,
}

#[derive(Clone, Copy, Debug)]
struct DenseSymbolCode {
    len: u8,
    bits: u16,
}

const MISSING_DENSE_SYMBOL_CODE: DenseSymbolCode = DenseSymbolCode {
    len: u8::MAX,
    bits: 0,
};

fn write_simple_prefix_code_symbols(
    writer: &mut BitWriter,
    alphabet_size: usize,
    symbols: &[u16],
) -> Result<Vec<SymbolCode>, CompressError> {
    let symbols = sorted_unique_symbols(symbols, alphabet_size)?;
    let alphabet_bits = alphabet_bits(alphabet_size);

    writer.write_bits(2, 1)?;
    writer.write_bits(2, (symbols.len() - 1) as u64)?;
    for &symbol in &symbols {
        writer.write_bits(alphabet_bits, u64::from(symbol))?;
    }
    if symbols.len() == 4 {
        writer.write_bits(1, 0)?;
    }

    Ok(simple_symbol_codes(&symbols))
}

fn write_simple_dense_prefix_code<const N: usize>(
    writer: &mut BitWriter,
    symbols: &[u16],
    map: &mut [DenseSymbolCode; N],
) -> Result<(), CompressError> {
    if symbols.is_empty() || symbols.len() > MAX_SIMPLE_PREFIX_SYMBOLS {
        return Err(BurliError::Format(
            "invalid Brotli simple prefix symbol count",
        ));
    }

    let alphabet_bits = alphabet_bits(N);
    writer.write_bits(2, 1)?;
    writer.write_bits(2, (symbols.len() - 1) as u64)?;
    for &symbol in symbols {
        if usize::from(symbol) >= N {
            return Err(BurliError::Format("Brotli prefix symbol exceeds alphabet"));
        }
        writer.write_bits(alphabet_bits, u64::from(symbol))?;
    }
    if symbols.len() == 4 {
        writer.write_bits(1, 0)?;
    }

    fill_dense_simple_symbol_code_map(symbols, map);
    Ok(())
}

fn fill_dense_simple_symbol_code_map<const N: usize>(
    symbols: &[u16],
    map: &mut [DenseSymbolCode; N],
) {
    match symbols.len() {
        1 => {
            map[usize::from(symbols[0])] = DenseSymbolCode { len: 0, bits: 0 };
        }
        2 => {
            map[usize::from(symbols[0])] = DenseSymbolCode { len: 1, bits: 0 };
            map[usize::from(symbols[1])] = DenseSymbolCode { len: 1, bits: 1 };
        }
        3 => {
            map[usize::from(symbols[0])] = DenseSymbolCode { len: 1, bits: 0 };
            map[usize::from(symbols[1])] = DenseSymbolCode { len: 2, bits: 1 };
            map[usize::from(symbols[2])] = DenseSymbolCode { len: 2, bits: 3 };
        }
        4 => {
            map[usize::from(symbols[0])] = DenseSymbolCode { len: 2, bits: 0 };
            map[usize::from(symbols[1])] = DenseSymbolCode { len: 2, bits: 2 };
            map[usize::from(symbols[2])] = DenseSymbolCode { len: 2, bits: 1 };
            map[usize::from(symbols[3])] = DenseSymbolCode { len: 2, bits: 3 };
        }
        _ => unreachable!(),
    }
}

fn sorted_unique_symbols(symbols: &[u16], alphabet_size: usize) -> Result<Vec<u16>, CompressError> {
    if symbols.is_empty() || symbols.len() > MAX_SIMPLE_PREFIX_SYMBOLS {
        return Err(BurliError::Format(
            "invalid Brotli simple prefix symbol count",
        ));
    }

    let mut unique = Vec::new();
    for &symbol in symbols {
        if usize::from(symbol) >= alphabet_size {
            return Err(BurliError::Format("Brotli prefix symbol exceeds alphabet"));
        }
        push_unique(&mut unique, symbol);
    }
    unique.sort_unstable();
    Ok(unique)
}

fn simple_symbol_codes(symbols: &[u16]) -> Vec<SymbolCode> {
    let lengths = match symbols.len() {
        1 => vec![0],
        2 => vec![1, 1],
        3 => vec![1, 2, 2],
        4 => vec![2, 2, 2, 2],
        _ => unreachable!(),
    };
    symbol_codes_from_lengths_and_symbols(&lengths, symbols)
}

fn symbol_codes_from_lengths(lengths: &[u8]) -> Vec<SymbolCode> {
    let symbols = lengths
        .iter()
        .enumerate()
        .filter_map(|(symbol, &len)| (len != 0).then_some(symbol as u16))
        .collect::<Vec<_>>();
    let lengths = symbols
        .iter()
        .map(|&symbol| lengths[usize::from(symbol)])
        .collect::<Vec<_>>();
    symbol_codes_from_lengths_and_symbols(&lengths, &symbols)
}

fn symbol_codes_from_lengths_and_symbols(lengths: &[u8], symbols: &[u16]) -> Vec<SymbolCode> {
    let mut counts = [0_u16; 16];
    for &len in lengths {
        if len != 0 {
            counts[usize::from(len)] += 1;
        }
    }

    let mut next_code = [0_u16; 16];
    let mut code = 0_u16;
    for bits in 1..=15 {
        code = (code + counts[bits - 1]) << 1;
        next_code[bits] = code;
    }

    let mut codes = Vec::with_capacity(symbols.len());
    for (&symbol, &len) in symbols.iter().zip(lengths) {
        let code = if len == 0 {
            0
        } else {
            let code = next_code[usize::from(len)];
            next_code[usize::from(len)] += 1;
            code
        };
        codes.push(SymbolCode {
            symbol,
            len,
            bits: reverse_bits_u16(code, len),
        });
    }
    codes
}

fn symbol_code_map(codes: &[SymbolCode], alphabet_size: usize) -> Vec<Option<SymbolCode>> {
    let mut map = vec![None; alphabet_size];
    for &code in codes {
        if usize::from(code.symbol) < alphabet_size {
            map[usize::from(code.symbol)] = Some(code);
        }
    }
    map
}

fn dense_symbol_code_map_from_symbol_codes<const N: usize>(
    codes: &[SymbolCode],
) -> Result<[DenseSymbolCode; N], CompressError> {
    let mut map = [MISSING_DENSE_SYMBOL_CODE; N];
    for &code in codes {
        let symbol = usize::from(code.symbol);
        if symbol >= N {
            return Err(BurliError::Format("Brotli prefix symbol exceeds alphabet"));
        }
        map[symbol] = DenseSymbolCode {
            len: code.len,
            bits: code.bits,
        };
    }
    Ok(map)
}

fn symbol_code(codes: &[Option<SymbolCode>], symbol: u16) -> Result<SymbolCode, CompressError> {
    codes
        .get(usize::from(symbol))
        .copied()
        .flatten()
        .ok_or(BurliError::Format("missing Brotli prefix symbol"))
}

fn fill_dense_symbol_code_map_from_lengths<const N: usize>(
    lengths: &[u8],
    map: &mut [DenseSymbolCode; N],
) {
    debug_assert_eq!(lengths.len(), N);
    let mut counts = [0_u16; 16];
    for &len in lengths {
        if len != 0 {
            counts[usize::from(len)] += 1;
        }
    }

    let mut next_code = [0_u16; 16];
    let mut code = 0_u16;
    for bits in 1..=15 {
        code = (code + counts[bits - 1]) << 1;
        next_code[bits] = code;
    }

    for (symbol, &len) in lengths.iter().enumerate() {
        if len == 0 {
            continue;
        }
        let code = next_code[usize::from(len)];
        next_code[usize::from(len)] += 1;
        map[symbol] = DenseSymbolCode {
            len,
            bits: reverse_bits_u16(code, len),
        };
    }
}

fn alphabet_bits(alphabet_size: usize) -> u8 {
    let value = alphabet_size.saturating_sub(1);
    (usize::BITS - value.leading_zeros()) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sparse_sample(duplicate_6_count: usize) -> sparse::Sample {
        sparse::Sample {
            duplicate_6_count,
            zero_count: 0,
            printable_count: 1024,
            max_miss_streak: 0,
            len: 1024,
        }
    }

    #[test]
    fn q1_emits_compressed_stream() {
        let input =
            b"abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789".repeat(64);
        let encoded =
            compress_with_options(&input, &Options::default().quality(1).unwrap()).unwrap();

        assert_ne!(
            encoded,
            crate::metablock::compress_uncompressed_with_options(
                &input,
                &Options::default().quality(0).unwrap()
            )
            .unwrap()
        );
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q0_emits_compressed_stream_for_repeated_payload() {
        let input = b"function demo(){return demo_value;} ".repeat(256);
        let encoded =
            compress_with_options(&input, &Options::default().quality(0).unwrap()).unwrap();
        let uncompressed = crate::metablock::compress_uncompressed_with_options(
            &input,
            &Options::default().quality(0).unwrap(),
        )
        .unwrap();

        assert!(encoded.len() < uncompressed.len());
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q0_uses_uncompressed_for_tiny_payloads() {
        let input = b"<html><body>hello burli</body></html>".repeat(4);
        let options = Options::default().quality(0).unwrap();
        let encoded = compress_with_options(&input, &options).unwrap();
        let uncompressed =
            crate::metablock::compress_uncompressed_with_options(&input, &options).unwrap();

        assert_eq!(encoded, uncompressed);
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q0_routes_by_size_and_sparse_outcome() {
        let css_2k = b"@charset \"UTF-8\";\n.selector{display:block;}\n".repeat(48);
        let script_2k = b"/*! comment */\nfunction demo(){return demo();}\n".repeat(48);
        let json_2k = br#"{"areaNames":{"205705993":"Arena","205705994":"Hall"}}"#.repeat(48);

        assert_eq!(
            q0_collect_route(css_2k[..2048].len(), None),
            q0_collect_route(script_2k[..2048].len(), None)
        );
        assert_eq!(
            q0_collect_route(script_2k[..2048].len(), None),
            q0_collect_route(json_2k[..2048].len(), None)
        );
        assert_eq!(
            q0_collect_route(2048, None),
            Q0CollectRoute::FastNoLastDistance
        );
        assert_eq!(q0_collect_route(4096, None), Q0CollectRoute::NoLastDistance);
        assert_eq!(q0_collect_route(8192, None), Q0CollectRoute::DefaultSkip);
        assert_eq!(
            q0_collect_route(16 * 1024, None),
            Q0CollectRoute::MediumNoLastDistance
        );
        assert_eq!(
            q0_collect_route(32 * 1024, None),
            Q0CollectRoute::MediumSkip
        );
        assert_eq!(
            q0_collect_route(64 * 1024, Some(sparse_sample(148))),
            Q0CollectRoute::K64MediumSkip
        );
        assert_eq!(
            q0_collect_route(128 * 1024, Some(sparse_sample(237))),
            Q0CollectRoute::K64FastSkip
        );
        assert_eq!(
            q0_collect_route(64 * 1024, Some(sparse_sample(583))),
            Q0CollectRoute::K32U16Skip
        );
        assert_eq!(
            q0_collect_route(128 * 1024, Some(sparse_sample(583))),
            Q0CollectRoute::K32DenseSkip
        );
        assert_eq!(
            q0_collect_route(2 * 1024 * 1024, Some(sparse_sample(583))),
            Q0CollectRoute::K64MediumSkip
        );
        assert_eq!(
            q0_collect_route(256 * 1024, Some(sparse_sample(148))),
            Q0CollectRoute::K32FasterSkip
        );

        assert_eq!(
            q0_write_route(2048, None),
            Q0WriteRoute::BalancedLiteralCommand
        );
        assert_eq!(q0_write_route(4096, None), Q0WriteRoute::PackedLiteralBody);
        assert_eq!(q0_write_route(16 * 1024, None), Q0WriteRoute::FastCommand);
        assert_eq!(
            q0_write_route(128 * 1024, Some(sparse_sample(583))),
            Q0WriteRoute::PackedLiteralBody
        );
        assert_eq!(
            q0_write_route(256 * 1024, Some(sparse_sample(148))),
            Q0WriteRoute::BalancedCommand
        );
        assert_eq!(
            q0_write_route(2 * 1024 * 1024, Some(sparse_sample(583))),
            Q0WriteRoute::Standard
        );

        let sparse_binary = sparse_binary_fixture(64 * 1024);
        let zero_heavy_sparse = {
            let mut input = sparse_binary_fixture(64 * 1024);
            for byte in input.iter_mut().step_by(8) {
                *byte = 0;
            }
            input
        };
        let repeated_binary = vec![42_u8; 64 * 1024];
        let printable_sparse = printable_sparse_fixture(64 * 1024);
        assert!(sparse::should_accelerate(&sparse_binary));
        assert!(!sparse::should_accelerate(&zero_heavy_sparse));
        assert!(!sparse::should_accelerate(&repeated_binary));
        assert!(!sparse::should_accelerate(&printable_sparse));
        let sao_like = {
            let mut input = sparse_binary_fixture(64 * 1024);
            for offset in (0..input.len().saturating_sub(72)).step_by(4096) {
                let copy: [u8; 8] = input[offset..offset + 8].try_into().unwrap();
                input[offset + 64..offset + 72].copy_from_slice(&copy);
            }
            input
        };
        assert_eq!(sparse::q1_skip(&sparse_binary), sparse::Q1Skip::Store);
        assert_eq!(sparse::q1_skip(&sao_like), sparse::Q1Skip::Moderate);
        assert_eq!(sparse::q1_skip(&printable_sparse), sparse::Q1Skip::None);
        assert!(sparse::should_accelerate(&sao_like));
        assert!(!sparse::should_accelerate(&printable_sparse));
        for block in 0..16 {
            assert_eq!(
                sparse::q0_store_block(block << 18, false),
                !matches!(block, 0 | 5 | 10)
            );
        }
        assert!(sparse::q0_store_block(8 << 18, true));
        assert!(!sparse::q1_store_block(0, false));
        assert!(!sparse::q1_store_block(1 << 18, false));
        assert!(!sparse::q1_store_block(6 << 18, false));
        assert!(!sparse::q1_store_block(8 << 18, false));
        assert!(!sparse::q1_store_block(9 << 18, false));
        assert!(!sparse::q1_store_block(8 << 18, true));
        assert!(q1_large_markup_lazy_is_likely_safe(
            b"<a><b><c><d><e><f><g><h></h></g></f></e></d></c></b></a>"
        ));
        assert!(!q1_large_markup_lazy_is_likely_safe(&script_2k));
        let prose_like =
            b"Many words in a sentence with enough spaces to look like prose. ".repeat(1200);
        let numeric_table_like = b"1 2 3 4 5 6 7 8 9 0                         \n".repeat(1600);
        let dictionary_like = b"word<entry>definition</entry> ".repeat(2600);
        let zero_high_mixed = (0..64 * 1024)
            .map(|index| match index % 4 {
                0 => 0,
                1 => 200,
                _ => index as u8,
            })
            .collect::<Vec<_>>();
        let zero_low_mixed = (0..64 * 1024)
            .map(|index| if index % 4 == 0 { 0 } else { b'a' })
            .collect::<Vec<_>>();
        assert!(q1_no_cross_one_lazy_is_likely_safe(&numeric_table_like));
        assert!(q1_no_cross_one_lazy_is_likely_safe(&zero_high_mixed));
        assert!(!q1_no_cross_one_lazy_is_likely_safe(&prose_like));
        assert!(!q1_no_cross_one_lazy_is_likely_safe(&dictionary_like));
        assert!(q1_no_cross_sparse_tail_no_last_is_likely_safe(
            &zero_low_mixed
        ));
        assert!(!q1_no_cross_sparse_tail_no_last_is_likely_safe(
            &zero_high_mixed
        ));
        assert!(!q1_no_cross_sparse_tail_no_last_is_likely_safe(
            &numeric_table_like
        ));
        assert!(!q1_no_cross_fast_writer_is_likely_safe(&prose_like));
        assert!(!q1_no_cross_fast_writer_is_likely_safe(&numeric_table_like));
        assert!(q1_no_cross_fast_writer_is_likely_safe(&sparse_binary));
        assert!(q1_no_cross_fast_writer_is_likely_safe(&dictionary_like));
    }

    #[test]
    fn q0_compresses_long_repetitive_payload() {
        let input = br#"{"name":"burli","kind":"brotli","safe":true}"#.repeat(4096);
        let encoded =
            compress_with_options(&input, &Options::default().quality(0).unwrap()).unwrap();

        assert!(encoded.len() * 20 < input.len());
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q0_round_trips_non_power_input_lengths() {
        for len in [
            257_usize, 383, 384, 385, 511, 512, 513, 639, 640, 641, 717, 1023, 1024, 1025,
        ] {
            let mut input = Vec::with_capacity(len);
            while input.len() < len {
                input.extend_from_slice(
                    b"<section data-kind=\"bench\">alpha beta gamma</section>\n",
                );
                input.extend_from_slice(b"const render = value => value + 17;\n");
            }
            input.truncate(len);

            let encoded =
                compress_with_options(&input, &Options::default().quality(0).unwrap()).unwrap();

            assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
        }
    }

    #[test]
    fn q1_round_trips_mixed_literals() {
        let input = b"function demo(){return 42;}";
        let encoded =
            compress_with_options(input, &Options::default().quality(1).unwrap()).unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q1_round_trips_non_power_input_lengths() {
        for len in [
            1_usize, 15, 16, 17, 223, 224, 255, 256, 257, 383, 384, 385, 511, 512, 513, 717, 1023,
            1024, 1025, 4095, 4096, 4097,
        ] {
            let mut input = Vec::with_capacity(len);
            while input.len() < len {
                input.extend_from_slice(b"<div class=\"item\">alpha beta gamma</div>\n");
                input.extend_from_slice(b"function render(value){return value + 17;}\n");
            }
            input.truncate(len);

            let encoded =
                compress_with_options(&input, &Options::default().quality(1).unwrap()).unwrap();

            assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
        }
    }

    #[test]
    fn q5_round_trips_long_literal_run() {
        let input = vec![b'a'; 3000];
        let encoded =
            compress_with_options(&input, &Options::default().quality(5).unwrap()).unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q5_round_trips_literal_run_above_64k() {
        let input = vec![b'x'; 70_000];
        let encoded =
            compress_with_options(&input, &Options::default().quality(5).unwrap()).unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q5_compresses_repeated_payload() {
        let input = b"0123456789abcdef".repeat(128);
        let encoded =
            compress_with_options(&input, &Options::default().quality(5).unwrap()).unwrap();

        assert!(encoded.len() < input.len());
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q1_compresses_long_repeated_payload() {
        let input =
            b"abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789".repeat(64);
        let encoded =
            compress_with_options(&input, &Options::default().quality(1).unwrap()).unwrap();

        assert!(encoded.len() < input.len());
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q1_sparse_binary_round_trips() {
        let mut input = sparse_binary_fixture(320 * 1024 + 17);
        for offset in (0..input.len().saturating_sub(72)).step_by(4096) {
            let copy: [u8; 8] = input[offset..offset + 8].try_into().unwrap();
            input[offset + 64..offset + 72].copy_from_slice(&copy);
        }
        let encoded =
            compress_with_options(&input, &Options::default().quality(1).unwrap()).unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q1_collects_four_byte_matches_for_small_tables() {
        let input = b"abcd----abcd----abcd----abcd----abcd----abcd----abcd----abcd----";
        let mut workspace = q1::Workspace::default();
        let batch = q1::collect(input, (1 << 22) - 16, &mut workspace).unwrap();

        assert!(batch.has_copy());
    }

    #[test]
    fn q1_collects_six_byte_matches_for_large_tables() {
        let mut input = b"abcdef0123456789".repeat(5000);
        input.extend_from_slice(b"abcdef0123456789abcdef0123456789");
        let mut workspace = q1::Workspace::default();
        let batch = q1::collect(&input, (1 << 22) - 16, &mut workspace).unwrap();

        assert!(batch.has_copy());
    }

    #[test]
    fn q0_sparse_binary_round_trips() {
        let input = sparse_binary_fixture(128 * 1024 + 17);
        let encoded =
            compress_with_options(&input, &Options::default().quality(0).unwrap()).unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q0_store_stats_report_sparse_skip_work() {
        let options = Options::default().quality(0).unwrap();
        let input = sparse_binary_fixture(128 * 1024 + 17);
        let stats = q0_store_stats(&input, &options).unwrap();

        assert_eq!(stats.blocks, 1);
        assert_eq!(stats.sampled_blocks, 1);
        assert_eq!(stats.stored_blocks, 1);
        assert_eq!(stats.stored_bytes, input.len());
        assert_eq!(stats.sampled_positions, 1024);
        assert!(stats.skipped_probe_positions > input.len() * 9 / 10);

        let printable = printable_sparse_fixture(128 * 1024 + 17);
        let stats = q0_store_stats(&printable, &options).unwrap();

        assert_eq!(stats.stored_blocks, 0);
        assert_eq!(stats.stored_bytes, 0);
    }

    fn sparse_binary_fixture(len: usize) -> Vec<u8> {
        let mut state = 0x1234_5678_9abc_def0_u64;
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            state ^= state << 7;
            state ^= state >> 9;
            state ^= state << 8;
            out.push((state as u8).wrapping_add(1));
        }
        out
    }

    fn printable_sparse_fixture(len: usize) -> Vec<u8> {
        let mut state = 0x0fed_cba9_8765_4321_u64;
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            state ^= state << 7;
            state ^= state >> 9;
            state ^= state << 8;
            out.push(32 + (state as u8 % 95));
        }
        out
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    #[kani::unwind(9)]
    fn reverse_bits_width_8_is_involution() {
        let value = kani::any::<u8>();

        assert_eq!(reverse_bits(reverse_bits(value, 8), 8), value);
    }

    #[kani::proof]
    #[kani::unwind(25)]
    fn insert_length_code_covers_meta_block_range() {
        let raw_len = kani::any::<u32>();
        kani::assume(raw_len > 0);
        kani::assume(raw_len <= MAX_META_BLOCK_SIZE as u32);

        let len = raw_len as usize;
        let insert = insert_length_code(len).unwrap();
        let (base, extra_bits) = insert_length_prefix(insert.code).unwrap();
        let command_symbol = command_symbol_for_insert(insert.code).unwrap();

        assert_eq!(base + insert.extra as usize, len);
        assert!(insert.extra < (1_u64 << extra_bits));
        assert!(usize::from(command_symbol) < 704);
        assert_eq!(decode_insert_code(command_symbol), insert.code);
    }

    fn decode_insert_code(symbol: u16) -> usize {
        let code = usize::from(symbol);
        let high = (code >> 3) & 0b111;
        match code >> 6 {
            0 => high,
            4 => 8 + high,
            7 => 16 + high,
            _ => usize::MAX,
        }
    }
}
