# frozen_string_literal: true

require "minitest/autorun"
require "minitest/spec"
require "stringio"
require "burli"
begin
  require "brotli"
rescue LoadError
  # Without the brotli gem, as on TruffleRuby, test burli against itself.
  Brotli = Burli
end

Warning[:experimental] = false
