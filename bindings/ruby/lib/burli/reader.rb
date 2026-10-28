# frozen_string_literal: true

module Burli
  # Decompresses a stream read from an IO.
  class Reader
    # Compressed bytes read from the IO at a time, and the least decoded
    # output each step asks for.
    CHUNK_SIZE = 16 * 1024

    # @param io [#read, #readpartial] where the compressed stream comes from
    # @param options [Hash, nil] +dictionary+, as for {Burli.inflate}
    def initialize(io, options = nil, **kwargs)
      raise ArgumentError, "io should not be nil" if io.nil?

      @io = io
      @decompressor = Decompressor.new(Hash(options).merge(kwargs))
      @buffer = "".b
      @closed = false
    end

    # Reads +length+ decoded bytes, or all of them without +length+.
    # @return [String, nil] nil at the end of the stream when +length+ is given
    def read(length = nil, outbuf = nil)
      ensure_open
      if length.nil?
        step(nil) until @decompressor.finished?
        return output(outbuf, @buffer.bytesize)
      end

      length = Integer(length)
      raise ArgumentError, "negative length #{length} given" if length.negative?
      return output(outbuf, 0) if length.zero?

      step(length - @buffer.bytesize) while @buffer.bytesize < length && !@decompressor.finished?
      if @buffer.empty?
        outbuf&.clear
        return nil
      end
      output(outbuf, length)
    end

    # Reads at most +maxlen+ decoded bytes, waiting only until some arrive.
    # @raise [EOFError] at the end of the stream
    def readpartial(maxlen, outbuf = nil)
      ensure_open
      maxlen = Integer(maxlen)
      raise ArgumentError, "max length must be positive" unless maxlen.positive?

      step(maxlen) while @buffer.empty? && !@decompressor.finished?
      if @buffer.empty?
        outbuf&.clear
        raise EOFError, "end of file reached"
      end
      output(outbuf, maxlen)
    end

    def eof?
      ensure_open
      step(1) while @buffer.empty? && !@decompressor.finished?
      @buffer.empty?
    end

    # Closes the IO.
    # @return [IO] the IO
    def close
      return @io if @closed

      begin
        @io.close if @io.respond_to?(:close)
      ensure
        @closed = true
        @buffer.clear
      end
      @io
    end

    def closed?
      @closed
    end

    private

    def ensure_open
      raise Error, "Reader is closed" if @closed
    end

    # Decodes into the buffer, at least +wanted+ bytes per step unless the
    # input ends first. Reads input only when the decompressor takes it.
    def step(wanted)
      input = "".b
      if @decompressor.can_accept_more_data
        input = read_input
        raise Error, "unexpected end of compressed stream" if input.nil? || input.empty?
      end
      limit = wanted && [wanted, CHUNK_SIZE].max
      @buffer << @decompressor.process(input, output_buffer_limit: limit)
    end

    def read_input
      return @io.read(CHUNK_SIZE) unless @io.respond_to?(:readpartial)

      @io.readpartial(CHUNK_SIZE)
    rescue EOFError
      nil
    end

    # Takes up to +length+ bytes from the buffer.
    def output(outbuf, length)
      data = length >= @buffer.bytesize ? @buffer : @buffer.byteslice(0, length)
      @buffer = length >= @buffer.bytesize ? "".b : @buffer.byteslice(length..)
      return data unless outbuf

      outbuf.replace(data)
    end
  end
end
