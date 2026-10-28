# frozen_string_literal: true

require_relative "lib/burli/version"

Gem::Specification.new do |spec|
  spec.name = "burli"
  spec.version = Burli::VERSION
  spec.authors = ["Patrik Wenger"]
  spec.email = ["paddor@gmail.com"]
  spec.summary = "Memory-safe Brotli compression in Rust, with the API of the brotli gem"
  spec.homepage = "https://github.com/paddor/burli/tree/main/bindings/ruby"
  spec.license = "MIT"
  spec.metadata["source_code_uri"] = spec.homepage
  spec.metadata["changelog_uri"] = "#{spec.homepage}/CHANGELOG.md"
  spec.metadata["rubygems_mfa_required"] = "true"

  spec.required_ruby_version = ">= 3.3"

  spec.files = Dir[
    "lib/**/*.rb",
    "ext/**/*.{rs,rb}",
    "ext/**/Cargo.toml",
    "README.md",
    "CHANGELOG.md"
  ]
  spec.require_paths = ["lib"]
  spec.extensions = ["ext/burli_native/extconf.rb"]

  spec.add_dependency "rb_sys", "~> 0.9"
end
