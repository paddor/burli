# frozen_string_literal: true

require "stringio"
require_relative "test_helper"

# burli must decode streams of the brotli gem, write streams it decodes, and
# follow its API.
class TestBurli < Minitest::Test
  def test_brotli_decodes_burli_output
    Payloads::ALL.each do |name, payload|
      (0..11).each do |quality|
        encoded = Burli.deflate(payload, quality: quality)
        assert_equal payload, Brotli.inflate(encoded), "#{name} at quality #{quality}"
      end
    end
  end

  def test_deflate_takes_brotli_options
    payload = Payloads::ALL[:mixed]
    [[:text, 16, 0], [:font, 10, 16], [:generic, 24, 24]].each do |mode, lgwin, lgblock|
      encoded = Burli.deflate(payload, mode: mode, quality: 4, lgwin: lgwin, lgblock: lgblock)
      assert_equal payload, Brotli.inflate(encoded)
    end
    assert_equal payload, Brotli.inflate(Burli.deflate(payload, {quality: 1}))
  end

  def test_decodes_brotli_output
    Payloads::ALL.each do |name, payload|
      (0..11).each do |quality|
        [10, 22].each do |lgwin|
          payload = payload.byteslice(0, 10_000) if quality >= 10
          encoded = Brotli.deflate(payload, quality: quality, lgwin: lgwin)
          assert_equal payload, Burli.inflate(encoded), "#{name} at quality #{quality}"
          decompressor = Burli::Decompressor.new
          decoded = Payloads.pieces(encoded, quality, 999).map { |piece| decompressor.process(piece) }.join
          assert_equal payload, decoded
          assert decompressor.finished?
        end
      end
    end
  end

  def test_output_is_binary
    assert_equal Encoding::BINARY, Burli.deflate("text").encoding
    assert_equal Encoding::BINARY, Burli.inflate(Burli.deflate("text")).encoding
    assert_equal Encoding::BINARY, Burli::Decompressor.new.process(Burli.deflate("text")).encoding
  end

  def test_compressor_writes_streams_in_pieces
    payload = Payloads::ALL[:mixed]
    [0, 1, 5, 11].each do |quality|
      compressor = Burli::Compressor.new(quality: quality)
      encoded = Payloads.pieces(payload, 1, 5000).map { |piece| compressor.process(piece) }.join
      encoded << compressor.finish
      assert compressor.finished?
      assert_equal payload, Brotli.inflate(encoded)
    end
  end

  def test_compressor_flush_makes_input_so_far_decodable
    payload = Payloads::ALL[:text]
    compressor = Burli::Compressor.new(quality: 5)
    decompressor = Brotli::Decompressor.new
    decoded = +""
    Payloads.pieces(payload, 2, 20_000).each do |piece|
      decoded << decompressor.process(compressor.process(piece) + compressor.flush)
      assert payload.start_with?(decoded)
      assert_operator decoded.bytesize, :>, 0
    end
    decoded << decompressor.process(compressor.finish)
    assert_equal payload, decoded
    assert decompressor.finished?
  end

  def test_compressor_takes_no_input_after_finish
    compressor = Burli::Compressor.new
    compressor.finish
    assert_raises(Burli::Error) { compressor.process("x") }
    assert_raises(Burli::Error) { compressor.flush }
    assert_equal "", compressor.finish
  end

  def test_rejects_invalid_options
    [{mode: :other}, {mode: 1}, {quality: 12}, {quality: -1}, {quality: "5"}, {lgwin: 9},
     {lgwin: 25}, {lgblock: 15}].each do |options|
      assert_raises(ArgumentError) { Burli.deflate("x", **options) }
      assert_raises(ArgumentError) { Burli::Compressor.new(options) }
    end
    assert_raises(ArgumentError) { Burli.deflate(nil) }
    assert_raises(TypeError) { Burli.deflate(5) }
  end

  def test_decodes_with_a_raw_dictionary
    skip "needs the brotli gem" if Brotli.equal?(Burli)
    dictionary = Payloads.text(10_000)
    payload = Payloads.text(20_000)
    encoded = Brotli.deflate(payload, dictionary: dictionary)
    assert_operator encoded.bytesize, :<, Brotli.deflate(payload).bytesize
    assert_equal payload, Burli.inflate(encoded, dictionary: dictionary)
    decompressor = Burli::Decompressor.new(dictionary: dictionary)
    assert_equal payload, Payloads.pieces(encoded, 3, 500).map { |piece| decompressor.process(piece) }.join
    assert_raises(Burli::Error) { Burli.inflate(encoded) }
    assert_raises(ArgumentError) { Burli.deflate(payload, dictionary: dictionary) }
    assert_raises(ArgumentError) { Burli::Compressor.new(dictionary: dictionary) }
  end

  def test_output_buffer_limit_bounds_each_call
    payload = Payloads::ALL[:zeros] + Payloads::ALL[:text]
    encoded = Brotli.deflate(payload, quality: 11)
    [1, 1000, 65_536].each do |limit|
      decompressor = Burli::Decompressor.new
      decoded = +""
      # The loop the brotli gem README shows.
      Payloads.pieces(encoded, limit, 4000).each do |piece|
        chunk = decompressor.process(piece, output_buffer_limit: limit)
        assert_operator chunk.bytesize, :<=, limit
        decoded << chunk
        until decompressor.can_accept_more_data || decompressor.finished?
          chunk = decompressor.process("", output_buffer_limit: limit)
          assert_operator chunk.bytesize, :<=, limit
          # Each call returns output or takes in all held input.
          assert(!chunk.empty? || decompressor.can_accept_more_data || decompressor.finished?)
          decoded << chunk
        end
      end
      assert_equal payload, decoded
      assert decompressor.finished?
    end
  end

  def test_rejects_invalid_output_buffer_limits
    decompressor = Burli::Decompressor.new
    assert_raises(ArgumentError) { decompressor.process("", output_buffer_limit: 0) }
    assert_raises(TypeError) { decompressor.process("", output_buffer_limit: "1") }
  end

  def test_rejects_input_while_it_holds_input
    encoded = Brotli.deflate(Payloads::ALL[:text])
    decompressor = Burli::Decompressor.new
    decompressor.process(encoded, output_buffer_limit: 100)
    refute decompressor.can_accept_more_data?
    assert_raises(Burli::Error) { decompressor.process("x") }
    # The decompressor still works.
    decoded = decompressor.process("")
    assert_equal Payloads::ALL[:text].byteslice(100..), decoded
    assert decompressor.finished?
  end

  def test_data_after_the_stream
    encoded = Brotli.deflate("payload")
    assert_raises(Burli::Error) { Burli.inflate(encoded + "x") }
    decompressor = Burli::Decompressor.new
    assert_equal "payload", decompressor.process(encoded + "next")
    assert decompressor.finished?
    refute decompressor.can_accept_more_data
    assert_equal "next", decompressor.unused_data
    assert_equal "", decompressor.process("")
    assert_raises(Burli::Error) { decompressor.process("x") }
  end

  def test_rejects_malformed_streams
    # Reserved bits set, and a large-window header.
    ["\xff\xff\xff".b, "\x11\x00".b].each do |corrupt|
      assert_raises(Burli::Error) { Burli.inflate(corrupt) }
      decompressor = Burli::Decompressor.new
      assert_raises(Burli::Error) { decompressor.process(corrupt) }
      assert_raises(Burli::Error) { decompressor.process("") }
    end
  end

  def test_truncated_stream_is_not_finished
    payload = Payloads::ALL[:text]
    encoded = Brotli.deflate(payload)
    assert_raises(Burli::Error) { Burli.inflate(encoded.byteslice(0...-5)) }
    decompressor = Burli::Decompressor.new
    decoded = decompressor.process(encoded.byteslice(0...-5))
    assert payload.start_with?(decoded)
    refute decompressor.finished?
  end

  def test_inflate_reads_ios
    payload = Payloads::ALL[:text]
    assert_equal payload, Burli.inflate(StringIO.new(Brotli.deflate(payload)))
  end

  def test_writer_and_reader
    payload = Payloads::ALL[:mixed]
    io = StringIO.new(+"".b)
    writer = Burli::Writer.new(io, quality: 4)
    Payloads.pieces(payload, 4, 3000).each { |piece| assert_equal piece.bytesize, writer.write(piece) }
    assert_same writer, writer.flush
    assert_same io, writer.close
    assert writer.closed?
    assert io.closed?
    assert_equal payload, Brotli.inflate(io.string)

    reader = Burli::Reader.new(StringIO.new(io.string))
    assert_equal payload.byteslice(0, 10), reader.read(10)
    assert_equal payload.byteslice(10, 1), reader.readpartial(1)
    buffer = +"old"
    assert_same buffer, reader.read(5, buffer)
    assert_equal payload.byteslice(11, 5), buffer
    refute reader.eof?
    assert_equal payload.byteslice(16..), reader.read
    assert reader.eof?
    assert_nil reader.read(1)
    assert_equal "", reader.read
    assert_raises(EOFError) { reader.readpartial(1) }
    reader.close
    assert_raises(Burli::Error) { reader.read }
  end

  def test_reader_reads_brotli_streams_in_small_reads
    payload = Payloads::ALL[:text]
    reader = Burli::Reader.new(StringIO.new(Brotli.deflate(payload, quality: 11)))
    decoded = +""
    while (chunk = reader.read(777))
      decoded << chunk
    end
    assert_equal payload, decoded
  end

  def test_reader_rejects_truncated_streams
    encoded = Brotli.deflate(Payloads::ALL[:text])
    reader = Burli::Reader.new(StringIO.new(encoded.byteslice(0...-5)))
    assert_raises(Burli::Error) { reader.read }
  end

  def test_works_in_threads
    # Threads share one String, which only one of them can lock.
    payload = Payloads::ALL[:mixed].dup
    threads = 8.times.map do |quality|
      Thread.new { [Burli.inflate(Burli.deflate(payload, quality: quality)), $!] }
    end
    threads.each { |thread| assert_equal [payload, nil], thread.value }
    payload << "unlocked"
  end

  def test_threads_wait_for_a_busy_decompressor
    payload = Payloads::ALL[:text] * 40
    decompressor = Burli::Decompressor.new
    worker = Thread.new { decompressor.process(Brotli.deflate(payload)) }
    # Waits while the worker decodes without the GVL, and does not raise.
    decompressor.finished? until worker.join(0)
    assert_equal payload, worker.value
    assert decompressor.finished?
  end

  def test_works_in_ractors
    skip "TruffleRuby has no Ractors" if RUBY_ENGINE == "truffleruby"
    ractor = Ractor.new(Payloads::ALL[:text]) { |payload| Burli.inflate(Burli.deflate(payload)) == payload }
    assert_equal true, ractor.respond_to?(:value) ? ractor.value : ractor.take
  end

  def test_matches_brotli_gem_names
    %i[deflate inflate version].each { |name| assert_respond_to Burli, name }
    %i[process flush finish finished?].each do |name|
      assert_includes Burli::Compressor.public_instance_methods, name
    end
    %i[process finished? is_finished can_accept_more_data can_accept_more_data? unused_data].each do |name|
      assert_includes Burli::Decompressor.public_instance_methods, name
    end
    assert_operator Burli::Error, :<, StandardError
  end
end
