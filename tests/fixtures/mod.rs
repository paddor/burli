use burli_core::bits::BitWriter;

/// A final compressed block producing 'A', with optional unused final trees.
pub fn literal_with_unused_trees(unused_literal: bool, unused_distance: bool) -> Vec<u8> {
    let mut writer = BitWriter::new();
    // Window 16, final compressed block, decoded length one.
    for (width, value) in [(1, 0), (1, 1), (1, 0), (2, 0), (16, 0)] {
        writer.write_bits(width, value).unwrap();
    }
    // One block type per category, no postfix/direct distances, LSB6 contexts.
    writer.write_bits(3, 0).unwrap();
    writer.write_bits(6, 0).unwrap();
    writer.write_bits(2, 0).unwrap();
    write_context_map(&mut writer, unused_literal);
    write_context_map(&mut writer, unused_distance);
    write_single_symbol(&mut writer, 8, u64::from(b'A'));
    if unused_literal {
        write_single_symbol(&mut writer, 8, u64::from(b'B'));
    }
    write_single_symbol(&mut writer, 10, 8); // Insert one byte; no copy needed.
    write_single_symbol(&mut writer, 6, 0);
    if unused_distance {
        write_single_symbol(&mut writer, 6, 1);
    }
    writer.into_bytes()
}

/// Distance block switches that each read 54 bits: a 15-bit type code, a
/// 15-bit count code, and 24 count extra bits. The command that reads each
/// switch takes a code of 1 to 15 bits first, so a decoder that buffers
/// fewer bits per command meets the switch short of bits, as in mbrotli
/// GHSA-gf34-947h-27j2. Returns the stream and its decoded bytes.
pub fn max_width_distance_block_switches() -> (Vec<u8>, Vec<u8>) {
    // Block 0, then one block after each switch.
    const BLOCKS: u64 = 17;
    // The least block count of count code 25.
    const MIN_COUNT: u64 = 16_625;

    // Commands as command code, distance code bit, and whether a switch
    // precedes the distance. Block `b` holds `MIN_COUNT + b` commands. The
    // first one reads the switch and has command code `127 + b`; the others
    // have command code 128.
    let mut commands = Vec::new();
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    for block in 0..BLOCKS {
        for index in 0..MIN_COUNT + block {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let switch = block != 0 && index == 0;
            let command = if switch { 127 + block } else { 128 };
            commands.push((command, state >> 63, switch));
        }
    }
    // Commands 128 to 135 copy 2 to 9 bytes from distance 1 or 2. Commands
    // 136 to 143 insert "a" first.
    let mut expected = b"ab".to_vec();
    for &(command, bit, _) in &commands {
        if command >= 136 {
            expected.push(b'a');
        }
        for _ in 0..(command & 7) + 2 {
            expected.push(expected[expected.len() - 1 - bit as usize]);
        }
    }

    let mut writer = BitWriter::new();
    // Window 16, then an uncompressed block "ab" for the copies to reach.
    for (width, value) in [(1, 0), (1, 0), (2, 0), (16, 1), (1, 1)] {
        writer.write_bits(width, value).unwrap();
    }
    writer.write_aligned_bytes(b"ab").unwrap();
    // Final compressed block of five length nibbles. One literal and one
    // command block type, 14 distance block types.
    let len = expected.len() as u64 - 2;
    assert!((1 << 16..1 << 20).contains(&(len - 1)));
    for (width, value) in [(1, 1), (1, 0), (2, 1), (20, len - 1)] {
        writer.write_bits(width, value).unwrap();
    }
    for (width, value) in [(1, 0), (1, 0), (1, 1), (3, 3), (3, 5)] {
        writer.write_bits(width, value).unwrap();
    }
    // Type codes 0 and 1 take 15 bits, and count codes 14 and 25.
    let mut type_lengths = vec![15, 15];
    type_lengths.extend(1..=14);
    let type_codes = write_complex_code(&mut writer, &type_lengths);
    let mut count_lengths: Vec<u8> = (1..=15).collect();
    count_lengths.extend([0; 10]);
    count_lengths.push(15);
    let count_codes = write_complex_code(&mut writer, &count_lengths);
    write_code(&mut writer, count_codes[25]);
    writer.write_bits(24, 0).unwrap();
    // No postfix, two direct distances, LSB6 literal context, one literal
    // and one distance tree.
    for (width, value) in [(2, 0), (4, 2), (2, 0), (1, 0), (1, 0)] {
        writer.write_bits(width, value).unwrap();
    }
    write_single_symbol(&mut writer, 8, u64::from(b'a'));
    // Command codes 128 to 141 take 1 to 14 bits, 142 and 143 15 bits.
    let mut command_lengths = vec![0; 128];
    command_lengths.extend(1..=15);
    command_lengths.push(15);
    let command_codes = write_complex_code(&mut writer, &command_lengths);
    // Distance codes 16 and 17 of one bit each: direct distances 1 and 2.
    for (width, value) in [(2, 1), (2, 1), (7, 16), (7, 17)] {
        writer.write_bits(width, value).unwrap();
    }

    let mut block = 0;
    for (command, bit, switch) in commands {
        write_code(&mut writer, command_codes[command as usize]);
        if switch {
            // Type code 1 selects the next block type.
            block += 1;
            write_code(&mut writer, type_codes[1]);
            write_code(&mut writer, count_codes[25]);
            writer.write_bits(24, block).unwrap();
        }
        writer.write_bits(1, bit).unwrap();
    }
    (writer.into_bytes(), expected)
}

/// Writes a complex prefix code with code lengths `lengths`, and returns
/// the code of each symbol as width and bits in write order. Its code-length
/// code gives lengths 0 to 15 four bits each.
fn write_complex_code(writer: &mut BitWriter, lengths: &[u8]) -> Vec<(u8, u64)> {
    writer.write_bits(2, 0).unwrap(); // Skip no code-length code lengths.
    for symbol in [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15] {
        // Length 4 is written "01", length 0 "00".
        writer.write_bits(2, u64::from(symbol < 16)).unwrap();
    }
    for &len in lengths {
        // The four-bit code of `len` is `len`, sent from its high bit.
        writer
            .write_bits(4, u64::from(len.reverse_bits() >> 4))
            .unwrap();
    }

    // Canonical codes, sent from their high bit.
    let mut next = [0_u64; 16];
    let mut code = 0;
    for (len, first) in next.iter_mut().enumerate().skip(1) {
        *first = code;
        code = (code + lengths.iter().filter(|&&l| usize::from(l) == len).count() as u64) << 1;
    }
    lengths
        .iter()
        .map(|&len| {
            if len == 0 {
                return (0, 0);
            }
            let code = next[usize::from(len)];
            next[usize::from(len)] += 1;
            (len, code.reverse_bits() >> (64 - len))
        })
        .collect()
}

fn write_code(writer: &mut BitWriter, (width, bits): (u8, u64)) {
    writer.write_bits(width, bits).unwrap();
}

fn write_single_symbol(writer: &mut BitWriter, width: u8, symbol: u64) {
    writer.write_bits(2, 1).unwrap();
    writer.write_bits(2, 0).unwrap();
    writer.write_bits(width, symbol).unwrap();
}

fn write_context_map(writer: &mut BitWriter, unused_tree: bool) {
    writer.write_bits(1, u64::from(unused_tree)).unwrap();
    if unused_tree {
        writer.write_bits(3, 0).unwrap(); // Declare two trees.
        writer.write_bits(1, 0).unwrap(); // No RLE.
        write_single_symbol(writer, 1, 0); // Every context selects tree zero.
        writer.write_bits(1, 0).unwrap(); // No MTF.
    }
}
