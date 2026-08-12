#![no_main]

use std::io::{self, Read, Write};

use libfuzzer_sys::fuzz_target;

struct FragmentedRead<'a> {
    input: &'a [u8],
    pos: usize,
    chunk: usize,
}

impl Read for FragmentedRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos == self.input.len() {
            return Ok(0);
        }
        let count = buf
            .len()
            .min(self.chunk)
            .min(self.input.len() - self.pos);
        buf[..count].copy_from_slice(&self.input[self.pos..self.pos + count]);
        self.pos += count;
        Ok(count)
    }
}

fuzz_target!(|input: &[u8]| {
    if input.len() < 2 {
        return;
    }

    let quality = input[0] % 6;
    let chunk = usize::from(input[1] % 32) + 1;
    let payload = &input[2..];

    let mut encoder = burli::StreamEncoder::new(Vec::new(), quality).unwrap();
    for part in payload.chunks(chunk) {
        encoder.write_all(part).unwrap();
    }
    let encoded = encoder.finish().unwrap();

    let source = FragmentedRead {
        input: &encoded,
        pos: 0,
        chunk,
    };
    let mut decoder = burli::StreamDecoder::new(source);
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).unwrap();

    assert_eq!(decoded, payload);
});
