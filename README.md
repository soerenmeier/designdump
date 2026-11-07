# designdump

Turns design files saved from Figma (`.fig`) into HTML, CSS and design
tokens that AI agents (and developers) can read to build a website from
the design, and that a browser can render as a preview. It reads the file
directly: no Figma API, no token, no plugin.

> Unofficial tool, not affiliated with, endorsed by or sponsored by Figma,
> Inc. It reads your own files; the `.fig` format is undocumented and may
> change with new Figma versions.

Why: an agent that builds a page from a screenshot guesses every color,
size and spacing. From the export it gets the real values, the variables
they come from, the auto layout as flexbox, the tablet and mobile versions
of each frame, and overviews across all frames that no single frame shows:
which component differs between pages, which texts need rich text, what
the page grid is, and the notes the designer left for the developers.

## Install

```sh
cargo install --git https://github.com/soerenmeier/designdump
```

Needs Rust ([rustup.rs](https://rustup.rs)). With SSH access instead of
HTTPS: `CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --git
ssh://git@github.com/soerenmeier/designdump.git` (cargo's own git client
ignores your SSH setup). Update with the same command plus `--force`.

## Prepare the design

1. In Figma: File → Save local copy → `design.fig`.
2. Export a screenshot of every frame you want, named exactly like the
   frame, one folder per width:

   ```
   shots/desktop/Landing page.png
   shots/tablet/Landing page.png
   shots/mobile/Landing page.png
   ```

   The screenshots pick the frames to export, and the folders tell frames
   of the same width apart (a tablet design drawn at the desktop width).
   Figma's `Name-1.png` for a second frame called `Name` is matched by
   size, and its other renames too: a frame "Account / Favorites" is
   exported as `Account/Favorites.png` (a folder), "Editor: New" as
   `Editor_ New.png`. For frames that clip their content (an overlay that scrolls, a
   slider that runs out of the frame), turn off "Clip content" before
   exporting, so the screenshot shows everything; a screenshot larger than
   its frame is matched by name.

What helps the result most: variables for colors and spacing (a collection
with Desktop / Tablet / Mobile modes for responsive values), text styles,
components with hover variants, the tablet and mobile designs as frames
with the same name, and notes for the developers as text layers on the
frame taken out of the auto layout ("autoplay, but much slower").

## Use

```sh
# the pages and their top level frames, with ids and sizes
designdump list design.fig

# export every frame that has a screenshot, with its tablet/mobile versions
designdump export design.fig --screenshots shots/ --responsive --out export/

# or pick frames by name or id
designdump export design.fig --frame "Landing page" --frame 4323:2656 --out export/

# map the Figma tokens to the variables and SCSS mixins a project already
# has (a comment at the top of tokens.css)
designdump export design.fig -s shots/ -r --project src/ --out export/

# debug: the resolved tree of one node as JSON
designdump dump design.fig --node 2042:1131
```

Short flags: `-s` screenshots, `-r` responsive, `-p` project, `-f` frame,
`-o` out. `--fonts <css>` adds font faces to the preview.

## Output

| File | What |
|---|---|
| `<frame>.html` | one element per visible layer, class names from the layer names; instances carry `data-component`, `data-variant`, `data-props`; text styles that are headings become `h1`–`h6` |
| `<frame>.css` | one rule per class: auto layout as flexbox (gap, padding, alignment, hug / fill / fixed), everything else absolute; bound variables instead of raw values; `:hover` rules from hover variants with their transition; `@media` rules from the smaller designs |
| top of both | the **page grid** (side padding of the frame's full-width sections, content width, most common first) and the **designer notes** (text layers taken out of the auto layout, also left in place as `<!-- designer note: … -->`) |
| textures | layers that are a decorative texture over the page, not content (the same image tiled over the frame width, blended or transparent, like paper noise): a comment in place and a list at the top of the frame files |
| `<frame>-<width>.*`, `<frame>-tablet.*` | the smaller designs on their own (`-r`), the reliable version |
| `tokens.css` | the variables the frames use as custom properties (aliases stay aliases), modes as media queries and `[data-<collection>="<mode>"]`, text styles as classes (`.t-h1`); with `-p` the mapping to the project's variables and the tokens without a match |
| `components.md` | every component across all frames: where it is used, its size per width, the values that differ between uses of the same variant (a bullet orange in one frame and blue in another needs a decision) |
| `texts.md` | texts with inline formatting (an accent colored line, links, lists: rich text needed) and texts repeated on several pages (UI text, global data, or entries shown in several places) |
| `<frame>.screenshot.*`, `assets/`, `index.html`, `base.css` | the matched screenshot, images, videos and vector layers as SVG, a list of the frames, a small reset |

A layer in the HTML and its rule:

```html
<div class="card" data-component="Card/Offer" data-variant="Has-image=True">
	<img class="photo" src="assets/photo-3f2a.jpg" alt="Photo">
	<h3 class="title t-h3">Personal training</h3>
</div>
```

```css
.card {
	display: flex;
	flex-direction: column;
	gap: var(--spacing-24-18);
	width: var(--card-width);
	padding: var(--spacing-12-8);
	background-color: var(--bg-grey);
	border-radius: var(--corner-radius-s);
}
```

## How it works

A `.fig` file is a zip with `canvas.fig`, images and videos. `canvas.fig`
holds a compressed [Kiwi](https://github.com/evanw/kiwi) schema and the
document encoded with it; `kiwi.rs` decodes both generically from the
schema in the file, so new Figma versions keep working as long as the
field names stay.

`doc.rs` indexes the nodes and expands instances: Figma stores an
instance without children, only its component, overrides keyed by guid
paths, the layout it computed (`derivedSymbolData`) and the values of its
component properties. `html.rs` turns the expanded tree into HTML and CSS,
`vars.rs` the variables into tokens, `svg.rs` vector layers into SVG,
`grid.rs` the page grid and notes, `components.rs` and `texts.rs` the
overviews, `map.rs` the mapping to a project.

## Known gaps

- Grid auto layout, rotated nodes in auto layout and masks are approximate.
- Gradients on vectors use their first color.
- Text keeps Figma's font names; the preview needs the fonts (`--fonts`).
- The `@media` rules merged into a frame's CSS are a best effort; the
  smaller designs exported on their own are exact.
- Breakpoints of variable modes are guessed from the frame widths.
- Tested on real designs of a few agencies; files from other designers may
  use features it doesn't know yet. Issues with a small sample file help.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in this
crate by you, as defined in the Apache-2.0 license, shall be dual licensed
as above, without any additional terms or conditions.
