use std::io::{self, Cursor, Read};

pub struct StreamDecoder<R> {
    inner: R,
    max_output_size: usize,
    decoded: Option<Cursor<Vec<u8>>>,
}

impl<R: Read> StreamDecoder<R> {
    pub const fn new(inner: R) -> Self {
        Self::with_limit(inner, burli_core::format::DEFAULT_MAX_OUTPUT_SIZE)
    }

    pub const fn with_limit(inner: R, max_output_size: usize) -> Self {
        Self {
            inner,
            max_output_size,
            decoded: None,
        }
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for StreamDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        if self.decoded.is_none() {
            let mut encoded = Vec::new();
            self.inner.read_to_end(&mut encoded)?;
            let decoded = crate::decompress_with_limit(&encoded, self.max_output_size)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            self.decoded = Some(Cursor::new(decoded));
        }

        let Some(decoded) = self.decoded.as_mut() else {
            return Err(io::Error::other("Brotli stream decoder did not initialize"));
        };
        decoded.read(buf)
    }
}
