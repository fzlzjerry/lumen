# Reference Homebrew formula.
#
# This file is a template for documentation. The actual formula served to users
# lives in the tap repository `fzlzjerry/homebrew-lumen` at `Formula/lumen.rb`
# and is regenerated on every release by .github/workflows/release.yml (it fills
# in the version and the four SHA-256 checksums from the GitHub Release assets).
#
# Users install with:
#   brew install fzlzjerry/lumen/lumen
# or:
#   brew tap fzlzjerry/lumen && brew install lumen
class Lumen < Formula
  desc "Dynamic, bytecode-compiled programming language with a full toolchain"
  homepage "https://github.com/fzlzjerry/lumen"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/fzlzjerry/lumen/releases/download/v0.1.0/lumen-0.1.0-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
    on_intel do
      url "https://github.com/fzlzjerry/lumen/releases/download/v0.1.0/lumen-0.1.0-x86_64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/fzlzjerry/lumen/releases/download/v0.1.0/lumen-0.1.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
    on_intel do
      url "https://github.com/fzlzjerry/lumen/releases/download/v0.1.0/lumen-0.1.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  def install
    bin.install "lumen"
  end

  test do
    assert_match "lumen", shell_output("#{bin}/lumen --version")
  end
end
