# frozen_string_literal: true

require "minitest/autorun"
require "burli"
begin
  require "brotli"
rescue LoadError
  # Without the brotli gem, as on TruffleRuby, test burli against itself.
  Brotli = Burli
end

Warning[:experimental] = false

module Payloads
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

  ALL = {
    empty: "".b,
    byte: "x".b,
    text: text(100_000),
    noise: noise(50_000),
    zeros: ("\0" * 200_000).b,
    mixed: text(30_000) + noise(20_000) + text(30_000)
  }.freeze

  # Splits +data+ into pieces of 1 to +largest+ bytes.
  def self.pieces(data, seed, largest)
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
end
