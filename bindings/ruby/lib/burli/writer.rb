# frozen_string_literal: true

module Burli
  # Compresses what is written to it into an IO.
  class Writer
    # @param io [#write] where the compressed stream goes
    # @param options [Hash, nil] +mode+, +quality+, +lgwin+, and +lgblock+, as
    #   for {Burli.deflate}
    def initialize(io, options = nil, **kwargs)
      raise ArgumentError, "io should not be nil" if io.nil?

      @io = io
      @compressor = Compressor.new(Hash(options).merge(kwargs))
      @closed = false
    end

    # Compresses +data+ and writes the output produced so far.
    # @return [Integer] the bytes of +data+
    def write(data)
      ensure_open
      data = String.try_convert(data) || raise(TypeError, "no implicit conversion of #{data.class} into String")
      put(@compressor.process(data))
      data.bytesize
    end

    # Writes output that decodes to all data so far, and flushes the IO.
    # @return [self]
    def flush
      ensure_open
      put(@compressor.flush)
      @io.flush if @io.respond_to?(:flush)
      self
    end

    # Ends the stream without closing the IO.
    # @return [IO] the IO
    def finish
      ensure_open
      put(@compressor.finish)
      @io
    end

    # Ends the stream and closes the IO.
    # @return [IO] the IO
    def close
      return @io if @closed

      begin
        finish
      ensure
        @closed = true
        @io.close if @io.respond_to?(:close)
      end
      @io
    end

    def closed?
      @closed
    end

    private

    def ensure_open
      raise Error, "Writer is closed" if @closed
    end

    def put(output)
      @io.write(output) unless output.empty?
    end
  end
end
