class Paradown < Formula
  desc "HTTP-focused download manager with resume, SQLite recovery, and CLI dashboard"
  homepage "https://github.com/llp/paradown"
  version "0.1.1"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/llp/paradown/releases/download/v0.1.1/paradown-v0.1.1-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_MACOS_ARM64_SHA256"
    else
      url "https://github.com/llp/paradown/releases/download/v0.1.1/paradown-v0.1.1-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_MACOS_X64_SHA256"
    end
  end

  on_linux do
    url "https://github.com/llp/paradown/releases/download/v0.1.1/paradown-v0.1.1-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "REPLACE_WITH_LINUX_X64_SHA256"
  end

  def install
    bin.install "paradown"
    doc.install "README.md", "CHANGELOG.md", "docs/install-and-usage.md"
    pkgshare.install "paradown.toml"
  end

  test do
    assert_match "paradown", shell_output("#{bin}/paradown --help")
  end
end
