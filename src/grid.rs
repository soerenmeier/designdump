//! Facts about a whole frame that no single layer shows: the page grid
//! (side padding and content width of its sections) and designer notes.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::css::num;
use crate::doc::RNode;

/// A text layer the designer put on the frame for the developers: a direct
/// child of the frame that is taken out of its auto layout (absolute), so
/// it isn't part of the page.
pub fn is_note(n: &RNode, parent: Option<&RNode>, depth: usize) -> bool {
	depth == 1
		&& n.ty == "TEXT"
		&& parent
			.is_some_and(|p| matches!(p.s("stackMode"), Some("HORIZONTAL" | "VERTICAL")))
		&& n.s("stackPositioning") == Some("ABSOLUTE")
}

pub fn note_text(n: &RNode) -> String {
	n.get("textData")
		.and_then(|t| t["characters"].as_str())
		.unwrap_or(&n.name)
		.split_whitespace()
		.collect::<Vec<_>>()
		.join(" ")
}

/// Notes of a frame, in layer order.
pub fn notes(frame: &RNode) -> Vec<String> {
	frame
		.children
		.iter()
		.filter(|c| is_note(c, Some(frame), 1))
		.map(note_text)
		.collect()
}

/// The page grid of a frame: for its full-width sections, the side padding
/// and the width of their widest child, most common first. One line per
/// value, e.g. `side padding 124px: 6 sections (Section, Hero/L)`.
pub fn page_grid(frame: &RNode) -> Vec<String> {
	let (fw, _) = frame.size();
	// (left, right) -> section names; content width -> count
	let mut pads: BTreeMap<(i64, i64), Vec<String>> = BTreeMap::new();
	let mut widths: BTreeMap<i64, usize> = BTreeMap::new();
	for s in sections(frame) {
		let left = s.f("stackHorizontalPadding").unwrap_or(0.0);
		let right = s.f("stackPaddingRight").unwrap_or(left);
		pads.entry((left.round() as i64, right.round() as i64))
			.or_default()
			.push(s.name.clone());
		let inner = fw - left - right;
		// the widest child that fills the padded width: the content width
		if let Some(w) = s
			.children
			.iter()
			.map(|c| c.size().0)
			.filter(|w| (inner - w).abs() < 1.0)
			.reduce(f64::max)
		{
			*widths.entry(w.round() as i64).or_default() += 1;
		}
	}
	let mut pads: Vec<_> = pads.into_iter().collect();
	pads.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
	let mut out = Vec::new();
	for ((l, r), names) in pads {
		let mut uniq: Vec<&String> = Vec::new();
		for n in &names {
			if !uniq.contains(&n) {
				uniq.push(n);
			}
		}
		let side = if l == r {
			format!("{}px", num(l as f64))
		} else {
			format!("{}px left, {}px right", num(l as f64), num(r as f64))
		};
		let mut line = String::new();
		let _ = write!(
			line,
			"side padding {side}: {} section{} ({})",
			names.len(),
			if names.len() == 1 { "" } else { "s" },
			uniq.iter()
				.take(4)
				.map(|s| s.as_str())
				.collect::<Vec<_>>()
				.join(", ")
		);
		out.push(line);
	}
	let mut widths: Vec<_> = widths.into_iter().collect();
	widths.sort_by(|a, b| b.1.cmp(&a.1));
	if let Some((w, n)) = widths.first() {
		out.push(format!(
			"content width {}px in {n} section{} (frame {}px)",
			num(*w as f64),
			if *n == 1 { "" } else { "s" },
			num(fw)
		));
	}
	out
}

/// The frame's children that span its width, and the full-width children
/// of a single wrapper (frames whose sections sit in one container).
fn sections(frame: &RNode) -> Vec<&RNode> {
	let (fw, _) = frame.size();
	let full = |n: &&RNode| (n.size().0 - fw).abs() < 1.0 && n.ty != "TEXT";
	let top: Vec<&RNode> = frame.children.iter().filter(full).collect();
	if top.len() == 1 && top[0].f("stackHorizontalPadding").unwrap_or(0.0) == 0.0 {
		let inner: Vec<&RNode> = top[0].children.iter().filter(full).collect();
		if inner.len() > 1 {
			return inner;
		}
	}
	top
}

/// The facts as a comment block for the top of a file.
pub fn comment(frame: &RNode, open: &str, close: &str) -> String {
	let grid = page_grid(frame);
	let notes = notes(frame);
	let mut s = String::new();
	if !grid.is_empty() {
		let _ = writeln!(s, "{open} Page grid (full-width sections of the frame):");
		for l in &grid {
			let _ = writeln!(s, "   - {l}");
		}
		let _ = writeln!(s, "   {close}");
	}
	if !notes.is_empty() {
		let _ = writeln!(
			s,
			"{open} Designer notes on this frame (not part of the page):"
		);
		for n in &notes {
			let _ = writeln!(s, "   - {}", n.replace("--", "-").replace("*/", "* /"));
		}
		let _ = writeln!(s, "   {close}");
	}
	s
}
