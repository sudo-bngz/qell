# Qell's logo

A `q` lit on a 5×7 LED matrix, with the last dot as a cursor.

- `qell-led.svg` — the mark with the `qell` wordmark, on the dark background.
- `qell-led-mark.svg` — the mark alone, on the dark background.
- `qell-led-mark-transparent.svg` — the mark alone, no background.

The app icon (`crates/qell-app/assets/icon/qell.svg`) is the transparent
mark on a rounded tile, on Apple's icon grid; its PNGs are rendered from it:

```sh
rsvg-convert -w 1024 -h 1024 crates/qell-app/assets/icon/qell.svg -o crates/qell-app/assets/icon/qell-1024.png
rsvg-convert -w 256 -h 256 crates/qell-app/assets/icon/qell.svg -o crates/qell-app/assets/icon/qell-256.png
```
