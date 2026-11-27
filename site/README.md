# Website

The landing page published with GitHub Pages, next to the user documentation
in [`docs/`](../docs) (an [mdBook](https://rust-lang.github.io/mdBook/)).
[`.github/workflows/pages.yml`](../.github/workflows/pages.yml) builds both
into one site — `/` is this page, `/docs/` the book — on every push to `main`.

Preview locally:

```sh
cargo install mdbook
mkdir -p /tmp/qell-site && cp -R site/. /tmp/qell-site/
mdbook build docs -d /tmp/qell-site/docs
python3 -m http.server -d /tmp/qell-site
```

Links are relative, so the site works at `sudo-bngz.github.io/qell/` and on
a custom domain alike. For a custom domain, add a `CNAME` file here holding
the domain name, and set the domain in the repository's *Settings → Pages*.

`assets/demo.mp4` is the looping demo in the hero, `assets/demo-poster.png`
its first frame; the screenshots live in `docs/src/images/`.
