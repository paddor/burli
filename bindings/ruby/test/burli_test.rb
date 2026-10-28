# frozen_string_literal: true

require_relative "test_helper"

# burli must decode streams of the brotli gem, write streams it decodes, and
# follow its API.
class BurliTest < Minitest::Spec
  WORDS = ["brotli ", "window ", "literal ", "context ", "copy ", "the\n"].freeze

  def self.text(size)
    random = Random.new(size)
    out = +""
    out << WORDS.sample(random: random) while out.bytesize < size
    out.byteslice(0, size).b
  end

  def self.noise(size)
    Random.new(size).bytes(size)
  end

  PAYLOADS = {
    empty: "".b,
    byte: "x".b,
    text: text(100_000),
    noise: noise(50_000),
    zeros: ("\0" * 200_000).b,
    mixed: text(30_000) + noise(20_000) + text(30_000)
  }.freeze
  TEXT = PAYLOADS[:text]
  MIXED = PAYLOADS[:mixed]
  # Reserved bits set, and a large-window header.
  MALFORMED = ["\xff\xff\xff".b, "\x11\x00".b].freeze
  INVALID_OPTIONS = [
    {mode: :other}, {mode: 1}, {quality: 12}, {quality: -1}, {quality: "5"},
    {lgwin: 9}, {lgwin: 25}, {lgblock: 15}
  ].freeze

  describe ".deflate" do
    it "writes streams the brotli gem decodes at every quality" do
      PAYLOADS.each do |name, payload|
        (0..11).each do |quality|
          encoded = Burli.deflate(payload, quality: quality)
          assert_equal payload, Brotli.inflate(encoded), "#{name} at quality #{quality}"
        end
      end
    end

    it "takes the options of the brotli gem" do
      [[:text, 16, 0], [:font, 10, 16], [:generic, 24, 24]].each do |mode, lgwin, lgblock|
        encoded = Burli.deflate(MIXED, mode: mode, quality: 4, lgwin: lgwin, lgblock: lgblock)
        assert_equal MIXED, Brotli.inflate(encoded)
      end
      assert_equal MIXED, Brotli.inflate(Burli.deflate(MIXED, {quality: 1}))
    end

    it "rejects invalid options" do
      INVALID_OPTIONS.each do |options|
        assert_raises(ArgumentError) { Burli.deflate("x", **options) }
      end
      assert_raises(ArgumentError) { Burli.deflate(nil) }
      assert_raises(TypeError) { Burli.deflate(5) }
    end

    it "rejects dictionaries" do
      assert_raises(ArgumentError) { Burli.deflate("x", dictionary: "dictionary") }
    end

    it "returns binary strings" do
      assert_equal Encoding::BINARY, Burli.deflate("text").encoding
    end
  end

  describe ".inflate" do
    it "decodes streams of the brotli gem" do
      each_brotli_stream do |payload, encoded|
        assert_equal payload, Burli.inflate(encoded)
      end
    end

    it "reads IOs" do
      assert_equal TEXT, Burli.inflate(StringIO.new(Brotli.deflate(TEXT)))
    end

    it "decodes with a raw dictionary" do
      skip "needs the brotli gem" unless brotli_gem?
      dictionary = self.class.text(10_000)
      payload = self.class.text(20_000)
      encoded = Brotli.deflate(payload, dictionary: dictionary)
      assert_operator encoded.bytesize, :<, Brotli.deflate(payload).bytesize
      assert_equal payload, Burli.inflate(encoded, dictionary: dictionary)
      assert_raises(Burli::Error) { Burli.inflate(encoded) }
    end

    it "rejects data after the stream" do
      assert_raises(Burli::Error) { Burli.inflate(Brotli.deflate("payload") + "x") }
    end

    it "rejects malformed and truncated streams" do
      MALFORMED.each do |corrupt|
        assert_raises(Burli::Error) { Burli.inflate(corrupt) }
      end
      assert_raises(Burli::Error) { Burli.inflate(Brotli.deflate(TEXT).byteslice(0...-5)) }
    end

    it "returns binary strings" do
      assert_equal Encoding::BINARY, Burli.inflate(Burli.deflate("text")).encoding
    end
  end

  describe Burli::Compressor do
    it "writes streams in pieces" do
      [0, 1, 5, 11].each do |quality|
        compressor = Burli::Compressor.new(quality: quality)
        encoded = pieces(MIXED, 1, 5000).map { |piece| compressor.process(piece) }.join
        encoded << compressor.finish
        assert compressor.finished?
        assert_equal MIXED, Brotli.inflate(encoded)
      end
    end

    it "flushes output that decodes to the input so far" do
      compressor = Burli::Compressor.new(quality: 5)
      decompressor = Brotli::Decompressor.new
      decoded = +""
      pieces(TEXT, 2, 20_000).each do |piece|
        decoded << decompressor.process(compressor.process(piece) + compressor.flush)
        assert TEXT.start_with?(decoded)
        refute_empty decoded
      end
      decoded << decompressor.process(compressor.finish)
      assert_equal TEXT, decoded
      assert decompressor.finished?
    end

    it "takes no input after finish" do
      compressor = Burli::Compressor.new
      compressor.finish
      assert_raises(Burli::Error) { compressor.process("x") }
      assert_raises(Burli::Error) { compressor.flush }
      assert_equal "", compressor.finish
    end

    it "rejects invalid options" do
      INVALID_OPTIONS.each do |options|
        assert_raises(ArgumentError) { Burli::Compressor.new(options) }
      end
      assert_raises(ArgumentError) { Burli::Compressor.new(dictionary: "dictionary") }
    end
  end

  describe Burli::Decompressor do
    it "decodes streams of the brotli gem in pieces" do
      each_brotli_stream do |payload, encoded, seed|
        decompressor = Burli::Decompressor.new
        decoded = pieces(encoded, seed, 999).map { |piece| decompressor.process(piece) }.join
        assert_equal payload, decoded
        assert decompressor.finished?
      end
    end

    it "decodes with a raw dictionary" do
      skip "needs the brotli gem" unless brotli_gem?
      dictionary = self.class.text(10_000)
      payload = self.class.text(20_000)
      encoded = Brotli.deflate(payload, dictionary: dictionary)
      decompressor = Burli::Decompressor.new(dictionary: dictionary)
      assert_equal payload, pieces(encoded, 3, 500).map { |piece| decompressor.process(piece) }.join
    end

    it "bounds each call with output_buffer_limit" do
      payload = PAYLOADS[:zeros] + TEXT
      encoded = Brotli.deflate(payload, quality: 11)
      [1, 1000, 65_536].each do |limit|
        decompressor = Burli::Decompressor.new
        decoded = +""
        # The loop the brotli gem README shows.
        pieces(encoded, limit, 4000).each do |piece|
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

    it "rejects invalid output_buffer_limit values" do
      decompressor = Burli::Decompressor.new
      assert_raises(ArgumentError) { decompressor.process("", output_buffer_limit: 0) }
      assert_raises(TypeError) { decompressor.process("", output_buffer_limit: "1") }
    end

    it "rejects input while it holds input" do
      decompressor = Burli::Decompressor.new
      decompressor.process(Brotli.deflate(TEXT), output_buffer_limit: 100)
      refute decompressor.can_accept_more_data?
      assert_raises(Burli::Error) { decompressor.process("x") }
      # The decompressor still works.
      assert_equal TEXT.byteslice(100..), decompressor.process("")
      assert decompressor.finished?
    end

    it "keeps data after the stream as unused_data" do
      decompressor = Burli::Decompressor.new
      assert_equal "payload", decompressor.process(Brotli.deflate("payload") + "next")
      assert decompressor.finished?
      refute decompressor.can_accept_more_data
      assert_equal "next", decompressor.unused_data
      assert_equal "", decompressor.process("")
      assert_raises(Burli::Error) { decompressor.process("x") }
    end

    it "rejects malformed streams" do
      MALFORMED.each do |corrupt|
        decompressor = Burli::Decompressor.new
        assert_raises(Burli::Error) { decompressor.process(corrupt) }
        assert_raises(Burli::Error) { decompressor.process("") }
      end
    end

    it "is not finished after a truncated stream" do
      decompressor = Burli::Decompressor.new
      decoded = decompressor.process(Brotli.deflate(TEXT).byteslice(0...-5))
      assert TEXT.start_with?(decoded)
      refute decompressor.finished?
    end

    it "returns binary strings" do
      assert_equal Encoding::BINARY, Burli::Decompressor.new.process(Burli.deflate("text")).encoding
    end

    it "lets other threads wait while it works" do
      payload = TEXT * 40
      decompressor = Burli::Decompressor.new
      worker = Thread.new { decompressor.process(Brotli.deflate(payload)) }
      # Waits while the worker decodes without the GVL, and does not raise.
      decompressor.finished? until worker.join(0)
      assert_equal payload, worker.value
      assert decompressor.finished?
    end
  end

  describe Burli::Writer do
    it "compresses what it writes into an IO" do
      io = StringIO.new(+"".b)
      writer = Burli::Writer.new(io, quality: 4)
      pieces(MIXED, 4, 3000).each { |piece| assert_equal piece.bytesize, writer.write(piece) }
      assert_same writer, writer.flush
      assert_same io, writer.close
      assert writer.closed?
      assert io.closed?
      assert_equal MIXED, Brotli.inflate(io.string)
    end
  end

  describe Burli::Reader do
    it "reads like an IO" do
      reader = Burli::Reader.new(StringIO.new(Brotli.deflate(MIXED)))
      assert_equal MIXED.byteslice(0, 10), reader.read(10)
      assert_equal MIXED.byteslice(10, 1), reader.readpartial(1)
      buffer = +"old"
      assert_same buffer, reader.read(5, buffer)
      assert_equal MIXED.byteslice(11, 5), buffer
      refute reader.eof?
      assert_equal MIXED.byteslice(16..), reader.read
      assert reader.eof?
      assert_nil reader.read(1)
      assert_equal "", reader.read
      assert_raises(EOFError) { reader.readpartial(1) }
      reader.close
      assert_raises(Burli::Error) { reader.read }
    end

    it "decodes streams of the brotli gem in small reads" do
      reader = Burli::Reader.new(StringIO.new(Brotli.deflate(TEXT, quality: 11)))
      decoded = +""
      while (chunk = reader.read(777))
        decoded << chunk
      end
      assert_equal TEXT, decoded
    end

    it "rejects truncated streams" do
      reader = Burli::Reader.new(StringIO.new(Brotli.deflate(TEXT).byteslice(0...-5)))
      assert_raises(Burli::Error) { reader.read }
    end
  end

  it "works in threads that share a String" do
    # Only one thread at a time can lock the String; the others copy it.
    payload = MIXED.dup
    threads = 8.times.map do |quality|
      Thread.new { [Burli.inflate(Burli.deflate(payload, quality: quality)), $!] }
    end
    threads.each { |thread| assert_equal [payload, nil], thread.value }
    payload << "unlocked"
  end

  it "works in Ractors" do
    skip "TruffleRuby has no Ractors" if RUBY_ENGINE == "truffleruby"
    ractor = Ractor.new(TEXT) { |payload| Burli.inflate(Burli.deflate(payload)) == payload }
    assert_equal true, ractor.respond_to?(:value) ? ractor.value : ractor.take
  end

  it "has the names of the brotli gem" do
    %i[deflate inflate version].each { |name| assert_respond_to Burli, name }
    %i[process flush finish finished?].each do |name|
      assert_includes Burli::Compressor.public_instance_methods, name
    end
    %i[process finished? is_finished can_accept_more_data can_accept_more_data? unused_data].each do |name|
      assert_includes Burli::Decompressor.public_instance_methods, name
    end
    assert_operator Burli::Error, :<, StandardError
  end

  private

  def brotli_gem?
    !Brotli.equal?(Burli)
  end

  # Splits +data+ into pieces of 1 to +largest+ bytes.
  def pieces(data, seed, largest)
    random = Random.new(seed)
    out = []
    pos = 0
    while pos < data.bytesize
      size = random.rand(1..largest)
      out << data.byteslice(pos, size)
      pos += size
    end
    out
  end

  # Yields each payload, compressed by the brotli gem at each quality and
  # two window sizes, with a seed.
  def each_brotli_stream
    PAYLOADS.each_value do |payload|
      (0..11).each do |quality|
        payload = payload.byteslice(0, 10_000) if quality >= 10
        [10, 22].each do |lgwin|
          yield payload, Brotli.deflate(payload, quality: quality, lgwin: lgwin), quality
        end
      end
    end
  end
end
