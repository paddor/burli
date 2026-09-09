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
