use std::io::{self, Read};

pub struct StreamDecoder<R> {
    inner: R,
    returned_error: bool,
}

impl<R: Read> StreamDecoder<R> {
    pub const fn new(inner: R) -> Self {
        Self {
            inner,
            returned_error: false,
        }
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for StreamDecoder<R> {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        if self.returned_error {
            return Ok(0);
        }
        self.returned_error = true;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "burli streaming decoder not implemented yet",
        ))
    }
}
