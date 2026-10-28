# frozen_string_literal: true

require_relative "burli/version"

# Precompiled gems hold one extension per Ruby minor version.
begin
  require_relative "burli/#{RUBY_VERSION[/\A\d+\.\d+/]}/burli_native"
rescue LoadError
  require_relative "burli/burli_native"
end

# Memory-safe Brotli compression and decompression, with the API of the
# brotli gem. Existing code can switch with:
#
#   Brotli = Burli
#
# Qualities 6 to 11 run as 5.
module Burli
  # Checks options, with the messages of the brotli gem, for the native
  # methods.
  module Options
    MODES = {generic: 0, text: 1, font: 2}.freeze

    module_function

    def merge(options, kwargs)
      Hash(options).merge(kwargs)
    end

    # @return [Array(Integer, Integer, Integer, Integer)] mode, quality,
    #   lgwin, and lgblock
    def encoder(options)
      unless options[:dictionary].nil?
        raise ArgumentError, "burli does not support dictionaries for compression"
      end

      mode = options[:mode].nil? ? 0 : MODES.fetch(options[:mode]) { raise ArgumentError, "invalid mode" }
      [
        mode,
        integer(options[:quality], 5, "invalid quality value. Should be 0 to 11.") { (0..11).cover?(_1) },
        integer(options[:lgwin], 22, "invalid lgwin value. Should be 10 to 24.") { (10..24).cover?(_1) },
        integer(options[:lgblock], 0, "invalid lgblock value. Should be 0 or 16 to 24.") do
          _1.zero? || (16..24).cover?(_1)
        end
      ]
    end

    def integer(value, default, message)
      return default if value.nil?
      raise ArgumentError, message unless value.is_a?(Integer) && yield(value)

      value
    end

    # @return [String, nil] raw dictionary bytes
    def dictionary(options)
      options[:dictionary]
    end

    # @return [Integer] the limit, or 0 for none
    def output_buffer_limit(options)
      limit = options[:output_buffer_limit]
      return 0 if limit.nil?
      raise TypeError, "output_buffer_limit must be an Integer" unless limit.is_a?(Integer)
      raise ArgumentError, "output_buffer_limit must be positive" unless limit.positive?

      limit
    end
  end
  private_constant :Options

  private_class_method :_native_deflate, :_native_inflate

  class << self
    # Compresses +string+.
    # @param options [Hash, nil] +mode+ (+:generic+, +:text+, or +:font+),
    #   +quality+ (0 to 11), +lgwin+ (10 to 24), and +lgblock+ (0 or 16 to 24)
    # @return [String] the Brotli stream
    # @raise [ArgumentError] for invalid options
    def deflate(string, options = nil, **kwargs)
      raise ArgumentError, "input should not be nil" if string.nil?

      _native_deflate(string, *Options.encoder(Options.merge(options, kwargs)))
    end

    # Decompresses a whole Brotli stream, from a String or an IO.
    # @param options [Hash, nil] +dictionary+, a raw dictionary
    # @return [String]
    # @raise [Error] for malformed or truncated streams, and for data after
    #   the end of the stream
    def inflate(string, options = nil, **kwargs)
      string = string.read if string.respond_to?(:read)
      _native_inflate(string, Options.dictionary(Options.merge(options, kwargs)))
    end

    # @return [String] the gem version
    def version
      VERSION
    end
  end

  # Compresses a stream in pieces.
  class Compressor
    private_class_method :_native_new

    # @param options [Hash, nil] as for {Burli.deflate}
    def self.new(options = nil, **kwargs)
      _native_new(*Options.encoder(Options.merge(options, kwargs)))
    end
  end

  # Decompresses a stream in pieces.
  class Decompressor
    private_class_method :_native_new
    private :_native_process

    # @param options [Hash, nil] +dictionary+, a raw dictionary
    def self.new(options = nil, **kwargs)
      _native_new(Options.dictionary(Options.merge(options, kwargs)))
    end

    # Decompresses +input+, returning the output produced so far. With
    # +output_buffer_limit+, returns at most that many bytes; then call with
    # empty input until {#can_accept_more_data} is true before passing more.
    # @raise [Error] for malformed streams, and for input while output is
    #   pending or after the end of the stream
    def process(input, options = nil, **kwargs)
      _native_process(input, Options.output_buffer_limit(Options.merge(options, kwargs)))
    end

    alias_method :is_finished, :finished?
    alias_method :can_accept_more_data?, :can_accept_more_data
  end
end

require_relative "burli/reader"
require_relative "burli/writer"
