use burli_core::{
    CompressError, Options,
    bits::BitWriter,
    format::{MAX_BLOCK_BITS, MIN_BLOCK_BITS},
};
use std::io::{self, Write};

pub struct StreamEncoder<W> {
    inner: W,
    options: Options,
    writer: BitWriter,
    workspace: crate::encode::Workspace,
    buffered: Vec<u8>,
    block_size: usize,
}

impl<W: Write> StreamEncoder<W> {
    pub fn new(inner: W, quality: u8) -> Result<Self, CompressError> {
        Self::with_options(inner, Options::default().quality(quality)?)
    }

    pub fn with_options(inner: W, options: Options) -> Result<Self, CompressError> {
        let block_bits = options.block_bits_value().unwrap_or(MIN_BLOCK_BITS);
        if !(MIN_BLOCK_BITS..=MAX_BLOCK_BITS).contains(&block_bits) {
            return Err(CompressError::InvalidBlockBits(block_bits));
        }
        let mut writer = BitWriter::new();
        if options.quality_value() == 0 {
            crate::metablock::write_window_bits(&mut writer, options.window_bits_value())?;
        } else {
            crate::encode::write_stream_header(&mut writer, &options)?;
        }

        Ok(Self {
            inner,
            options,
            writer,
            workspace: crate::encode::Workspace::default(),
            buffered: Vec::new(),
            block_size: 1_usize << block_bits,
        })
    }

    pub const fn options(&self) -> &Options {
        &self.options
    }

    pub fn finish(mut self) -> Result<W, CompressError> {
        self.write_buffered_chunk()
            .map_err(|_| CompressError::Format("failed to write compressed Brotli stream"))?;
        crate::metablock::write_last_empty_meta_block(&mut self.writer)?;
        self.write_final_bytes()
            .map_err(|_| CompressError::Format("failed to write compressed Brotli stream"))?;
        Ok(self.inner)
    }

    pub fn into_inner(self) -> W {
        self.inner
    }

    fn write_meta_block(&mut self, input: &[u8]) -> io::Result<()> {
        if input.is_empty() {
            return Ok(());
        }
        let result = if self.options.quality_value() == 0 {
            crate::metablock::write_uncompressed_meta_block(&mut self.writer, input)
        } else {
            crate::encode::write_stream_chunk_with_workspace(
                &mut self.writer,
                input,
                &self.options,
                &mut self.workspace,
            )
        };
        result.map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.write_full_bytes()
    }

    fn write_buffered_chunk(&mut self) -> io::Result<()> {
        if self.buffered.is_empty() {
            return Ok(());
        }
        let chunk = core::mem::take(&mut self.buffered);
        self.write_meta_block(&chunk)
    }

    fn write_full_bytes(&mut self) -> io::Result<()> {
        let bytes = self.writer.take_full_bytes();
        if bytes.is_empty() {
            return Ok(());
        }
        self.inner.write_all(&bytes)
    }

    fn write_final_bytes(&mut self) -> io::Result<()> {
        let bytes = core::mem::take(&mut self.writer).into_bytes();
        if bytes.is_empty() {
            return Ok(());
        }
        self.inner.write_all(&bytes)
    }
}

impl<W: Write> Write for StreamEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffered.extend_from_slice(buf);
        while self.buffered.len() >= self.block_size {
            let chunk: Vec<u8> = self.buffered.drain(..self.block_size).collect();
            self.write_meta_block(&chunk)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn stream_encoder_keeps_only_tail_buffer() {
        let input = vec![3_u8; (1 << 16) * 2 + 7];
        let mut encoder = StreamEncoder::new(Vec::new(), 0).unwrap();

        encoder.write_all(&input).unwrap();

        assert_eq!(encoder.buffered.len(), 7);
        let encoded = encoder.finish().unwrap();
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn stream_encoder_q5_round_trips_multiple_chunks() {
        let input = b"abcdefghijklmnopqrstuvwxyz0123456789".repeat(4096);
        let mut encoder = StreamEncoder::new(Vec::new(), 5).unwrap();

        encoder.write_all(&input[..10_000]).unwrap();
        encoder.write_all(&input[10_000..]).unwrap();
        let encoded = encoder.finish().unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }
}
