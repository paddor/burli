//! Brotli stream decoding with input and output in pieces.

use alloc::vec::Vec;

use burli_core::{BurliError, DecompressError, bits::BitReader};

use crate::compressed::DistanceRing;
use crate::compressed::incremental::{END_OF_INPUT, MetaBlock, Step, is_end_of_input};
use crate::stored::{self, MetaBlockHeader};
use crate::{Options, RawDictionary};

/// Input bytes the decoder takes at a time when it needs more. A step that
/// stops for output room hands back what it did not read, so taking all
/// input at once would copy the rest of a long input in every call.
const INPUT_PIECE: usize = 64 * 1024;

/// Brotli stream decoder that takes its input and output in pieces.
///
/// Each [`decompress`](Self::decompress) call takes the input the decoder
/// needs and writes decoded bytes as soon as they are decoded, also within
/// a meta-block. Of the input that does not finish a step, it keeps only
/// what that step read, so input split in any way decodes in linear time.
/// It keeps the decoded history that later copies may reach, plus output
/// not yet written.
///
/// The decoder never takes input past the stream end, so bytes that follow
/// the stream stay with the caller.
pub struct PushDecoder {
    max_output_size: usize,
    raw_dictionary: RawDictionary,
    /// Input taken from the caller that the decoder may still read.
    encoded: Vec<u8>,
    /// Position in `encoded` of the last checkpoint.
    bit_pos: usize,
    /// Zero until the stream header is read.
    window_bits: u8,
    distances: DistanceRing,
    /// Decoded history and output not yet written.
    output: Vec<u8>,
    /// Decoded bytes dropped from the front of `output`.
    output_base: usize,
    /// Bytes of `output` written to the caller.
    written: usize,
    /// Bytes of `output` decoded.
    decoded: usize,
    state: State,
}

enum State {
    StreamHeader,
    MetaBlockHeader,
    Uncompressed {
        remaining: usize,
    },
    Metadata {
        remaining: usize,
        is_last: bool,
    },
    Compressed(MetaBlock),
    Done,
    /// Output up to the failure is written before the error is returned.
    Failed(DecompressError),
}

/// What a [`PushDecoder::decompress`] call needs next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeStatus {
    /// The decoder took all of the input and needs more.
    NeedsInput,
    /// The decoder has more output, or an error to return. Call again with
    /// output room, and with the input past what this call consumed.
    NeedsOutput,
    /// The stream is decoded, and all of its output is written.
    Finished,
}

/// The result of a [`PushDecoder::decompress`] call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodeProgress {
    /// Input bytes the decoder took.
    pub consumed: usize,
    /// Output bytes written.
    pub written: usize,
    /// What the decoder needs next.
    pub status: DecodeStatus,
}

impl Default for PushDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl PushDecoder {
    /// Create a decoder with no practical output limit.
    pub fn new() -> Self {
        Self::with_options(&Options::new())
    }

    /// Create a decoder with explicit [`Options`].
    pub fn with_options(options: &Options) -> Self {
        Self::with_raw_dictionary_and_options(RawDictionary::empty(), options)
    }

    /// Create a decoder with a raw LZ77 prefix dictionary.
    pub fn with_raw_dictionary(dictionary: RawDictionary) -> Self {
        Self::with_raw_dictionary_and_options(dictionary, &Options::new())
    }

    /// Create a decoder with a raw LZ77 prefix dictionary and explicit
    /// [`Options`].
    pub fn with_raw_dictionary_and_options(dictionary: RawDictionary, options: &Options) -> Self {
        Self {
            max_output_size: options.max_output_size(),
            raw_dictionary: dictionary,
            encoded: Vec::new(),
            bit_pos: 0,
            window_bits: 0,
            distances: DistanceRing::new(),
            output: Vec::new(),
            output_base: 0,
            written: 0,
            decoded: 0,
            state: State::StreamHeader,
        }
    }

    /// Return current decode options.
    pub fn options(&self) -> Options {
        Options::new().with_max_output_size(self.max_output_size)
    }

    /// Return the configured raw LZ77 prefix dictionary.
    pub const fn raw_dictionary(&self) -> &RawDictionary {
        &self.raw_dictionary
    }

    /// Whether the stream is decoded and all of its output is written.
    pub fn is_finished(&self) -> bool {
        matches!(self.state, State::Done) && self.written == self.decoded
    }

    /// Decode from `input` into `output`.
    ///
    /// The decoder first writes output it holds, and takes input only when
    /// it needs more. It decodes until `output` is full or the input runs
    /// out; with `output` full, it still reads headers and the stream end,
    /// which write nothing. [`DecodeProgress::consumed`] says how much input
    /// it took; pass the rest again in a later call. The status says what to
    /// do next.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed streams, unsupported large-window
    /// streams, or output-limit violations, once all output up to the
    /// failure is written. The decoder then returns the same error again.
    pub fn decompress(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<DecodeProgress, DecompressError> {
        // Input before the last checkpoint is decoded.
        let decoded_input = self.bit_pos / 8;
        self.encoded.drain(..decoded_input);
        self.bit_pos -= decoded_input * 8;

        // Length of `encoded` before it took `input`, and the bytes of
        // `input` taken.
        let mut taken = None;
        let mut offered = 0;
        let mut written = 0;
        loop {
            written += self.write_output(&mut output[written..]);
            if self.written < self.decoded {
                return Ok(self.progress(input, taken, written, DecodeStatus::NeedsOutput));
            }
            match &self.state {
                State::Done => {
                    self.release_output();
                    return Ok(self.progress(input, taken, written, DecodeStatus::Finished));
                }
                State::Failed(error) if written == 0 => return Err(error.clone()),
                State::Failed(_) => {
                    return Ok(self.progress(input, taken, written, DecodeStatus::NeedsOutput));
                }
                _ => {}
            }

            let stop_at = self.decoded + (output.len() - written);
            match self.step(stop_at) {
                Ok(false) => {}
                Ok(true) => {
                    written += self.write_output(&mut output[written..]);
                    return Ok(self.progress(input, taken, written, DecodeStatus::NeedsOutput));
                }
                Err(error) if is_end_of_input(&error) => {
                    // The step may have decoded output up to a checkpoint.
                    written += self.write_output(&mut output[written..]);
                    if self.written < self.decoded {
                        return Ok(self.progress(input, taken, written, DecodeStatus::NeedsOutput));
                    }
                    if offered == input.len() {
                        return Ok(self.progress(input, taken, written, DecodeStatus::NeedsInput));
                    }
                    taken.get_or_insert(self.encoded.len());
                    let piece = &input[offered..input.len().min(offered + INPUT_PIECE)];
                    self.encoded.extend_from_slice(piece);
                    offered += piece.len();
                }
                Err(error) => {
                    self.output.truncate(self.decoded);
                    self.state = State::Failed(error);
                }
            }
        }
    }

    /// Input the decoder took but has not decoded past its last checkpoint.
    #[cfg(feature = "std")]
    pub(crate) fn pending_input(&self) -> &[u8] {
        &self.encoded[self.bit_pos / 8..]
    }

    /// Returns the progress of a call that took `input` when `taken` is set,
    /// and hands back the input past the last checkpoint unless the decoder
    /// needs all of it.
    fn progress(
        &mut self,
        input: &[u8],
        taken: Option<usize>,
        written: usize,
        status: DecodeStatus,
    ) -> DecodeProgress {
        let consumed = match taken {
            None => 0,
            Some(_) if status == DecodeStatus::NeedsInput => input.len(),
            Some(before) => {
                let keep = self.bit_pos.div_ceil(8).clamp(before, self.encoded.len());
                self.encoded.truncate(keep);
                keep - before
            }
        };
        DecodeProgress {
            consumed,
            written,
            status,
        }
    }

    fn write_output(&mut self, output: &mut [u8]) -> usize {
        let count = (self.decoded - self.written).min(output.len());
        output[..count].copy_from_slice(&self.output[self.written..self.written + count]);
        self.written += count;
        count
    }

    /// Drops written history that later copies cannot reach. Moving the
    /// rest costs its length, so it drops only when that is no more than
    /// what goes.
    fn compact_output(&mut self) {
        let window_size = (1_usize << self.window_bits) - 16;
        let drop = self.decoded.saturating_sub(window_size).min(self.written);
        if drop == 0 || drop < self.output.len() - drop {
            return;
        }
        self.output.drain(..drop);
        self.output_base += drop;
        self.written -= drop;
        self.decoded -= drop;
    }

    /// Frees the history of a finished stream once all of it is written.
    fn release_output(&mut self) {
        if self.written == self.decoded && !self.output.is_empty() {
            self.output_base += self.output.len();
            self.output = Vec::new();
            self.written = 0;
            self.decoded = 0;
        }
    }

    /// Decodes one step from the input taken so far, and returns whether it
    /// stopped for output room at `stop_at`. Steps that write no output run
    /// with no room. A step that runs out of input fails with the
    /// end-of-input error and leaves the decoder at its last checkpoint.
    fn step(&mut self, stop_at: usize) -> Result<bool, DecompressError> {
        if matches!(self.state, State::MetaBlockHeader) {
            self.compact_output();
        }
        let Self {
            max_output_size,
            raw_dictionary,
            encoded,
            bit_pos,
            window_bits,
            distances,
            output,
            output_base,
            decoded,
            state,
            ..
        } = self;

        match state {
            State::StreamHeader => {
                let mut reader = BitReader::with_bit_pos(encoded, *bit_pos)?;
                *window_bits = stored::read_window_bits(&mut reader)?;
                *bit_pos = reader.consumed_bits();
                *state = State::MetaBlockHeader;
            }
            State::MetaBlockHeader => {
                let mut reader = BitReader::with_bit_pos(encoded, *bit_pos)?;
                match stored::read_meta_block_header(&mut reader)? {
                    MetaBlockHeader::LastEmpty => {
                        finish_stream(&reader)?;
                        *state = State::Done;
                    }
                    MetaBlockHeader::Metadata { len, is_last } => {
                        reader.read_zero_padding_to_byte()?;
                        *state = State::Metadata {
                            remaining: len,
                            is_last,
                        };
                    }
                    MetaBlockHeader::Uncompressed { len } => {
                        let needed = output
                            .len()
                            .checked_add(len)
                            .and_then(|needed| needed.checked_add(*output_base))
                            .ok_or(BurliError::Format("Brotli output length overflow"))?;
                        if needed > *max_output_size {
                            return Err(BurliError::OutputLimitExceeded {
                                limit: *max_output_size,
                                needed,
                            });
                        }
                        reader.read_zero_padding_to_byte()?;
                        output.reserve(len);
                        *state = State::Uncompressed { remaining: len };
                    }
                    MetaBlockHeader::Compressed { len, is_last } => {
                        *state = State::Compressed(MetaBlock::new(
                            len,
                            is_last,
                            output,
                            *output_base,
                            *window_bits,
                            *max_output_size,
                        )?);
                    }
                }
                *bit_pos = reader.consumed_bits();
            }
            State::Uncompressed { remaining } => {
                if *decoded == stop_at {
                    return Ok(true);
                }
                let start = *bit_pos / 8;
                let count = (encoded.len() - start)
                    .min(*remaining)
                    .min(stop_at - *decoded);
                if count == 0 {
                    return Err(END_OF_INPUT);
                }
                output.extend_from_slice(&encoded[start..start + count]);
                *bit_pos += count * 8;
                *decoded += count;
                *remaining -= count;
                if *remaining == 0 {
                    *state = State::MetaBlockHeader;
                }
            }
            State::Metadata { remaining, is_last } => {
                if *remaining != 0 {
                    let count = (encoded.len() - *bit_pos / 8).min(*remaining);
                    if count == 0 {
                        return Err(END_OF_INPUT);
                    }
                    *bit_pos += count * 8;
                    *remaining -= count;
                }
                if *remaining == 0 {
                    *state = if *is_last {
                        finish_stream(&BitReader::with_bit_pos(encoded, *bit_pos)?)?;
                        State::Done
                    } else {
                        State::MetaBlockHeader
                    };
                }
            }
            State::Compressed(block) => {
                let result = block.step(
                    encoded,
                    bit_pos,
                    output,
                    distances,
                    crate::dictionary::RawDictionary::new(raw_dictionary.as_bytes()),
                    stop_at,
                );
                *decoded = block.decoded_end();
                match result? {
                    Step::More => {}
                    Step::Stopped => return Ok(true),
                    Step::Done => {
                        *state = if block.is_last() {
                            finish_stream(&BitReader::with_bit_pos(encoded, *bit_pos)?)?;
                            State::Done
                        } else {
                            State::MetaBlockHeader
                        };
                    }
                }
            }
            State::Done | State::Failed(_) => {}
        }
        Ok(false)
    }
}

/// Checks the padding bits after the last meta-block. Bytes after it belong
/// to the caller.
fn finish_stream(reader: &BitReader<'_>) -> Result<(), DecompressError> {
    let padding = (8 - (reader.consumed_bits() % 8)) % 8;
    if padding != 0 && reader.peek_bits(padding as u8)? != 0 {
        return Err(BurliError::Format("non-zero trailing Brotli padding"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use burli_core::Options as EncodeOptions;

    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x1234_5678_u32;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8 & 0x3f
            })
            .collect()
    }

    /// Feeds `encoded` in `input` pieces with `output` bytes of room per
    /// call, and checks the decoder after each call with `check`.
    fn decode_checked(
        encoded: &[u8],
        input: usize,
        output: usize,
        mut check: impl FnMut(&PushDecoder),
    ) -> Vec<u8> {
        let mut decoder = PushDecoder::new();
        let mut decoded = Vec::new();
        let mut room = vec![0; output];
        let mut pos = 0;
        loop {
            let end = (pos + input).min(encoded.len());
            let progress = decoder.decompress(&encoded[pos..end], &mut room).unwrap();
            decoded.extend_from_slice(&room[..progress.written]);
            pos += progress.consumed;
            check(&decoder);
            if progress.status == DecodeStatus::Finished {
                return decoded;
            }
        }
    }

    #[test]
    fn keeps_only_the_input_of_the_step_it_could_not_finish() {
        // q0 writes meta-blocks of 256 KiB from 256 KiB of input.
        let input = noise(600_000);
        let encoded = burli::compress(&input, 0).unwrap();
        let mut most_pending = 0;
        let decoded = decode_checked(&encoded, 512, 1 << 20, |decoder| {
            most_pending = most_pending.max(decoder.encoded.len() - decoder.bit_pos / 8);
        });
        assert_eq!(decoded, input);
        assert!(
            most_pending <= 512 + 64,
            "pending input grew to {most_pending}"
        );
    }

    #[test]
    fn retains_about_the_window_of_history() {
        let input = vec![7_u8; (1 << 16) * 4 + 123];
        let options = EncodeOptions::default()
            .with_quality(0)
            .unwrap()
            .with_window_bits(16)
            .unwrap()
            .with_block_bits(Some(16))
            .unwrap();
        let encoded = burli::compress_with_options(&input, &options).unwrap();
        let mut most_retained = 0;
        let decoded = decode_checked(&encoded, 4096, 1024, |decoder| {
            most_retained = most_retained.max(decoder.output.len());
        });
        assert_eq!(decoded, input);
        // Up to twice the window before a compaction, plus one meta-block.
        assert!(most_retained <= 3 << 16, "retained {most_retained} bytes");
    }

    #[test]
    fn frees_the_history_of_a_finished_stream() {
        let input = noise(100_000);
        let encoded = burli::compress(&input, 3).unwrap();
        let mut decoder = PushDecoder::new();
        let mut output = vec![0; input.len()];
        let progress = decoder.decompress(&encoded, &mut output).unwrap();
        assert_eq!(progress.status, DecodeStatus::Finished);
        assert_eq!(output, input);
        assert_eq!(decoder.output.capacity(), 0);
    }
}
