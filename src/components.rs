//! The components across all exported frames: where each one is used, its
//! size per frame width and the values that differ between its uses (e.g.
//! a bullet that is orange in some frames and blue in others). One frame
//! shows one use; the differences only show next to each other.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::css::Decls;

/// One instance in one frame.
pub struct Use {
	pub component: String,
	pub variant: Option<String>,
	/// e.g. `landing-page-1024`
	pub frame: String,
	pub width: f64,
	/// layer path inside the instance ("" = the instance) -> declarations
	pub layers: Vec<(String, Decls)>,
}

/// Declarations about the size of the instance itself.
const SIZE: &[&str] = &[
	"width",
	"height",
	"min-width",
	"max-width",
	"min-height",
	"flex",
	"flex-grow",
	"flex-basis",
	"align-self",
	"aspect-ratio",
];
/// Where it sits, not what it looks like.
const PLACE: &[&str] = &[
	"left",
	"top",
	"right",
	"bottom",
	"position",
	"z-index",
	"inset",
	"transform",
];

pub fn markdown(uses: &[Use]) -> String {
	let mut by: BTreeMap<&str, Vec<&Use>> = BTreeMap::new();
	for u in uses {
		by.entry(&u.component).or_default().push(u);
	}
	let mut comps: Vec<_> = by.into_iter().collect();
	comps.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));

	let mut out = String::from(
		"# Components across frames\n\n\
		 Every component used in the exported frames: where, its size per frame\n\
		 width, and the values that differ between its uses (layer path inside\n\
		 the component, then the value and the frames that use it). A size that\n\
		 is the same at every width is usually a fixed ratio or a max size, not a\n\
		 fixed size; values that differ between desktop frames are worth a\n\
		 decision (one is often a mistake in the design).\n\n",
	);
	for (name, list) in comps {
		let frames = uniq(list.iter().map(|u| u.frame.as_str()));
		let _ = writeln!(
			out,
			"## {name}\n\n{} use{} in {} frame{}: {}",
			list.len(),
			if list.len() == 1 { "" } else { "s" },
			frames.len(),
			if frames.len() == 1 { "" } else { "s" },
			some(&frames, 8)
		);
		let variants = uniq(list.iter().filter_map(|u| u.variant.as_deref()));
		if variants.len() > 1 {
			let mut v: Vec<&str> = variants.iter().take(6).copied().collect();
			let more = format!("… ({} more)", variants.len().saturating_sub(6));
			if variants.len() > 6 {
				v.push(&more);
			}
			let _ = writeln!(out, "\nVariants: {}", v.join(" | "));
		}

		// size per frame width
		let mut sizes: BTreeMap<i64, Vec<String>> = BTreeMap::new();
		for u in &list {
			let Some((_, d)) = u.layers.iter().find(|(p, _)| p.is_empty()) else {
				continue;
			};
			let s: Vec<String> = d
				.iter()
				.filter(|(p, _)| SIZE.contains(&p.as_str()))
				.map(|(p, v)| format!("{p}: {v}"))
				.collect();
			let s = if s.is_empty() {
				"hug".to_string()
			} else {
				s.join("; ")
			};
			let e = sizes.entry(-(u.width.round() as i64)).or_default();
			if !e.contains(&s) {
				e.push(s);
			}
		}
		if !sizes.is_empty() {
			out.push_str("\nSize:\n");
			for (w, s) in &sizes {
				let _ = writeln!(out, "- at {}px: {}", -w, some(s, 3));
			}
		}

		// values that differ between uses of the same variant: different
		// variants are meant to differ, the same variant shouldn't
		let mut by_variant: BTreeMap<&str, Vec<&Use>> = BTreeMap::new();
		for u in &list {
			by_variant
				.entry(u.variant.as_deref().unwrap_or(""))
				.or_default()
				.push(u);
		}
		let mut lines = Vec::new();
		for (variant, us) in &by_variant {
			let mut vals: BTreeMap<(String, String), BTreeMap<String, Vec<&str>>> =
				BTreeMap::new();
			for u in us {
				for (path, d) in &u.layers {
					for (p, v) in d {
						// placement, the instance's own size (above) and content images
						if PLACE.contains(&p.as_str())
							|| (path.is_empty() && SIZE.contains(&p.as_str()))
							|| v.contains("url(")
						{
							continue;
						}
						let e = vals.entry((path.clone(), p.clone())).or_default();
						let f = e.entry(v.clone()).or_default();
						if !f.contains(&u.frame.as_str()) {
							f.push(&u.frame);
						}
					}
				}
			}
			for ((path, prop), v) in vals.into_iter().filter(|(_, v)| v.len() > 1) {
				let layer = if path.is_empty() {
					"(component)".to_string()
				} else {
					path
				};
				let parts: Vec<String> = v
					.iter()
					.map(|(val, f)| format!("`{val}` in {}", some(f, 4)))
					.collect();
				let of = if by_variant.len() > 1 && !variant.is_empty() {
					format!(" [{variant}]")
				} else {
					String::new()
				};
				lines.push(format!("- {layer} `{prop}`{of}: {}", parts.join("; ")));
			}
		}
		if !lines.is_empty() {
			out.push_str("\nDiffers between uses of the same variant:\n");
			for l in lines.iter().take(30) {
				let _ = writeln!(out, "{l}");
			}
			if lines.len() > 30 {
				let _ = writeln!(out, "- … {} more", lines.len() - 30);
			}
		}
		out.push('\n');
	}
	out
}

fn uniq<'a>(it: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
	let mut v: Vec<&str> = Vec::new();
	for s in it {
		if !v.contains(&s) {
			v.push(s);
		}
	}
	v
}

/// The first `max` items, joined with ", ".
fn some<S: AsRef<str>>(v: &[S], max: usize) -> String {
	let mut s: Vec<&str> = v.iter().take(max).map(|x| x.as_ref()).collect();
	let more;
	if v.len() > max {
		more = format!("… ({} more)", v.len() - max);
		s.push(&more);
	}
	s.join(", ")
}
