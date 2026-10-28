# frozen_string_literal: true

require "mkmf"
require "rb_sys/mkmf"

create_rust_makefile("burli/burli_native") do |r|
  r.profile = ENV.fetch("RB_SYS_CARGO_PROFILE", :release).to_sym
  # Gem installs build the crate outside the repo workspace and its profile.
  r.env = {
    "CARGO_PROFILE_RELEASE_LTO" => "true",
    "CARGO_PROFILE_RELEASE_CODEGEN_UNITS" => "1",
    "CARGO_PROFILE_RELEASE_STRIP" => "true"
  }
end
