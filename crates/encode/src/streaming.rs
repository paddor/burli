use burli_core::{CompressError, Options};
use std::io::{self, Write};

pub struct StreamEncoder<W> {
    inner: W,
    options: Options,
    buffered: Vec<u8>,
}

impl<W: Write> StreamEncoder<W> {
    pub fn new(inner: W, quality: u8) -> Result<Self, CompressError> {
        Self::with_options(inner, Options::default().quality(quality)?)
    }

    pub fn with_options(inner: W, options: Options) -> Result<Self, CompressError> {
        Ok(Self {
            inner,
            options,
            buffered: Vec::new(),
        })
    }

    pub const fn options(&self) -> &Options {
        &self.options
    }

    pub fn finish(self) -> Result<W, CompressError> {
        let compressed = crate::compress_with_options(&self.buffered, &self.options)?;
        let mut inner = self.inner;
        inner
            .write_all(&compressed)
            .map_err(|_| CompressError::Format("failed to write compressed Brotli stream"))?;
        Ok(inner)
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for StreamEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffered.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
