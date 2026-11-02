# TODO

## Test files

Small `.fig` files of our own (a few MB, small images, no client content)
in `tests/fixtures/<name>/`, each with its screenshots in
`shots/desktop|tablet|mobile/` (named like the frames) and a short
`EXPECT.md` saying what should come out (e.g. "the button gets a `:hover`
rule with the color change", "the overlay screenshot is matched by name").
The tests then export each file and check the output against it.

### `layout.fig`: pages and responsive

- [ ] Pages "Desktop", "Tablet", "Mobile", each with a frame **"Page"** of the
      same name (e.g. 1440, 1024, 390).
- [ ] "Page" as vertical auto layout with sections: one with side padding
      (page grid), one full-width row, one with **wrap** auto layout, if
      possible one with the **grid auto layout**.
- [ ] A **second frame of the same width** on Tablet as on Desktop (e.g. an
      overlay at 855): told apart by the screenshot folder.
- [ ] An **overlay with "Clip content"** whose content sticks out at the
      bottom and to the right; its screenshot exported once with and once
      without "Clip content".
- [ ] A **designer note**: a text layer on the frame, taken out of the auto
      layout (absolute).
- [ ] A **hidden layer**, and an element positioned absolutely inside an auto
      layout.

### `tokens-components.fig`: variables, styles, components

- [ ] **Variables**: a color collection with aliases (`bg/accent →
      color/orange`), a "Responsive" collection with modes Desktop / Tablet /
      Mobile (a spacing, a font size, a card width).
- [ ] **Text styles** H1, H2, Body, and a text **without** a style.
- [ ] A **button component with variants** (Type=Primary/Secondary ×
      State=Default/Hover) and the prototype interaction "while hovering →
      Hover" (smart animate, e.g. 200 ms).
- [ ] **Instances with overrides**: changed text, a boolean property (hide an
      icon), an instance swap property (another icon), a nested instance (a
      card with a button).
- [ ] **The same component in two frames with a different color** (a
      deliberate inconsistency for `components.md`).
- [ ] **Texts**: a title with an accent colored second line, a text with a
      link, a bullet list; the same "Contact" on several frames; one name in
      several cards (for `texts.md`).

### `graphics.fig`: the hard cases

- [ ] Image fills (fill and fit), if possible a short video fill.
- [ ] A vector icon, a logo made of several paths.
- [ ] A gradient on a rectangle and on a vector, a shadow and a blur, a
      border on one side only, different corner radii.
- [ ] A rotated element, a mask, a group inside an auto layout.

## Release

- [ ] Tests on the fixtures above (decoding, HTML/CSS, screenshot matching,
      tokens, `components.md`, `texts.md`).
- [ ] CI: build, test, `cargo fmt --check`, `cargo clippy`.
- [ ] Prebuilt binaries for macOS (arm64, x86_64) and Linux per tag, so
      designers can install without Rust. Until then: `cargo install --git
      https://github.com/soerenmeier/designdump`.
- [ ] Try 2–3 `.fig` files from other designers / newer Figma versions and
      fix what breaks.
- [ ] `cargo publish` (crates.io name `designdump`), then `cargo install
      designdump` in the README.
