#!/usr/bin/env python3
"""Regenerate crates/qell-app/assets/fonts/qell-icons.ttf.

Qell draws file icons with Nerd Fonts glyphs, picked per file by the
`devicons` crate. The full "Symbols Nerd Font" is ~2.5 MB; this keeps only
the glyphs devicons can return plus the few Qell uses itself.

Usage (needs `pip install fonttools`):

    curl -LO https://github.com/ryanoasis/nerd-fonts/releases/latest/download/NerdFontsSymbolsOnly.zip
    unzip NerdFontsSymbolsOnly.zip SymbolsNerdFont-Regular.ttf
    cargo fetch
    python3 scripts/subset-icons.py SymbolsNerdFont-Regular.ttf

Re-run after bumping `devicons` in Cargo.toml.
"""

import glob
import os
import re
import sys

from fontTools import subset
from fontTools.ttLib import TTFont

# Glyphs Qell uses directly (keep in sync with crates/qell-app/src/icons.rs).
EXTRA = [
    0xE5FF,  # nf-custom-folder
    0xE5FE,  # nf-custom-folder_open
    0xE64E,  # nf-seti-default (unknown files)
    0xE615,  # nf-seti-config (VCL, which devicons doesn't know)
    # Codicons (VS Code's own icon set) for the activity bar and Source Control.
    0xEAF0,  # cod-files
    0xEA68,  # cod-source_control
    0xEA60,  # cod-add
    0xEB3B,  # cod-remove
    0xEAB2,  # cod-check
    0xEB37,  # cod-refresh
    # Completion item kinds (VS Code's symbol icons).
    0xEA8C, 0xEB5F, 0xEA88, 0xEB5B, 0xEB61, 0xEA8B, 0xEB65, 0xEA95, 0xEB5E,
    0xEB62, 0xEB66, 0xEB5C, 0xEB60, 0xEB5D, 0xEA91, 0xEA86, 0xEB64, 0xEA92,
    0xEA93, 0xEA96, 0xEB63, 0xEB36, 0xEA83, 0xEB8D,
    # Git graph: commit, tag, remote branch.
    0xEAFC, 0xEA66, 0xEBAA,
    # Search: search, case, whole word, regex, close, chevrons.
    0xEA6D, 0xEAB1, 0xEB7E, 0xEB38, 0xEA76, 0xEAB6, 0xEAB4,
    # Activity bar: the AWS view (Material Design Icons' md-aws).
    0xF0E0F,
]

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "crates/qell-app/assets/fonts/qell-icons.ttf")


def devicons_source():
    lock = open(os.path.join(ROOT, "Cargo.lock")).read()
    version = re.search(r'name = "devicons"\nversion = "([^"]+)"', lock).group(1)
    home = os.environ.get("CARGO_HOME", os.path.expanduser("~/.cargo"))
    hits = glob.glob(f"{home}/registry/src/*/devicons-{version}/src/icons/dark.rs")
    if not hits:
        sys.exit(f"devicons {version} not found in {home}/registry; run `cargo fetch` first")
    return open(hits[0]).read()


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    src = devicons_source()
    codepoints = {ord(c) for c in re.findall(r"icon: '(.)'", src)}
    codepoints |= {int(h, 16) for h in re.findall(r"icon: '\\u\{([0-9a-fA-F]+)\}'", src)}
    codepoints |= set(EXTRA)

    font = TTFont(sys.argv[1])
    available = font.getBestCmap()
    keep = sorted(cp for cp in codepoints if cp in available)

    options = subset.Options()
    options.name_IDs = ["*"]  # keep the copyright / licence name records
    options.notdef_outline = True
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=keep)
    subsetter.subset(font)
    font.save(OUT)
    missing = len(codepoints) - len(keep)
    print(f"wrote {OUT}: {len(keep)} glyphs ({missing} not in the font, rendered by fallback fonts)")


if __name__ == "__main__":
    main()
