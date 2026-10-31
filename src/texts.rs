//! The texts across all exported frames, for whoever models the content:
//! which texts carry inline formatting (an accent colored line, links,
//! lists: the field needs rich text) and which texts repeat on several
//! pages (navigation, buttons, labels: fixed UI text or global data, not a
//! field per page).

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// One text layer in one frame.
pub struct TextUse {
	pub frame: String,
	/// layer path from the frame, components marked
	pub path: String,
	/// the innermost component around it
	pub component: Option<String>,
	pub text: String,
	/// the exported inner html (spans, links, list items, breaks)
	pub html: String,
}

/// The page of a frame slug: `landing-page-1024` -> `landing-page`.
fn page(frame: &str) -> &str {
	match frame.rsplit_once('-') {
		Some((p, w)) if w.chars().all(|c| c.is_ascii_digit()) && w.len() >= 3 => p,
		_ => frame,
	}
}

/// The html as readable text: `[text]{style}` for styled runs, `[text](url)`
/// for links, `•` for list items, ` / ` for line breaks.
fn readable(html: &str) -> String {
	let mut out = String::new();
	let mut rest = html;
	while let Some(i) = rest.find('<') {
		out.push_str(&rest[..i]);
		let Some(j) = rest[i..].find('>') else { break };
		let tag = &rest[i + 1..i + j];
		rest = &rest[i + j + 1..];
		if tag.starts_with("br") {
			out.push_str(" / ");
		} else if tag.starts_with("li") {
			out.push_str(" • ");
		} else if let Some(style) = tag.strip_prefix("span style=\"") {
			let style = style.trim_end_matches('"').to_string();
			let end = rest.find("</span>").unwrap_or(rest.len());
			let _ = write!(out, "[{}]{{{}}}", &rest[..end], style);
			rest = rest.get(end + 7..).unwrap_or("");
		} else if let Some(a) = tag.strip_prefix("a href=\"") {
			let url = a.split('"').next().unwrap_or("");
			let end = rest.find("</a>").unwrap_or(rest.len());
			let _ = write!(out, "[{}]({url})", &rest[..end]);
			rest = rest.get(end + 4..).unwrap_or("");
		}
	}
	out.push_str(rest);
	out.replace("&amp;", "&")
		.replace("&quot;", "\"")
		.replace("&lt;", "<")
		.replace("&gt;", ">")
}

fn short(s: &str, max: usize) -> String {
	let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
	if s.chars().count() > max {
		format!("{}…", s.chars().take(max).collect::<String>())
	} else {
		s
	}
}

pub fn markdown(texts: &[TextUse]) -> String {
	let mut out = String::from(
		"# Texts across frames\n\n\
		 For modelling the content: which texts need rich text and which are\n\
		 the same on several pages.\n\n",
	);

	// rich text: inline styles, links, lists
	let mut rich: BTreeMap<String, (Vec<&str>, &TextUse)> = BTreeMap::new();
	for t in texts {
		if t.html.contains("<span") || t.html.contains("<a ") || t.html.contains("<li") {
			let e = rich.entry(short(&t.text, 200)).or_insert((Vec::new(), t));
			if !e.0.contains(&page(&t.frame)) {
				e.0.push(page(&t.frame));
			}
		}
	}
	out.push_str(
		"## Rich text\n\n\
		 Texts with inline formatting: styled runs (`[text]{style}`, e.g. an\n\
		 accent colored second line), links (`[text](url)`) or list items (`•`).\n\
		 A field for such a text needs rich text (or a structure for the\n\
		 styled part), plain text loses it.\n\n",
	);
	if rich.is_empty() {
		out.push_str("None.\n");
	}
	for (_, (pages, t)) in &rich {
		let within = t
			.component
			.as_deref()
			.map(|c| format!(" in {c}"))
			.unwrap_or_default();
		let _ = writeln!(
			out,
			"- {}{within} ({}): {}",
			t.path,
			pages.join(", "),
			short(&readable(&t.html), 240)
		);
	}

	// the same text on several pages
	let mut same: BTreeMap<String, (Vec<&str>, Vec<String>)> = BTreeMap::new();
	for t in texts {
		let text = short(&t.text, 80);
		if text.is_empty() || t.text.chars().count() > 80 {
			continue;
		}
		let e = same.entry(text).or_default();
		if !e.0.contains(&page(&t.frame)) {
			e.0.push(page(&t.frame));
		}
		let ctx = t
			.component
			.clone()
			.unwrap_or_else(|| "(no component)".into());
		if !e.1.contains(&ctx) {
			e.1.push(ctx);
		}
	}
	let mut same: Vec<_> = same
		.into_iter()
		.filter(|(_, (p, _))| p.len() >= 3)
		.collect();
	same.sort_by(|a, b| b.1.0.len().cmp(&a.1.0.len()).then(a.0.cmp(&b.0)));
	out.push_str(
		"\n## Texts on several pages\n\n\
		 The same text on 3 or more pages. Navigation, buttons, labels and the\n\
		 footer are usually fixed UI text (a translation) or global data (menu,\n\
		 footer, contact), not a field on every page. Names, titles and roles\n\
		 that repeat in cards (people, services, clients) point to entries\n\
		 shown in several places: one entry, related from the blocks, not\n\
		 typed again in each block.\n\n",
	);
	if same.is_empty() {
		out.push_str("None.\n");
	}
	for (text, (pages, ctx)) in same.iter().take(80) {
		let _ = writeln!(
			out,
			"- \"{text}\": {} pages, in {}",
			pages.len(),
			ctx.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
		);
	}
	out
}
