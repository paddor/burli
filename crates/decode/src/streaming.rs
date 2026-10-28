use std::io::{self, Read};

use burli_core::{BurliError, DecompressError};

use crate::push::{DecodeStatus, PushDecoder};

const READ_CHUNK_SIZE: usize = 8 * 1024;

/// `std::io::Read` Brotli stream decoder.
///
/// The decoder returns bytes as soon as it decodes them, also within a
/// meta-block, and keeps only the decoded window history needed for future
/// backward copies. It runs a [`PushDecoder`] on input read from the
/// wrapped reader.
pub struct StreamDecoder<R> {
    inner: R,
    decoder: PushDecoder,
    /// Input read from `inner` that the decoder has not taken.
    input: Vec<u8>,
    input_pos: usize,
}

/// Buffered input returned when a stream decoder cannot safely unwrap its
/// reader without losing bytes.
///
/// The buffered bytes precede the wrapped reader's future bytes. Consume them
/// first when reconstructing the unread input stream.
pub struct IntoInnerError<R> {
    inner: R,
    buffered: Vec<u8>,
}

impl<R> IntoInnerError<R> {
    /// Recover the wrapped reader and bytes already read into the decoder.
    pub fn into_parts(self) -> (R, Vec<u8>) {
        (self.inner, self.buffered)
    }
}

impl<R: Read> StreamDecoder<R> {
    /// Create a stream decoder with no practical output limit.
    pub fn new(inner: R) -> Self {
        Self::with_decoder(inner, PushDecoder::new())
    }

    /// Create a stream decoder with a hard output limit.
    pub fn with_limit(inner: R, max_output_size: usize) -> Self {
        Self::with_options(
            inner,
            &crate::Options::new().with_max_output_size(max_output_size),
        )
    }

    /// Create a stream decoder with explicit [`crate::Options`].
    pub fn with_options(inner: R, options: &crate::Options) -> Self {
        Self::with_decoder(inner, PushDecoder::with_options(options))
    }

    /// Create a stream decoder with a raw LZ77 prefix dictionary.
    pub fn with_raw_dictionary(inner: R, dictionary: crate::RawDictionary) -> Self {
        Self::with_decoder(inner, PushDecoder::with_raw_dictionary(dictionary))
    }

    /// Create a stream decoder with a raw LZ77 prefix dictionary and hard
    /// output limit.
    pub fn with_raw_dictionary_and_limit(
        inner: R,
        dictionary: crate::RawDictionary,
        max_output_size: usize,
    ) -> Self {
        Self::with_raw_dictionary_and_options(
            inner,
            dictionary,
            &crate::Options::new().with_max_output_size(max_output_size),
        )
    }

    /// Create a stream decoder with a raw LZ77 prefix dictionary and explicit
    /// [`crate::Options`].
    pub fn with_raw_dictionary_and_options(
        inner: R,
        dictionary: crate::RawDictionary,
        options: &crate::Options,
    ) -> Self {
        Self::with_decoder(
            inner,
            PushDecoder::with_raw_dictionary_and_options(dictionary, options),
        )
    }

    const fn with_decoder(inner: R, decoder: PushDecoder) -> Self {
        Self {
            inner,
            decoder,
            input: Vec::new(),
            input_pos: 0,
        }
    }

    /// Return the wrapped reader after the Brotli stream is fully consumed.
    ///
    /// Returns buffered bytes separately when the decoder read ahead or when
    /// decoding is incomplete. This prevents trailing input from
    /// disappearing. On error, call [`IntoInnerError::into_parts`] and consume
    /// the returned bytes before reading from the wrapped reader.
    pub fn into_inner(self) -> Result<R, IntoInnerError<R>> {
        let unread = &self.input[self.input_pos..];
        if self.decoder.is_finished() {
            if unread.is_empty() {
                return Ok(self.inner);
            }
            return Err(IntoInnerError {
                buffered: unread.to_vec(),
                inner: self.inner,
            });
        }

        Err(IntoInnerError {
            buffered: [self.decoder.pending_input(), unread].concat(),
            inner: self.inner,
        })
    }

    /// Return current decode options.
    pub fn options(&self) -> crate::Options {
        self.decoder.options()
    }

    /// Return the configured raw LZ77 prefix dictionary.
    pub const fn raw_dictionary(&self) -> &crate::RawDictionary {
        self.decoder.raw_dictionary()
    }

    /// Reads more input after the decoder took all of it. Returns `false`
    /// at the end of the wrapped reader.
    fn read_more_input(&mut self) -> io::Result<bool> {
        self.input.resize(READ_CHUNK_SIZE, 0);
        self.input_pos = 0;
        match self.inner.read(&mut self.input) {
            Ok(read) => {
                self.input.truncate(read);
                Ok(read != 0)
            }
            Err(error) => {
                self.input.clear();
                Err(error)
            }
        }
    }
}

fn invalid_data(error: DecompressError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

impl<R: Read> Read for StreamDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            let progress = self
                .decoder
                .decompress(&self.input[self.input_pos..], buf)
                .map_err(invalid_data)?;
            self.input_pos += progress.consumed;
            if progress.written != 0 {
                return Ok(progress.written);
            }
            match progress.status {
                DecodeStatus::Finished => return Ok(0),
                DecodeStatus::NeedsOutput => {}
                DecodeStatus::NeedsInput => {
                    if !self.read_more_input()? {
                        return Err(invalid_data(BurliError::Format(
                            "unexpected end of Brotli input",
                        )));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One literal 'A' in a final compressed block, with two padding bits.
    const FINAL_COMPRESSED: &[u8] = &[0x02, 0, 0, 0, 0x44, 0x50, 0x20, 0x10, 0];

    struct Chunked<R> {
        inner: R,
        chunk: usize,
    }

    impl<R: Read> Read for Chunked<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let count = buf.len().min(self.chunk);
            self.inner.read(&mut buf[..count])
        }
    }

    #[test]
    fn final_blocks_preserve_following_data() {
        for (encoded, expected) in [
            (FINAL_COMPRESSED, b"A".as_slice()),
            (&[0x1a][..], b"".as_slice()), // Final empty metadata block.
            (&[0x06][..], b"".as_slice()), // Final empty block.
        ] {
            assert_eq!(crate::decompress(encoded).unwrap(), expected);
            let mut input = encoded.to_vec();
            input.extend_from_slice(b"next frame");
            assert!(crate::decompress(&input).is_err());

            for chunk in [1, 2, input.len()] {
                let source = Chunked {
                    inner: input.as_slice(),
                    chunk,
                };
                let mut decoder = StreamDecoder::new(source);
                let mut output = Vec::new();
                decoder.read_to_end(&mut output).unwrap();
                assert_eq!(output, expected);
                let (mut inner, mut unread) = match decoder.into_inner() {
                    Ok(inner) => (inner, Vec::new()),
                    Err(error) => error.into_parts(),
                };
                inner.read_to_end(&mut unread).unwrap();
                assert_eq!(unread, b"next frame");
            }
        }
    }

    #[test]
    fn final_blocks_still_reject_nonzero_padding() {
        for encoded in [FINAL_COMPRESSED, &[0x06]] {
            let mut input = encoded.to_vec();
            *input.last_mut().unwrap() |= 0x80;
            input.extend_from_slice(b"next frame");
            let mut decoder = StreamDecoder::new(input.as_slice());
            let error = decoder.read_to_end(&mut Vec::new()).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(error.to_string(), "non-zero trailing Brotli padding");
        }
    }
}
