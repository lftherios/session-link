# Homebrew formula for the standalone slink binary. This lives in a SEPARATE
# public tap repo — session-link/homebrew-tap — so users can:
#
#   brew install session-link/tap/slink
#
# Update `version` and the four `sha256` values per release from the
# SHA256SUMS.txt the release workflow attaches. (A CI step in the tap repo can
# regenerate this file on each session-link release.)
class Slink < Formula
  desc "Capture LLM sessions locally, publish the ones worth sharing"
  homepage "https://session.link"
  version "0.1.0"
  license "MIT"

  # Point at your public releases repo (the code repo once public, or a
  # dedicated one). update-formula.mjs fills version + sha256 per release.
  BASE = "https://github.com/lftherios/session-link/releases/download/v#{version}".freeze

  on_macos do
    on_arm do
      url "#{BASE}/slink-darwin-arm64"
      sha256 "REPLACE_WITH_darwin_arm64_sha256"
    end
    on_intel do
      url "#{BASE}/slink-darwin-x64"
      sha256 "REPLACE_WITH_darwin_x64_sha256"
    end
  end

  on_linux do
    on_arm do
      url "#{BASE}/slink-linux-arm64"
      sha256 "REPLACE_WITH_linux_arm64_sha256"
    end
    on_intel do
      url "#{BASE}/slink-linux-x64"
      sha256 "REPLACE_WITH_linux_x64_sha256"
    end
  end

  def install
    bin.install Dir["slink-*"].first => "slink"
  end

  test do
    assert_match "session.link", shell_output("#{bin}/slink help 2>&1")
  end
end
