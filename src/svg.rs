//! Vector nodes (icons, logos, lines) as SVG files.

use std::fmt::Write;

use serde_json::Value;

use crate::css::{color, num};
use crate::doc::{Doc, RNode};

pub const VECTOR_TYPES: &[&str] = &[
	"VECTOR",
	"STAR",
	"LINE",
	"ELLIPSE",
	"REGULAR_POLYGON",
	"BOOLEAN_OPERATION",
	"BRUSH",
];

const SHAPE_TYPES: &[&str] = &["RECTANGLE", "ROUNDED_RECTANGLE"];

fn has_image_fill(n: &RNode) -> bool {
	paints(n, "fillPaints").any(|p| p["type"] == "IMAGE" || p["type"] == "VIDEO")
}

pub fn paints<'a>(n: &'a RNode, key: &str) -> impl Iterator<Item = &'a Value> {
	n.get(key)
		.and_then(Value::as_array)
		.into_iter()
		.flatten()
		.filter(|p| p.get("visible").and_then(Value::as_bool) != Some(false))
}

/// Whether a node is drawn best as one SVG: a vector, or a container
/// without auto layout that only holds vectors and plain shapes.
pub fn is_graphic(n: &RNode) -> bool {
	if VECTOR_TYPES.contains(&n.ty.as_str()) {
		return true;
	}
	if !matches!(n.ty.as_str(), "FRAME" | "GROUP" | "INSTANCE" | "SYMBOL")
		|| n.children.is_empty()
		|| matches!(n.s("stackMode"), Some("HORIZONTAL" | "VERTICAL" | "GRID"))
		|| has_image_fill(n)
	{
		return false;
	}
	fn only_shapes(n: &RNode) -> bool {
		n.children.iter().all(|c| {
			if VECTOR_TYPES.contains(&c.ty.as_str()) {
				return true;
			}
			if SHAPE_TYPES.contains(&c.ty.as_str()) {
				return !has_image_fill(c);
			}
			matches!(c.ty.as_str(), "FRAME" | "GROUP" | "INSTANCE")
				&& !has_image_fill(c)
				&& only_shapes(c)
		})
	}
	fn has_vector(n: &RNode) -> bool {
		n.children
			.iter()
			.any(|c| VECTOR_TYPES.contains(&c.ty.as_str()) || has_vector(c))
	}
	only_shapes(n) && has_vector(n)
}

type Matrix = [f64; 6];

fn mul(a: &Matrix, b: &Matrix) -> Matrix {
	// [a0 a1 a2; a3 a4 a5] * [b0 b1 b2; b3 b4 b5]
	[
		a[0] * b[0] + a[1] * b[3],
		a[0] * b[1] + a[1] * b[4],
		a[0] * b[2] + a[1] * b[5] + a[2],
		a[3] * b[0] + a[4] * b[3],
		a[3] * b[1] + a[4] * b[4],
		a[3] * b[2] + a[4] * b[5] + a[5],
	]
}

/// Render a graphic node into a standalone SVG.
pub fn render(doc: &Doc, n: &RNode) -> String {
	let (w, h) = n.size();
	let mut body = String::new();
	let identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
	draw(doc, n, &identity, 1.0, &mut body);
	format!(
		"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\" fill=\"none\">\n{body}</svg>\n",
		w = num(w),
		h = num(h),
	)
}

fn draw(doc: &Doc, n: &RNode, m: &Matrix, opacity: f64, out: &mut String) {
	let opacity = opacity * n.f("opacity").unwrap_or(1.0);
	let matrix = format!(
		"matrix({} {} {} {} {} {})",
		num(m[0]),
		num(m[3]),
		num(m[1]),
		num(m[4]),
		num(m[2]),
		num(m[5])
	);

	for (key, paint_key) in [
		("fillGeometry", "fillPaints"),
		("strokeGeometry", "strokePaints"),
	] {
		let Some(fill) = paint_color(n, paint_key) else {
			continue;
		};
		let geoms = n.get(key).and_then(Value::as_array);
		for geom in geoms.into_iter().flatten() {
			let Some(blob) = geom.get("commandsBlob").and_then(Value::as_u64) else {
				continue;
			};
			let Some(d) = doc.blobs.get(blob as usize).and_then(|b| path_data(b)) else {
				continue;
			};
			if d.is_empty() {
				continue;
			}
			let rule = match geom.get("windingRule").and_then(Value::as_str) {
				Some("ODD") | Some("EVENODD") => " fill-rule=\"evenodd\"",
				_ => "",
			};
			let op = if opacity < 0.999 {
				format!(" opacity=\"{}\"", num(opacity))
			} else {
				String::new()
			};
			let _ = writeln!(
				out,
				"<path transform=\"{matrix}\" d=\"{d}\" fill=\"{fill}\"{rule}{op}/>"
			);
		}
	}

	for c in &n.children {
		let cm = mul(m, &c.transform());
		draw(doc, c, &cm, opacity, out);
	}
}

/// The color of the topmost visible solid paint (gradients use their first
/// stop).
fn paint_color(n: &RNode, key: &str) -> Option<String> {
	let p = paints(n, key).last()?;
	let op = p.get("opacity").and_then(Value::as_f64).unwrap_or(1.0);
	match p.get("type").and_then(Value::as_str)? {
		"SOLID" => Some(color(p.get("color")?, op)),
		t if t.starts_with("GRADIENT") => {
			Some(color(p.get("stops")?.get(0)?.get("color")?, op))
		}
		_ => None,
	}
}

/// Decode a Figma path blob into SVG path data.
pub fn path_data(b: &[u8]) -> Option<String> {
	let mut out = String::new();
	let mut i = 0;
	let f = |i: usize| -> Option<f64> {
		Some(f32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?) as f64)
	};
	while i < b.len() {
		let cmd = b[i];
		i += 1;
		let (letter, count) = match cmd {
			0 => ("Z", 0),
			1 => ("M", 2),
			2 => ("L", 2),
			3 => ("Q", 4),
			4 => ("C", 6),
			_ => return None,
		};
		out.push_str(letter);
		for k in 0..count {
			if k > 0 {
				out.push(' ');
			}
			out.push_str(&num(f(i)?));
			i += 4;
		}
	}
	Some(out)
}
