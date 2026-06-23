# Homebrew formula for Compass.
#
# This is a template: per release, set `version` and fill each `sha256` from the matching
# `compass-<target>.tar.gz.sha256` asset on the GitHub Release. Host it in a tap repo
# (e.g. `QuirijnPolinoco/homebrew-tap`) so users can:
#
#   brew install QuirijnPolinoco/tap/compass
#
# See packaging/README.md.
class Compass < Formula
  desc "Local-first tool that maps a codebase into a queryable graph for AI and humans"
  homepage "https://github.com/QuirijnPolinoco/compass"
  version "0.5.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/QuirijnPolinoco/compass/releases/download/v#{version}/compass-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_aarch64-apple-darwin_SHA256"
    end
    on_intel do
      url "https://github.com/QuirijnPolinoco/compass/releases/download/v#{version}/compass-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_x86_64-apple-darwin_SHA256"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/QuirijnPolinoco/compass/releases/download/v#{version}/compass-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_x86_64-unknown-linux-gnu_SHA256"
    end
  end

  def install
    bin.install "compass"
  end

  test do
    assert_match "Supported languages", shell_output("#{bin}/compass languages")
  end
end
