# Install

Qell runs on **macOS 11 or later** (Apple silicon and Intel) and on
**Linux x86_64**. Download the latest build from the
[releases page](https://github.com/sudo-bngz/qell/releases).

## macOS

1. Download `Qell-<version>-macos-universal.zip` and unzip it.
2. Move `Qell.app` to **Applications**.
3. The app isn't signed by Apple yet, so the first time macOS refuses to
   open it. Right-click `Qell.app` → **Open**, then **Open** again — once.
   Or, in a terminal:

   ```sh
   xattr -dr com.apple.quarantine /Applications/Qell.app
   ```

Started from the Dock or the Finder, Qell asks your login shell for its
`PATH` once, so it finds `gopls`, `terraform`, Homebrew's tools and the rest
as a terminal would.

To open folders from a terminal, add an alias to your shell:

```sh
alias qell='/Applications/Qell.app/Contents/MacOS/qell'
qell ~/src/my-project
```

## Linux

1. Download `qell-<version>-linux-x86_64.tar.gz` and unpack it.
2. Run `./install.sh`: it puts `qell` in `~/.local/bin`, with a menu entry
   and an icon. Make sure `~/.local/bin` is on your `PATH`.

Qell needs the usual windowing libraries: `libxkbcommon`, `libwayland` or
X11, `libgl`, and for the file dialogs `libgtk-3` or `xdg-desktop-portal`.
On Debian or Ubuntu:

```sh
sudo apt install libxkbcommon0 libgl1 libgtk-3-0
```

Each release comes with a `SHA256SUMS` file to check the downloads:

```sh
sha256sum -c SHA256SUMS --ignore-missing
```

## From source

With a Rust toolchain (1.95 or later):

```sh
git clone https://github.com/sudo-bngz/qell
cd qell
cargo install --path crates/qell-app
```

## Opening things

```sh
qell                 # the current folder
qell ~/src/api       # a folder
qell src/main.rs     # a file, in the project around it
```

Opening a file opens its project: Qell walks up to the nearest folder with
a `.git`, `go.mod`, `Cargo.toml`, `package.json` or similar.

## Language servers

Qell talks to the language servers on your `PATH` and **never installs
them**. When one is missing, the status bar says *LSP not installed*; hover
it for the command to run, click to copy it. The usual ones:

```sh
rustup component add rust-analyzer          # Rust
go install golang.org/x/tools/gopls@latest  # Go
npm i -g typescript typescript-language-server  # TypeScript / JavaScript
brew install hashicorp/tap/terraform-ls     # Terraform
```

See [Languages](languages.md) to use other servers.

## Updating

Download the new release and replace the app (or run `install.sh` again).
Your settings and sessions are kept.
