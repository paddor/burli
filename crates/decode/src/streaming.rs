use std::io::{self, Cursor, Read};

pub struct StreamDecoder<R> {
    inner: R,
    decoded: Option<Cursor<Vec<u8>>>,
}

impl<R: Read> StreamDecoder<R> {
    pub const fn new(inner: R) -> Self {
        Self {
            inner,
            decoded: None,
        }
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for StreamDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.decoded.is_none() {
            let mut encoded = Vec::new();
            self.inner.read_to_end(&mut encoded)?;
            let decoded = crate::decompress(&encoded)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            self.decoded = Some(Cursor::new(decoded));
        }

        self.decoded.as_mut().expect("decoded above").read(buf)
    }
}
