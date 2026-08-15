use std::io::{self, Read};

use burli_core::{BurliError, DecompressError, bits::BitReader};

use crate::{
    compressed::{DistanceRing, MetaBlockDecodeParams},
    stored::{self, MetaBlockHeader},
};

const READ_CHUNK_SIZE: usize = 8 * 1024;
const MIN_STREAM_WINDOW_SIZE: usize = (1 << 10) - 16;

/// `std::io::Read` Brotli stream decoder.
///
/// The decoder emits bytes as soon as a complete meta-block is available and
/// keeps only the decoded window history needed for future backward copies.
pub struct StreamDecoder<R> {
    inner: R,
    max_output_size: usize,
    encoded: Vec<u8>,
    bit_pos: usize,
    window_bits: Option<u8>,
    distances: DistanceRing,
    raw_dictionary: Vec<u8>,
    output: Vec<u8>,
    output_pos: usize,
    output_base: usize,
    state: State,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Reading,
    Done,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DecodeStep {
    NeedMore,
    MadeProgress,
    MadeOutput,
    Done,
}

impl<R: Read> StreamDecoder<R> {
    /// Create a stream decoder with no practical output limit.
    pub const fn new(inner: R) -> Self {
        Self::with_limit(inner, burli_core::format::DEFAULT_MAX_OUTPUT_SIZE)
    }

    /// Create a stream decoder with a hard output limit.
    pub const fn with_limit(inner: R, max_output_size: usize) -> Self {
        Self {
            inner,
            max_output_size,
            encoded: Vec::new(),
            bit_pos: 0,
            window_bits: None,
            distances: DistanceRing::new(),
            raw_dictionary: Vec::new(),
            output: Vec::new(),
            output_pos: 0,
            output_base: 0,
            state: State::Reading,
        }
    }

    /// Create a stream decoder with a raw LZ77 prefix dictionary.
    pub fn with_raw_dictionary(inner: R, dictionary: &[u8]) -> Self {
        Self::with_raw_dictionary_and_limit(
            inner,
            dictionary,
            burli_core::format::DEFAULT_MAX_OUTPUT_SIZE,
        )
    }

    /// Create a stream decoder with a raw LZ77 prefix dictionary and hard
    /// output limit.
    pub fn with_raw_dictionary_and_limit(
        inner: R,
        dictionary: &[u8],
        max_output_size: usize,
    ) -> Self {
        Self {
            inner,
            max_output_size,
            encoded: Vec::new(),
            bit_pos: 0,
            window_bits: None,
            distances: DistanceRing::new(),
            raw_dictionary: dictionary.to_vec(),
            output: Vec::new(),
            output_pos: 0,
            output_base: 0,
            state: State::Reading,
        }
    }

    /// Return the wrapped reader.
    pub fn into_inner(self) -> R {
        self.inner
    }

    fn read_more_encoded(&mut self) -> io::Result<bool> {
        let mut chunk = [0_u8; READ_CHUNK_SIZE];
        let read = self.inner.read(&mut chunk)?;
        if read == 0 {
            return Ok(false);
        }
        self.encoded.extend_from_slice(&chunk[..read]);
        Ok(true)
    }

    fn compact_encoded(&mut self) {
        let bytes = self.bit_pos / 8;
        if bytes == 0 {
            return;
        }
        self.encoded.drain(..bytes);
        self.bit_pos -= bytes * 8;
    }

    fn window_size(&self) -> usize {
        self.window_bits
            .map_or(MIN_STREAM_WINDOW_SIZE, |bits| (1_usize << bits) - 16)
    }

    fn compact_output(&mut self) {
        let keep_from = self.output.len().saturating_sub(self.window_size());
        let drain = self.output_pos.min(keep_from);
        if drain == 0 {
            return;
        }
        self.output.drain(..drain);
        self.output_pos -= drain;
        self.output_base += drain;
    }

    fn decode_next_step(&mut self) -> Result<DecodeStep, DecompressError> {
        let step = match self.try_decode_next_step() {
            Err(BurliError::Format("unexpected end of Brotli input")) => Ok(DecodeStep::NeedMore),
            result => result,
        }?;
        if step != DecodeStep::NeedMore {
            self.compact_encoded();
        }
        Ok(step)
    }

    fn try_decode_next_step(&mut self) -> Result<DecodeStep, DecompressError> {
        let mut reader = BitReader::with_bit_pos(&self.encoded, self.bit_pos)?;
        let window_bits = match self.window_bits {
            Some(window_bits) => window_bits,
            None => {
                let window_bits = stored::read_window_bits(&mut reader)?;
                self.window_bits = Some(window_bits);
                self.bit_pos = reader.consumed_bits();
                window_bits
            }
        };

        let header = stored::read_meta_block_header(&mut reader)?;
        let before_output = self.output.len();
        let mut output = self.output.clone();
        let mut distances = self.distances.clone();

        match header {
            MetaBlockHeader::LastEmpty => {
                stored::finish_stream(&reader)?;
                self.bit_pos = reader.consumed_bits();
                self.state = State::Done;
                Ok(DecodeStep::Done)
            }
            MetaBlockHeader::Metadata { len, is_last } => {
                reader.read_zero_padding_to_byte()?;
                let _metadata = reader.read_aligned_bytes(len)?;
                if is_last {
                    stored::finish_stream(&reader)?;
                    self.state = State::Done;
                }
                self.bit_pos = reader.consumed_bits();
                Ok(if self.state == State::Done {
                    DecodeStep::Done
                } else {
                    DecodeStep::MadeProgress
                })
            }
            MetaBlockHeader::Uncompressed { len } => {
                let needed = output
                    .len()
                    .checked_add(len)
                    .ok_or(BurliError::Format("Brotli output length overflow"))?;
                let global_needed = self
                    .output_base
                    .checked_add(needed)
                    .ok_or(BurliError::Format("Brotli output length overflow"))?;
                if global_needed > self.max_output_size {
                    return Err(BurliError::OutputLimitExceeded {
                        limit: self.max_output_size,
                        needed: global_needed,
                    });
                }
                output.reserve(len);

                reader.read_zero_padding_to_byte()?;
                let bytes = reader.read_aligned_bytes(len)?;
                output.extend_from_slice(bytes);
                self.output = output;
                self.bit_pos = reader.consumed_bits();
                Ok(if self.output.len() > before_output {
                    DecodeStep::MadeOutput
                } else {
                    DecodeStep::NeedMore
                })
            }
            MetaBlockHeader::Compressed { len, is_last } => {
                crate::compressed::decode_meta_block_with_base(
                    &mut reader,
                    &mut output,
                    MetaBlockDecodeParams {
                        output_base: self.output_base,
                        len,
                        max_output_size: self.max_output_size,
                        window_bits,
                        raw_dictionary: crate::dictionary::RawDictionary::new(&self.raw_dictionary),
                    },
                    &mut distances,
                )?;
                if is_last {
                    stored::finish_stream(&reader)?;
                    self.state = State::Done;
                }
                self.output = output;
                self.distances = distances;
                self.bit_pos = reader.consumed_bits();
                Ok(if self.output.len() > before_output {
                    DecodeStep::MadeOutput
                } else if self.state == State::Done {
                    DecodeStep::Done
                } else {
                    DecodeStep::NeedMore
                })
            }
        }
    }

    fn drain_decoded(&mut self, buf: &mut [u8]) -> usize {
        let available = self.output.len() - self.output_pos;
        let count = available.min(buf.len());
        buf[..count].copy_from_slice(&self.output[self.output_pos..self.output_pos + count]);
        self.output_pos += count;
        self.compact_output();
        count
    }
}

impl<R: Read> Read for StreamDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            if self.output_pos < self.output.len() {
                return Ok(self.drain_decoded(buf));
            }
            if self.state == State::Done {
                return Ok(0);
            }

            match self
                .decode_next_step()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            {
                DecodeStep::MadeOutput | DecodeStep::MadeProgress | DecodeStep::Done => {}
                DecodeStep::NeedMore => {
                    if !self.read_more_encoded()? {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            BurliError::Format("unexpected end of Brotli input"),
                        ));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burli_core::Options;

    #[test]
    fn decoder_retains_only_window_history_after_drain() {
        let input = vec![7_u8; (1 << 16) * 4 + 123];
        let options = Options::default()
            .quality(0)
            .unwrap()
            .window_bits(16)
            .unwrap()
            .block_bits(Some(16))
            .unwrap();
        let encoded = burli::compress_with_options(&input, &options).unwrap();
        let mut decoder = StreamDecoder::new(encoded.as_slice());
        let mut decoded = Vec::new();

        decoder.read_to_end(&mut decoded).unwrap();

        assert_eq!(decoded, input);
        assert!(decoder.output.len() <= (1 << 16) - 16);
        assert_eq!(decoder.output_pos, decoder.output.len());
    }
}
