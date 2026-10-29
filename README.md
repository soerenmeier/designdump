# figma-to-html

Reads Figma `.fig` files directly (no API, no plugin) and turns frames into
HTML + CSS that AI agents can read to build components, and that a browser
can render as a preview.

```sh
# pages and top level frames
figma-to-html list design.fig

# export frames that have a screenshot (named after the frame) in shots/
figma-to-html export design.fig --screenshots shots/ --out out/

# or pick frames by name or id
figma-to-html export design.fig --frame "Landing page" --frame 4323:2656 --out out/

# add the tablet / mobile designs (frames with the same name, smaller width)
figma-to-html export design.fig --screenshots shots/ --responsive --out out/

# debug: the resolved tree of one node as JSON
figma-to-html dump design.fig --node 2042:1131
```

## Output

- `tokens.css`: the Figma variables that the frames use, as CSS custom
  properties (aliases stay aliases, e.g. `--bg-gold: var(--color-gold)`).
  Collections with several modes (e.g. Desktop / Tablet / Mobile) get a
  media query (breakpoint guessed from the frame widths) and a
  `[data-<collection>="<mode>"]` selector. Text styles become classes
  (`.t-h1`, `.t-body-md`).
- `<frame>.html`: one element per visible layer, class names from the layer
  names. Instances carry `data-component`, `data-variant` and `data-props`
  (component property values). Text uses `h1`–`h6` when its style is a
  heading.
- `<frame>.css`: one rule per class. Auto layout becomes flexbox (gap,
  padding, alignment, hug / fill / fixed sizes), everything else absolute
  positioning. Bound variables are used instead of raw values.
- Hover: instances whose component swaps to another variant on hover get
  `:hover` rules with the difference between the variants and the
  transition of the interaction. Clicks that navigate become `data-link`.
- `--responsive`: frames with the same name and a smaller width (the most
  similar one per width) become `@media` rules in the frame's CSS, where
  every layer with a class changes the same way; layers that only exist in
  the smaller design are listed in a comment. They are also exported on
  their own as `<frame>-<width>.html/css`, which is the reliable version.
- `<frame>.screenshot.*`: the screenshot that matched the frame.
- `assets/`: images, videos and vector layers (icons, logos) as SVG.
- `base.css`: a small reset, plus `--fonts <file>` for the preview.

## How it works

A `.fig` file is a zip with `canvas.fig`, images and videos. `canvas.fig`
holds a compressed [Kiwi](https://github.com/evanw/kiwi) schema and the
document encoded with it; `kiwi.rs` decodes both generically, so new Figma
versions keep working as long as field names stay.

`doc.rs` indexes the nodes and expands instances: Figma stores instances
without children, only a component id, overrides keyed by guid paths, the
layout it computed (`derivedSymbolData`) and component property values.
`html.rs` turns the expanded tree into HTML and CSS, `vars.rs` the
variables into tokens, `svg.rs` vector layers into SVG files.

## Known gaps

- Grid auto layout, rotated nodes in auto layout and masks are approximate.
- Gradients on vectors use their first color.
- Text keeps Figma's font names; the preview needs the fonts (`--fonts`).
