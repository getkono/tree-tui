# Template for the getkono/homebrew-tap formula. The Release-plz workflow's
# "update-tap" job fills in the version and four SHA-256 values below on each
# release, then commits the rendered file to the tap. The capitalised tokens
# are substituted automatically -- leave them intact when editing. Lint with:
#   ruby -c .github/homebrew/tree-tui.rb
#   brew style .github/homebrew/tree-tui.rb
class TreeTui < Formula
  desc "Interactive terminal UI for visualizing directories: code, size, and git stats"
  homepage "https://github.com/getkono/tree-tui"
  version "__VERSION__"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/getkono/tree-tui/releases/download/v#{version}/tree-tui-aarch64-apple-darwin.tar.gz"
      sha256 "__SHA256_AARCH64_APPLE_DARWIN__"
    end
    on_intel do
      url "https://github.com/getkono/tree-tui/releases/download/v#{version}/tree-tui-x86_64-apple-darwin.tar.gz"
      sha256 "__SHA256_X86_64_APPLE_DARWIN__"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/getkono/tree-tui/releases/download/v#{version}/tree-tui-aarch64-unknown-linux-musl.tar.gz"
      sha256 "__SHA256_AARCH64_UNKNOWN_LINUX_MUSL__"
    end
    on_intel do
      url "https://github.com/getkono/tree-tui/releases/download/v#{version}/tree-tui-x86_64-unknown-linux-musl.tar.gz"
      sha256 "__SHA256_X86_64_UNKNOWN_LINUX_MUSL__"
    end
  end

  # The binary is named `tree` (it intentionally shadows the classic `tree`),
  # so it collides with the homebrew-core `tree` formula on the same link path.
  conflicts_with "tree", because: "both install a `tree` binary"

  def install
    bin.install "tree"
  end

  def caveats
    <<~EOS
      tree-tui installs its binary as `tree`, shadowing the classic `tree`
      command on your PATH. Invoke it as `tree [dir]` (defaults to .).
      Keep only one of the two, or run the other via its full path.
    EOS
  end

  test do
    # `tree -V` prints "tree <version>" on its first line (BIN_NAME is "tree").
    assert_match "tree #{version}", shell_output("#{bin}/tree --version")
  end
end
