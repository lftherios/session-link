# Homebrew formula for the standalone slink binary. This lives in a SEPARATE
# public tap repo — lftherios/homebrew-tap — so users can:
#
#   brew install lftherios/tap/slink
#
# Update `version` and the four `sha256` values per release from the
# SHA256SUMS.txt the release workflow attaches. (A CI step in the tap repo can
# regenerate this file on each session-link release.)
class Slink < Formula
  desc "Capture LLM sessions locally, publish the ones worth sharing"
  homepage "https://session.link"
  version "0.2.0"
  license "MIT"

  # Point at your public releases repo (the code repo once public, or a
  # dedicated one). update-formula.mjs fills version + sha256 per release.
  BASE = "https://github.com/lftherios/session-link/releases/download/v#{version}".freeze

  on_macos do
    on_arm do
      url "#{BASE}/slink-darwin-arm64"
      sha256 "7c1502b53fd3d782d6872c584ae9ff47b440a5e36ded0b1304f9d1eaca9ca08a"
    end
    on_intel do
      url "#{BASE}/slink-darwin-x64"
      sha256 "6cbaa9128bc08da5763cb96764000db29c0fc00c1508734e1653e64528d01083"
    end
  end

  on_linux do
    on_arm do
      url "#{BASE}/slink-linux-arm64"
      sha256 "59e78c820e70e63f704203c96bd4417328e3967db5c2bc280c67a2ec67cd755a"
    end
    on_intel do
      url "#{BASE}/slink-linux-x64"
      sha256 "f4dcc3fa85faa2147c00c9d50fcded4c5968516407eb0136f0f43ad1e3a15f9b"
    end
  end

  def install
    bin.install Dir["slink-*"].first => "slink"
  end

  test do
    assert_match "session.link", shell_output("#{bin}/slink help 2>&1")
  end
end
