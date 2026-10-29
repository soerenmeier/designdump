//! Small CSS helpers.

use serde_json::Value;

pub type Decls = Vec<(String, String)>;

pub fn push(d: &mut Decls, prop: &str, value: impl Into<String>) {
	let value = value.into();
	if let Some(slot) = d.iter_mut().find(|(p, _)| p == prop) {
		slot.1 = value;
	} else {
		d.push((prop.to_string(), value));
	}
}

/// Format a number with at most two decimals and no trailing zeros.
pub fn num(f: f64) -> String {
	let r = (f * 100.0).round() / 100.0;
	let s = format!("{r:.2}");
	let s = s.trim_end_matches('0').trim_end_matches('.');
	if s == "-0" { "0".into() } else { s.to_string() }
}

pub fn px(f: f64) -> String {
	let n = num(f);
	if n == "0" { n } else { format!("{n}px") }
}

/// A Figma color ({r, g, b, a} in 0..1) as CSS.
pub fn color(c: &Value, opacity: f64) -> String {
	let g = |k: &str| c.get(k).and_then(Value::as_f64).unwrap_or(0.0);
	let to = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
	let a = (g("a") * opacity).clamp(0.0, 1.0);
	let (r, gg, b) = (to(g("r")), to(g("g")), to(g("b")));
	if a >= 0.999 {
		format!("#{r:02x}{gg:02x}{b:02x}")
	} else {
		format!("rgba({r}, {gg}, {b}, {})", num(a))
	}
}

/// "Hero text/Title 2" -> "hero-text-title-2"
pub fn kebab(s: &str) -> String {
	let mut out = String::new();
	let mut dash = false;
	let mut prev_lower = false;
	for ch in s.chars() {
		let ch = match ch {
			'ä' | 'Ä' => 'a',
			'ö' | 'Ö' => 'o',
			'ü' | 'Ü' => 'u',
			'é' | 'è' | 'ê' | 'É' => 'e',
			c => c,
		};
		if ch.is_ascii_alphanumeric() {
			if ch.is_ascii_uppercase() && prev_lower && !dash && !out.is_empty() {
				out.push('-');
			}
			if dash && !out.is_empty() {
				out.push('-');
			}
			dash = false;
			prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
			out.push(ch.to_ascii_lowercase());
		} else {
			dash = true;
			prev_lower = false;
		}
	}
	out
}

pub fn escape(s: &str) -> String {
	s.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
		.replace('"', "&quot;")
}
