# Fonts for `synthpass-gen`

This directory is empty by default — no font files are vendored in the repo.

To render real glyphs (instead of placeholder bars) for the VIZ fields and the
MRZ band, drop two OFL (SIL Open Font License)-licensed font files here:

- `ocr-b.ttf` — an OCR-B-style monospaced font for the MRZ band.
- `sans.ttf` — a proportional sans-serif font for the human-readable VIZ fields.

Then build with the `embedded-fonts` feature enabled:

```sh
cargo build -p synthpass-gen --features embedded-fonts
```

Without this feature (the default), `fonts::load_fonts` returns
`FontError::NotEmbedded` and the renderer draws placeholder bars in the exact
layout rectangles instead — bounding boxes in `Labels` stay meaningful either
way. The unconditional "SYNTHETIC / SPECIMEN" watermark and the generic,
non-country template render regardless of this feature; they do not depend on
any TTF.
