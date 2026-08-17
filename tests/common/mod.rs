use std::io::{self, Read};

pub struct FragmentedRead<'a> {
    pub input: &'a [u8],
    pub pos: usize,
    pub chunk: usize,
}

impl<'a> FragmentedRead<'a> {
    pub const fn new(input: &'a [u8], chunk: usize) -> Self {
        Self {
            input,
            pos: 0,
            chunk,
        }
    }
}

impl Read for FragmentedRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos == self.input.len() {
            return Ok(0);
        }
        let count = buf.len().min(self.chunk).min(self.input.len() - self.pos);
        buf[..count].copy_from_slice(&self.input[self.pos..self.pos + count]);
        self.pos += count;
        Ok(count)
    }
}
