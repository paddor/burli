use std::io::{self, Read};

use burli_core::{BurliError, DecompressError, bits::BitReader};

use crate::{
    compressed::DistanceRing,
    stored::{self, MetaBlockHeader},
};

const READ_CHUNK_SIZE: usize = 8 * 1024;

pub struct StreamDecoder<R> {
    inner: R,
    max_output_size: usize,
    encoded: Vec<u8>,
    bit_pos: usize,
    window_bits: Option<u8>,
    distances: DistanceRing,
    output: Vec<u8>,
    output_pos: usize,
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
    MadeOutput,
    Done,
}

impl<R: Read> StreamDecoder<R> {
    pub const fn new(inner: R) -> Self {
        Self::with_limit(inner, burli_core::format::DEFAULT_MAX_OUTPUT_SIZE)
    }

    pub const fn with_limit(inner: R, max_output_size: usize) -> Self {
        Self {
            inner,
            max_output_size,
            encoded: Vec::new(),
            bit_pos: 0,
            window_bits: None,
            distances: DistanceRing::new(),
            output: Vec::new(),
            output_pos: 0,
            state: State::Reading,
        }
    }

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

    fn decode_next_step(&mut self) -> Result<DecodeStep, DecompressError> {
        match self.try_decode_next_step() {
            Err(BurliError::Format("unexpected end of Brotli input")) => Ok(DecodeStep::NeedMore),
            result => result,
        }
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
                    DecodeStep::NeedMore
                })
            }
            MetaBlockHeader::Uncompressed { len } => {
                let needed = output.len().saturating_add(len);
                if needed > self.max_output_size {
                    return Err(BurliError::OutputLimitExceeded {
                        limit: self.max_output_size,
                        needed,
                    });
                }

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
                crate::compressed::decode_meta_block(
                    &mut reader,
                    &mut output,
                    len,
                    self.max_output_size,
                    window_bits,
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
                DecodeStep::MadeOutput => continue,
                DecodeStep::Done => continue,
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
