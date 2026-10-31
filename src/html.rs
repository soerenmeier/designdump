//! Turns a resolved node tree into HTML with one class per node and a CSS
//! file. Auto layout becomes flexbox, bound variables become `var(--…)`.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::components::Use;
use crate::css::{Decls, color, escape, kebab, num, push, px};
use crate::doc::{Doc, Guid, RNode};
use crate::grid;
use crate::svg;
use crate::texts::TextUse;
use crate::vars::Vars;

pub struct Emitter<'a> {
	pub doc: &'a Doc,
	pub vars: &'a Vars,
	pub images: &'a HashMap<String, Vec<u8>>,
	pub videos: &'a HashMap<String, Vec<u8>>,
	pub assets_dir: PathBuf,
	/// path of the assets dir as seen from the html
	pub assets_url: String,
	/// class name -> rule bodies that use it (index = suffix)
	classes: HashMap<String, Vec<Decls>>,
	/// rules in document order: (selector, declarations)
	rules: Vec<(String, Decls)>,
	/// class of every emitted node, by address in the current tree
	class_of: HashMap<*const RNode, String>,
	/// rules for the frame at other widths
	media: Vec<Media>,
	/// hover rules already written, to skip repeats
	seen_rules: std::collections::HashSet<String>,
	/// text styles used: style guid -> class
	pub text_styles: BTreeMap<Guid, String>,
	written_assets: HashMap<String, String>,
	/// the frame being emitted, for `uses`: (slug, width)
	pub frame_label: (String, f64),
	/// every instance emitted, with the declarations of its layers
	pub uses: Vec<Use>,
	/// every text layer emitted, for texts.md
	pub texts: Vec<TextUse>,
	/// open instances: (index in `uses`, depth of the instance)
	inst_stack: Vec<(usize, usize)>,
	/// layer names from the frame down to the current node
	names: Vec<String>,
}

impl<'a> Emitter<'a> {
	pub fn new(
		doc: &'a Doc,
		vars: &'a Vars,
		images: &'a HashMap<String, Vec<u8>>,
		videos: &'a HashMap<String, Vec<u8>>,
		assets_dir: &Path,
		assets_url: &str,
	) -> Self {
		Self {
			doc,
			vars,
			images,
			videos,
			assets_dir: assets_dir.to_path_buf(),
			assets_url: assets_url.to_string(),
			classes: HashMap::new(),
			rules: Vec::new(),
			class_of: HashMap::new(),
			media: Vec::new(),
			seen_rules: Default::default(),
			text_styles: BTreeMap::new(),
			written_assets: HashMap::new(),
			frame_label: (String::new(), 0.0),
			uses: Vec::new(),
			texts: Vec::new(),
			inst_stack: Vec::new(),
			names: Vec::new(),
		}
	}

	/// Emit a frame; returns the html body. Classes start fresh for every
	/// frame, text styles and assets are shared.
	pub fn frame(&mut self, n: &RNode) -> Result<String> {
		self.classes.clear();
		self.rules.clear();
		self.class_of.clear();
		self.media.clear();
		self.seen_rules.clear();
		let mut out = String::new();
		self.node(n, None, 0, &mut out)?;
		Ok(out)
	}

	pub fn css(&self) -> String {
		let mut out = String::new();
		for (selector, decls) in &self.rules {
			if decls.is_empty() {
				continue;
			}
			let _ = writeln!(out, "{selector} {{");
			for (p, v) in decls {
				let _ = writeln!(out, "\t{p}: {v};");
			}
			out.push_str("}\n\n");
		}
		for m in &self.media {
			let _ = writeln!(out, "/* {} */", m.label);
			if !m.only.is_empty() {
				let _ = writeln!(
					out,
					"/* only in this design, not in the html: {} */",
					m.only.join(", ")
				);
			}
			let _ = writeln!(out, "@media {} {{", m.query);
			for (selector, decls) in &m.rules {
				let _ = writeln!(out, "\t{selector} {{");
				for (p, v) in decls {
					let _ = writeln!(out, "\t\t{p}: {v};");
				}
				out.push_str("\t}\n\n");
			}
			out.push_str("}\n\n");
		}
		out
	}

	/// Classes for the text styles used, they go to the tokens file.
	pub fn text_styles_css(&mut self) -> String {
		let mut out = String::new();
		let styles: Vec<(Guid, String)> = self
			.text_styles
			.iter()
			.map(|(g, c)| (*g, c.clone()))
			.collect();
		for (g, class) in styles {
			let Some(style) = self.doc.node(g) else {
				continue;
			};
			let n = RNode {
				guid: g,
				ty: "TEXT".into(),
				name: String::new(),
				p: style.clone(),
				children: vec![],
				instance: None,
			};
			let decls = self.font_decls(&n);
			let _ = writeln!(
				out,
				"/* text style \"{}\" */\n.{class} {{",
				self.doc.name(g)
			);
			for (p, v) in decls {
				let _ = writeln!(out, "\t{p}: {v};");
			}
			out.push_str("}\n\n");
		}
		out
	}

	fn class_for(&mut self, name: &str, decls: &Decls) -> String {
		let mut base = kebab(name);
		if base.is_empty() {
			base = "node".into();
		}
		if base.starts_with(|c: char| c.is_ascii_digit()) {
			base = format!("n-{base}");
		}
		let list = self.classes.entry(base.clone()).or_default();
		let idx = match list.iter().position(|d| d == decls) {
			Some(i) => i,
			None => {
				list.push(decls.clone());
				let i = list.len() - 1;
				let class = if i == 0 {
					base.clone()
				} else {
					format!("{base}-{}", i + 1)
				};
				self.rules.push((format!(".{class}"), decls.clone()));
				i
			}
		};
		if idx == 0 {
			base
		} else {
			format!("{base}-{}", idx + 1)
		}
	}

	/// What a node becomes in HTML.
	fn kind(&self, n: &RNode) -> Kind {
		if svg::is_graphic(n) {
			return Kind::Graphic;
		}
		if matches!(
			n.ty.as_str(),
			"RECTANGLE" | "ROUNDED_RECTANGLE" | "ELLIPSE" | "FRAME" | "INSTANCE"
		) && n.children.is_empty()
			&& let Some(img) = svg::paints(n, "fillPaints")
				.last()
				.filter(|p| p["type"] == "IMAGE" || p["type"] == "VIDEO")
				.cloned()
		{
			return if img["type"] == "VIDEO" {
				Kind::Video(img)
			} else {
				Kind::Image(img)
			};
		}
		if n.ty == "TEXT" {
			return Kind::Text;
		}
		Kind::Box
	}

	/// All declarations of a node.
	fn decls(&mut self, n: &RNode, parent: Option<&RNode>) -> Decls {
		let mut decls = Decls::new();
		self.layout_decls(n, parent, &mut decls);
		match self.kind(n) {
			Kind::Graphic => {
				self.effect_decls(n, &mut decls);
				push_opacity(n, &mut decls);
			}
			Kind::Video(_) => {
				push(&mut decls, "object-fit", "cover");
				self.box_decls(n, &mut decls, true);
			}
			Kind::Image(img) => {
				let fit = match img.get("imageScaleMode").and_then(Value::as_str) {
					Some("FIT") => "contain",
					Some("STRETCH") => "fill",
					_ => "cover",
				};
				push(&mut decls, "object-fit", fit);
				self.box_decls(n, &mut decls, true);
			}
			Kind::Text => {
				self.text_decls(n, &mut decls);
				push_opacity(n, &mut decls);
				self.effect_decls(n, &mut decls);
			}
			Kind::Box => {
				self.box_decls(n, &mut decls, false);
				if n.children.iter().any(|c| !in_flow(c, n)) {
					push(&mut decls, "position", "relative");
				}
			}
		}
		decls
	}

	fn node(
		&mut self,
		n: &RNode,
		parent: Option<&RNode>,
		depth: usize,
		out: &mut String,
	) -> Result<()> {
		let pad = "\t".repeat(depth);
		// notes for the developers, listed at the top of the file too
		if grid::is_note(n, parent, depth) {
			let text = grid::note_text(n).replace("--", "-");
			let _ = writeln!(out, "{pad}<!-- designer note: {text} -->");
			return Ok(());
		}
		let decls = self.decls(n, parent);

		// what the layers of instances look like, for components.md
		self.names.truncate(depth);
		// siblings with the same name get their position: `Photo[2]`
		let same = parent
			.map(|p| p.children.iter().filter(|c| c.name == n.name).count())
			.unwrap_or(1);
		if same > 1 {
			let i = parent
				.map(|p| {
					p.children
						.iter()
						.filter(|c| c.name == n.name)
						.position(|c| std::ptr::eq(c, n))
						.unwrap_or(0)
				})
				.unwrap_or(0);
			self.names.push(format!("{}[{}]", n.name, i + 1));
		} else {
			self.names.push(n.name.clone());
		}
		if let Some(info) = &n.instance {
			self.uses.push(Use {
				component: info.component.clone(),
				variant: info.variant.clone(),
				frame: self.frame_label.0.clone(),
				width: self.frame_label.1,
				layers: Vec::new(),
			});
			self.inst_stack.push((self.uses.len() - 1, depth));
		}
		let mut recorded = None;
		if let Some(&(i, d0)) = self.inst_stack.last() {
			let path = self.names[d0 + 1..].join("/");
			self.uses[i].layers.push((path, decls.clone()));
			recorded = Some((i, self.uses[i].layers.len() - 1));
		}

		let mut attrs = String::new();
		if let Some(info) = &n.instance {
			let _ = write!(attrs, " data-component=\"{}\"", escape(&info.component));
			if let Some(v) = &info.variant {
				let _ = write!(attrs, " data-variant=\"{}\"", escape(v));
			}
			if !info.props.is_empty() {
				let props: Vec<String> = info
					.props
					.iter()
					.map(|(k, v)| format!("{k}: {}", short(v, 40)))
					.collect();
				let _ = write!(attrs, " data-props=\"{}\"", escape(&props.join("; ")));
			}
		}
		attrs.push_str(&self.link_attrs(n));
		if parent.is_none() {
			attrs.push_str(&self.mode_attrs(n));
		}

		match self.kind(n) {
			Kind::Graphic => {
				let svg = svg::render(self.doc, n);
				let file = self.asset(&kebab(&n.name), "svg", svg.as_bytes())?;
				let class = self.class_for(&n.name, &decls);
				let _ = writeln!(
					out,
					"{pad}<img class=\"{class}\" src=\"{file}\" alt=\"\"{attrs}>"
				);
				self.class_of.insert(n, class);
			}
			Kind::Video(img) => {
				let src = self.image_asset(&img)?;
				let video = self.video_asset(&img)?;
				let class = self.class_for(&n.name, &decls);
				let _ = writeln!(
					out,
					"{pad}<video class=\"{class}\" src=\"{video}\" poster=\"{src}\" autoplay muted loop playsinline{attrs}></video>"
				);
				self.class_of.insert(n, class);
			}
			Kind::Image(img) => {
				let src = self.image_asset(&img)?;
				let class = self.class_for(&n.name, &decls);
				let _ = writeln!(
					out,
					"{pad}<img class=\"{class}\" src=\"{src}\" alt=\"{}\"{attrs}>",
					escape(&n.name)
				);
				self.class_of.insert(n, class);
			}
			Kind::Text => {
				let (tag, style_class) = self.text_tag(n);
				if let (Some((i, l)), Some(sc)) = (recorded, &style_class) {
					self.uses[i].layers[l]
						.1
						.push(("text style".into(), sc.clone()));
				}
				// text layers are often named after their content
				let auto =
					n.get("autoRename")
						.and_then(Value::as_bool)
						.unwrap_or(false) || n.name.chars().count() > 32;
				let name = match (auto, parent) {
					(true, Some(p)) => format!("{} text", p.name),
					(true, None) => "text".into(),
					(false, _) => n.name.clone(),
				};
				let class = self.class_for(&name, &decls);
				let all = match style_class {
					Some(s) => format!("{class} {s}"),
					None => class.clone(),
				};
				let content = self.text_content(n);
				let component = self
					.inst_stack
					.last()
					.map(|&(i, _)| self.uses[i].component.clone());
				self.texts.push(TextUse {
					frame: self.frame_label.0.clone(),
					path: self.names[1..].join(" › "),
					component,
					text: n
						.get("textData")
						.and_then(|t| t["characters"].as_str())
						.unwrap_or("")
						.to_string(),
					html: content.clone(),
				});
				let _ =
					writeln!(out, "{pad}<{tag} class=\"{all}\"{attrs}>{content}</{tag}>");
				self.class_of.insert(n, class);
			}
			Kind::Box => {
				let class = self.class_for(&n.name, &decls);
				self.class_of.insert(n, class.clone());
				if n.children.is_empty() {
					let _ = writeln!(out, "{pad}<div class=\"{class}\"{attrs}></div>");
				} else {
					let _ = writeln!(out, "{pad}<div class=\"{class}\"{attrs}>");
					for c in &n.children {
						self.node(c, Some(n), depth + 1, out)?;
					}
					let _ = writeln!(out, "{pad}</div>");
				}
			}
		}

		if n.instance.is_some() {
			self.hover_rules(n);
			self.inst_stack.pop();
		}
		Ok(())
	}

	/// Prototype links: clicks that navigate to or scroll to a frame.
	fn link_attrs(&self, n: &RNode) -> String {
		let mut s = String::new();
		for (event, action) in interactions(n) {
			if event != "ON_CLICK" {
				continue;
			}
			let Some(target) = action.get("transitionNodeID").and_then(Guid::from_value)
			else {
				continue;
			};
			if self.doc.node(target).is_none() {
				continue;
			}
			let attr = match action.get("navigationType").and_then(Value::as_str) {
				Some("NAVIGATE") => "data-link",
				Some("SCROLL_TO") => "data-scroll-to",
				Some("OVERLAY") => "data-overlay",
				_ => continue,
			};
			let _ = write!(s, " {attr}=\"{}\"", escape(self.doc.name(target)));
		}
		s
	}

	/// `:hover` rules for an instance whose component swaps to another
	/// variant on hover: the difference between the two variants.
	fn hover_rules(&mut self, inst: &RNode) {
		let Some((target, action)) = interactions(inst).find_map(|(event, action)| {
			if event != "ON_HOVER"
				|| action.get("navigationType").and_then(Value::as_str)
					!= Some("SWAP_STATE")
			{
				return None;
			}
			let t = action.get("transitionNodeID").and_then(Guid::from_value)?;
			Some((t, action.clone()))
		}) else {
			return;
		};
		let source = inst
			.get("overriddenSymbolID")
			.and_then(Guid::from_value)
			.or_else(|| {
				inst.get("symbolData")?
					.get("symbolID")
					.and_then(Guid::from_value)
			});
		let (Some(source), Some(class)) =
			(source, self.class_of.get(&(inst as *const RNode)).cloned())
		else {
			return;
		};
		if self.doc.ty(target) != "SYMBOL" || target == source {
			return;
		}
		let (Some(from), Some(to)) = (self.doc.resolve(source), self.doc.resolve(target))
		else {
			return;
		};

		let base = format!(".{class}");
		let mut changes = Vec::new();
		self.tree_diff(inst, &from, &to, None, "hover", &mut changes);
		let mut rules: Vec<(String, Decls)> = Vec::new();
		let mut animated: Vec<String> = Vec::new();
		for ch in changes {
			let Some(c) = self.class_of.get(&ch.node).cloned() else {
				continue;
			};
			let (selector, plain) = if std::ptr::eq(ch.node, inst) {
				(format!("{base}:hover"), base.clone())
			} else {
				(format!("{base}:hover .{c}"), format!("{base} .{c}"))
			};
			let decls = if ch.gone {
				vec![("opacity".into(), "0".into())]
			} else {
				ch.decls
			};
			rules.push((selector, decls));
			animated.push(plain);
		}
		if rules.is_empty() {
			return;
		}

		// the transition of the interaction on everything that changes
		if let Some(t) = transition(&action) {
			animated.dedup();
			let key = format!("{}|{t}", animated.join(","));
			if self.seen_rules.insert(key) {
				self.rules
					.push((animated.join(",\n"), vec![("transition".into(), t)]));
			}
		}
		for (selector, decls) in rules {
			let key = format!("{selector}|{decls:?}");
			if self.seen_rules.insert(key) {
				self.rules.push((selector, decls));
			}
		}
	}

	/// Walk an emitted tree (`actual`) along two versions of it and
	/// collect what changes from `from` to `to`, per emitted node.
	/// Children are matched by name and position among equal names.
	fn tree_diff(
		&mut self,
		actual: &RNode,
		from: &RNode,
		to: &RNode,
		parents: Option<(&RNode, &RNode)>,
		suffix: &str,
		out: &mut Vec<Change>,
	) {
		let a = self.decls(from, parents.map(|p| p.0));
		let b = self.decls(to, parents.map(|p| p.1));
		let mut diff = Decls::new();
		for (k, v) in &b {
			if a.iter().find(|(ka, _)| ka == k).map(|(_, va)| va) != Some(v) {
				push(&mut diff, k, v.clone());
			}
		}
		for (k, _) in &a {
			if !b.iter().any(|(kb, _)| kb == k) {
				push(&mut diff, k, "unset");
			}
		}
		// vectors that are drawn differently swap their image
		if matches!(self.kind(from), Kind::Graphic)
			&& matches!(self.kind(to), Kind::Graphic)
		{
			let (sa, sb) = (svg::render(self.doc, from), svg::render(self.doc, to));
			if sa != sb
				&& let Ok(url) = self.asset(
					&format!("{}-{suffix}", kebab(&to.name)),
					"svg",
					sb.as_bytes(),
				) {
				push(&mut diff, "content", format!("url(\"{url}\")"));
			}
		}
		if !diff.is_empty() {
			out.push(Change {
				node: actual,
				decls: diff,
				gone: false,
			});
		}

		let pick = |list: &'_ [RNode], name: &str, nth: usize| -> Option<usize> {
			list.iter()
				.enumerate()
				.filter(|(_, c)| c.name == name)
				.nth(nth)
				.map(|(i, _)| i)
		};
		let mut seen: HashMap<&str, usize> = HashMap::new();
		for c in &actual.children {
			let nth = seen.entry(c.name.as_str()).or_default();
			let i = *nth;
			*nth += 1;
			let Some(fi) = pick(&from.children, &c.name, i) else {
				continue;
			};
			match pick(&to.children, &c.name, i) {
				Some(ti) => self.tree_diff(
					c,
					&from.children[fi],
					&to.children[ti],
					Some((from, to)),
					suffix,
					out,
				),
				None => out.push(Change {
					node: c,
					decls: Decls::new(),
					gone: true,
				}),
			}
		}
	}

	/// Media query rules for the same frame designed at another width.
	/// `frame` must be the tree that was just emitted.
	pub fn responsive(&mut self, frame: &RNode, other: &RNode, query: &str, label: &str) {
		let mut changes = Vec::new();
		self.tree_diff(frame, frame, other, None, label, &mut changes);
		// a class can stand for several layers: only use a change when all
		// of them change the same way
		let mut uses: HashMap<String, usize> = HashMap::new();
		for c in self.class_of.values() {
			*uses.entry(c.clone()).or_default() += 1;
		}
		let mut by_class: Vec<(String, Vec<Decls>)> = Vec::new();
		for ch in changes {
			let Some(class) = self.class_of.get(&ch.node).cloned() else {
				continue;
			};
			let mut decls = ch.decls;
			if ch.gone {
				decls = vec![("display".into(), "none".into())];
			}
			// the root keeps the width of the design, let it follow the viewport
			if std::ptr::eq(ch.node, frame) {
				for d in decls.iter_mut() {
					if d.0 == "width" {
						d.1 = "100%".into();
					}
				}
			}
			match by_class.iter_mut().find(|(c, _)| *c == class) {
				Some((_, list)) => list.push(decls),
				None => by_class.push((class, vec![decls])),
			}
		}
		let mut rules: Vec<(String, Decls)> = Vec::new();
		let mut skipped = 0;
		for (class, list) in by_class {
			let same = list.iter().all(|d| *d == list[0]);
			if !same || list.len() != uses.get(&class).copied().unwrap_or(0) {
				skipped += 1;
				continue;
			}
			rules.push((format!(".{class}"), list.into_iter().next().unwrap()));
		}
		let label = if skipped > 0 {
			format!(
				"{label}; {skipped} classes differ between their layers and are left out"
			)
		} else {
			label.to_string()
		};
		// layers that only exist at this width
		let mut only = Vec::new();
		collect_new(frame, other, "", &mut only);
		self.media.push(Media {
			query: query.to_string(),
			label,
			rules,
			only,
		});
	}

	fn mode_attrs(&self, n: &RNode) -> String {
		let mut s = String::new();
		let entries = n
			.get("variableModeBySetMap")
			.and_then(|m| m.get("entries"))
			.and_then(Value::as_array);
		for e in entries.into_iter().flatten() {
			let (Some(set), Some(mode)) = (
				e.get("variableSetID").and_then(Guid::from_value),
				e.get("variableModeID").and_then(Guid::from_value),
			) else {
				continue;
			};
			if let Some((k, v)) = self.vars.mode_attr(set, mode) {
				let _ = write!(s, " {k}=\"{v}\"");
			}
		}
		s
	}

	// --- variables

	/// The variable bound to a field of a node, as `var(--…)`.
	fn bound(&self, n: &RNode, field: &str) -> Option<String> {
		for map in ["parameterConsumptionMap", "variableConsumptionMap"] {
			let entries = n
				.get(map)
				.and_then(|m| m.get("entries"))
				.and_then(Value::as_array);
			for e in entries.into_iter().flatten() {
				if e.get("variableField").and_then(Value::as_str) != Some(field) {
					continue;
				}
				let data = &e["variableData"];
				if let Some(id) = Vars::alias_of(data) {
					return self.vars.reference(id);
				}
			}
		}
		None
	}

	fn bound_id(&self, n: &RNode, field: &str) -> Option<Guid> {
		for map in ["parameterConsumptionMap", "variableConsumptionMap"] {
			let entries = n
				.get(map)
				.and_then(|m| m.get("entries"))
				.and_then(Value::as_array);
			for e in entries.into_iter().flatten() {
				if e.get("variableField").and_then(Value::as_str) != Some(field) {
					continue;
				}
				let data = &e["variableData"];
				let data = data
					.pointer("/value/fontStyleValue/asString")
					.unwrap_or(data);
				if let Some(id) = Vars::alias_of(data) {
					return Some(id);
				}
			}
		}
		None
	}

	/// A length, from a bound variable or the raw value.
	fn len(&self, n: &RNode, field: &str, raw: f64) -> String {
		self.bound(n, field).unwrap_or_else(|| px(raw))
	}

	// --- layout

	fn layout_decls(&self, n: &RNode, parent: Option<&RNode>, d: &mut Decls) {
		let (w, h) = n.size();
		let stack = n
			.s("stackMode")
			.filter(|m| *m == "HORIZONTAL" || *m == "VERTICAL");
		let parent_stack = parent.and_then(|p| {
			p.s("stackMode")
				.filter(|m| *m == "HORIZONTAL" || *m == "VERTICAL")
		});
		let flow = parent.map(|p| in_flow(n, p)).unwrap_or(true);

		// own auto layout
		if let Some(mode) = stack {
			push(d, "display", "flex");
			if mode == "VERTICAL" {
				push(d, "flex-direction", "column");
			}
			if n.s("stackWrap") == Some("WRAP") {
				push(d, "flex-wrap", "wrap");
			}
			let justify = n.s("stackPrimaryAlignItems").unwrap_or("MIN");
			match justify {
				"CENTER" => push(d, "justify-content", "center"),
				"MAX" => push(d, "justify-content", "flex-end"),
				"SPACE_BETWEEN" => push(d, "justify-content", "space-between"),
				"SPACE_EVENLY" | "SPACE_EVENLY_CSS" => {
					push(d, "justify-content", "space-evenly")
				}
				"SPACE_AROUND" => push(d, "justify-content", "space-around"),
				_ => {}
			}
			match n.s("stackCounterAlignItems").unwrap_or("MIN") {
				"CENTER" => push(d, "align-items", "center"),
				"MAX" => push(d, "align-items", "flex-end"),
				"BASELINE" => push(d, "align-items", "baseline"),
				_ => push(d, "align-items", "flex-start"),
			}
			let gap = n.f("stackSpacing").unwrap_or(0.0);
			let gap_var = self.bound(n, "STACK_SPACING");
			if justify != "SPACE_BETWEEN" && (gap != 0.0 || gap_var.is_some()) {
				push(d, "gap", gap_var.unwrap_or_else(|| px(gap)));
			}
			if n.s("stackWrap") == Some("WRAP")
				&& let Some(cg) = n.f("stackCounterSpacing")
			{
				push(d, "row-gap", self.len(n, "STACK_COUNTER_SPACING", cg));
			}
			let left = n.f("stackHorizontalPadding").unwrap_or(0.0);
			let top = n.f("stackVerticalPadding").unwrap_or(0.0);
			let right = n.f("stackPaddingRight").unwrap_or(left);
			let bottom = n.f("stackPaddingBottom").unwrap_or(top);
			let p = [
				self.len(n, "STACK_PADDING_TOP", top),
				self.len(n, "STACK_PADDING_RIGHT", right),
				self.len(n, "STACK_PADDING_BOTTOM", bottom),
				self.len(n, "STACK_PADDING_LEFT", left),
			];
			if p.iter().any(|v| v != "0") {
				push(d, "padding", shorthand(&p));
			}
		}

		// position
		if let (false, Some(p)) = (flow, parent) {
			let t = n.transform();
			let (pw, ph) = p.size();
			push(d, "position", "absolute");
			match n.s("horizontalConstraint") {
				Some("MAX") => push(d, "right", px(pw - t[2] - w)),
				Some("STRETCH") => {
					push(d, "left", px(t[2]));
					push(d, "right", px(pw - t[2] - w));
				}
				Some("CENTER") => {
					push(
						d,
						"left",
						format!("calc(50% + {})", px(t[2] + w / 2.0 - pw / 2.0)),
					);
					push(d, "translate", "-50% 0");
				}
				_ => push(d, "left", px(t[2])),
			}
			match n.s("verticalConstraint") {
				Some("MAX") => push(d, "bottom", px(ph - t[5] - h)),
				Some("STRETCH") => {
					push(d, "top", px(t[5]));
					push(d, "bottom", px(ph - t[5] - h));
				}
				_ => push(d, "top", px(t[5])),
			}
			let angle = t[3].atan2(t[0]).to_degrees();
			if angle.abs() > 0.01 {
				push(d, "transform-origin", "0 0");
				push(d, "rotate", format!("{}deg", num(angle)));
			}
		}

		// Figma paints later layers on top, css paints positioned elements
		// above the flow: lift flow children that follow an absolute one
		if flow
			&& let Some(p) = parent
			&& p.children
				.iter()
				.take_while(|c| !std::ptr::eq(*c, n))
				.any(|c| !in_flow(c, p))
		{
			push(d, "position", "relative");
		}

		// sizes
		let text_auto = n.s("textAutoResize");
		let is_text = n.ty == "TEXT";
		let hug_x = if is_text {
			text_auto == Some("WIDTH_AND_HEIGHT")
		} else {
			match stack {
				Some("HORIZONTAL") => n.s("stackPrimarySizing") != Some("FIXED"),
				Some("VERTICAL") => matches!(
					n.s("stackCounterSizing"),
					Some("RESIZE_TO_FIT" | "RESIZE_TO_FIT_WITH_IMPLICIT_SIZE")
				),
				_ => false,
			}
		};
		let hug_y = if is_text {
			matches!(text_auto, Some("WIDTH_AND_HEIGHT" | "HEIGHT"))
		} else {
			match stack {
				Some("VERTICAL") => n.s("stackPrimarySizing") != Some("FIXED"),
				Some("HORIZONTAL") => matches!(
					n.s("stackCounterSizing"),
					Some("RESIZE_TO_FIT" | "RESIZE_TO_FIT_WITH_IMPLICIT_SIZE")
				),
				_ => false,
			}
		};

		let grow = n.f("stackChildPrimaryGrow").unwrap_or(0.0) > 0.0;
		let stretch = n.s("stackChildAlignSelf") == Some("STRETCH");
		let (mut fill_x, mut fill_y) = (false, false);
		if flow {
			match parent_stack {
				Some("HORIZONTAL") => {
					fill_x = grow;
					fill_y = stretch;
				}
				Some("VERTICAL") => {
					fill_y = grow;
					fill_x = stretch;
				}
				_ => {}
			}
		}
		if grow && flow && parent_stack.is_some() {
			push(d, "flex", "1 1 0");
			push(
				d,
				if parent_stack == Some("HORIZONTAL") {
					"min-width"
				} else {
					"min-height"
				},
				"0",
			);
		}
		let has_max_x = n
			.get("maxSize")
			.and_then(|m| m.pointer("/value/x"))
			.and_then(Value::as_f64)
			.is_some();
		if stretch && flow && parent_stack.is_some() {
			// a stretched child with a max width stays aligned in Figma
			if parent_stack == Some("VERTICAL") && has_max_x {
				push(d, "width", "100%");
			} else {
				push(d, "align-self", "stretch");
			}
		}

		if parent.is_none() {
			push(d, "width", px(w));
			// exports are clipped to the frame
			push(d, "overflow", "hidden");
		} else {
			let horizontal_stretch =
				!flow && n.s("horizontalConstraint") == Some("STRETCH");
			if !fill_x && !hug_x && !horizontal_stretch {
				push(d, "width", self.len(n, "WIDTH", w));
			}
			let vertical_stretch = !flow && n.s("verticalConstraint") == Some("STRETCH");
			if !fill_y && !hug_y && !vertical_stretch {
				push(d, "height", self.len(n, "HEIGHT", h));
			}
			// only filling children shrink in Figma
			if flow && parent_stack.is_some() && !grow {
				push(d, "flex-shrink", "0");
			}
		}
		if is_text && text_auto == Some("WIDTH_AND_HEIGHT") {
			push(d, "white-space", "nowrap");
		}

		for (key, axis, prop) in [
			("minSize", "x", "min-width"),
			("minSize", "y", "min-height"),
			("maxSize", "x", "max-width"),
			("maxSize", "y", "max-height"),
		] {
			if let Some(v) = n
				.get(key)
				.and_then(|m| m.pointer(&format!("/value/{axis}")))
				.and_then(Value::as_f64)
				&& v > 0.0
			{
				let field = format!(
					"{}_{}",
					if key == "minSize" { "MIN" } else { "MAX" },
					if axis == "x" { "WIDTH" } else { "HEIGHT" }
				);
				push(d, prop, self.len(n, &field, v));
			}
		}
	}

	// --- box

	fn box_decls(&mut self, n: &RNode, d: &mut Decls, is_img: bool) {
		if !is_img {
			self.fill_decls(n, d);
		}
		self.stroke_decls(n, d);
		self.radius_decls(n, d);
		self.effect_decls(n, d);
		push_opacity(n, d);
		push_blend(n, d);
		if matches!(n.ty.as_str(), "ELLIPSE") {
			push(d, "border-radius", "50%");
		}
		// clipping, only where something sticks out
		let clips = matches!(n.ty.as_str(), "FRAME" | "INSTANCE" | "SYMBOL")
			&& n.get("frameMaskDisabled").and_then(Value::as_bool) != Some(true);
		if clips && overflows(n) {
			push(d, "overflow", "hidden");
		}
	}

	fn paint_css(&mut self, p: &Value, n: &RNode, as_layer: bool) -> Option<String> {
		let op = p.get("opacity").and_then(Value::as_f64).unwrap_or(1.0);
		match p.get("type").and_then(Value::as_str)? {
			"SOLID" => {
				let var = p
					.get("colorVar")
					.and_then(Vars::alias_of)
					.and_then(|id| self.vars.reference(id));
				let c = match var {
					Some(v) if op < 0.999 => format!(
						"color-mix(in srgb, {v} {}%, transparent)",
						num(op * 100.0)
					),
					Some(v) => v,
					None => color(p.get("color")?, op),
				};
				Some(if as_layer {
					format!("linear-gradient({c}, {c})")
				} else {
					c
				})
			}
			"GRADIENT_LINEAR" | "GRADIENT_RADIAL" | "GRADIENT_ANGULAR"
			| "GRADIENT_DIAMOND" => {
				let stops: Vec<String> = p
					.get("stops")?
					.as_array()?
					.iter()
					.map(|s| {
						format!(
							"{} {}%",
							color(&s["color"], op),
							num(s["position"].as_f64().unwrap_or(0.0) * 100.0)
						)
					})
					.collect();
				if p["type"] == "GRADIENT_LINEAR" {
					let angle = gradient_angle(p, n);
					Some(format!(
						"linear-gradient({}deg, {})",
						num(angle),
						stops.join(", ")
					))
				} else {
					Some(format!("radial-gradient({})", stops.join(", ")))
				}
			}
			"IMAGE" => {
				let src = self.image_asset(p).ok()?;
				Some(format!("url(\"{src}\")"))
			}
			_ => None,
		}
	}

	fn fill_decls(&mut self, n: &RNode, d: &mut Decls) {
		let fills: Vec<Value> = svg::paints(n, "fillPaints").cloned().collect();
		if fills.is_empty() {
			return;
		}
		if fills.len() == 1 && fills[0]["type"] == "SOLID" {
			if let Some(c) = self.paint_css(&fills[0], n, false) {
				push(d, "background-color", c);
			}
			return;
		}
		// css lists the top layer first
		let mut layers = Vec::new();
		let mut image = false;
		for p in fills.iter().rev() {
			if p["type"] == "IMAGE" {
				image = true;
			}
			if let Some(c) = self.paint_css(p, n, true) {
				layers.push(c);
			}
		}
		if !layers.is_empty() {
			push(d, "background", layers.join(", "));
		}
		if image {
			push(d, "background-size", "cover");
			push(d, "background-position", "center");
		}
	}

	fn stroke_decls(&mut self, n: &RNode, d: &mut Decls) {
		let Some(p) = svg::paints(n, "strokePaints").last().cloned() else {
			return;
		};
		let Some(c) = self.paint_css(&p, n, false) else {
			return;
		};
		let w = n.f("strokeWeight").unwrap_or(1.0);
		if w <= 0.0 {
			return;
		}
		let style = if n
			.get("dashPattern")
			.and_then(Value::as_array)
			.is_some_and(|a| !a.is_empty())
		{
			"dashed"
		} else {
			"solid"
		};
		let sides = [
			n.f("borderTopWeight"),
			n.f("borderRightWeight"),
			n.f("borderBottomWeight"),
			n.f("borderLeftWeight"),
		];
		let independent = sides.iter().all(Option::is_some)
			&& sides.iter().any(|s| (s.unwrap() - w).abs() > 0.01);
		if n.s("strokeAlign") == Some("OUTSIDE") && !independent {
			push(d, "outline", format!("{} {style} {c}", px(w)));
			return;
		}
		if independent {
			for (side, v) in ["top", "right", "bottom", "left"].iter().zip(sides) {
				let v = v.unwrap_or(0.0);
				if v > 0.0 {
					push(
						d,
						&format!("border-{side}"),
						format!("{} {style} {c}", px(v)),
					);
				}
			}
		} else {
			push(d, "border", format!("{} {style} {c}", px(w)));
		}
	}

	fn radius_decls(&self, n: &RNode, d: &mut Decls) {
		let corners = [
			(
				"rectangleTopLeftCornerRadius",
				"RECTANGLE_TOP_LEFT_CORNER_RADIUS",
			),
			(
				"rectangleTopRightCornerRadius",
				"RECTANGLE_TOP_RIGHT_CORNER_RADIUS",
			),
			(
				"rectangleBottomRightCornerRadius",
				"RECTANGLE_BOTTOM_RIGHT_CORNER_RADIUS",
			),
			(
				"rectangleBottomLeftCornerRadius",
				"RECTANGLE_BOTTOM_LEFT_CORNER_RADIUS",
			),
		];
		let base = n.f("cornerRadius").unwrap_or(0.0);
		let independent = n
			.get("rectangleCornerRadiiIndependent")
			.and_then(Value::as_bool)
			.unwrap_or(false);
		let vals: Vec<String> = corners
			.iter()
			.map(|(k, field)| {
				let raw = if independent {
					n.f(k).unwrap_or(base)
				} else {
					base
				};
				self.bound(n, field)
					.or_else(|| self.bound(n, "CORNER_RADIUS"))
					.unwrap_or_else(|| px(raw))
			})
			.collect();
		if vals.iter().any(|v| v != "0") {
			let v = if vals.iter().all(|v| *v == vals[0]) {
				vals[0].clone()
			} else {
				vals.join(" ")
			};
			push(d, "border-radius", v);
		}
	}

	fn effect_decls(&self, n: &RNode, d: &mut Decls) {
		let mut shadows = Vec::new();
		let effects = n.get("effects").and_then(Value::as_array);
		for e in effects.into_iter().flatten() {
			if e.get("visible").and_then(Value::as_bool) == Some(false) {
				continue;
			}
			let r = e.get("radius").and_then(Value::as_f64).unwrap_or(0.0);
			match e.get("type").and_then(Value::as_str) {
				Some(t @ ("DROP_SHADOW" | "INNER_SHADOW")) => {
					let x = e
						.pointer("/offset/x")
						.and_then(Value::as_f64)
						.unwrap_or(0.0);
					let y = e
						.pointer("/offset/y")
						.and_then(Value::as_f64)
						.unwrap_or(0.0);
					let s = e.get("spread").and_then(Value::as_f64).unwrap_or(0.0);
					let c = color(&e["color"], 1.0);
					let inset = if t == "INNER_SHADOW" { "inset " } else { "" };
					shadows.push(if n.ty == "TEXT" {
						format!("{} {} {} {c}", px(x), px(y), px(r))
					} else {
						format!("{inset}{} {} {} {} {c}", px(x), px(y), px(r), px(s))
					});
				}
				Some("FOREGROUND_BLUR") => {
					push(d, "filter", format!("blur({})", px(r / 2.0)))
				}
				Some("BACKGROUND_BLUR") => {
					push(d, "backdrop-filter", format!("blur({})", px(r / 2.0)))
				}
				_ => {}
			}
		}
		if !shadows.is_empty() {
			let prop = if n.ty == "TEXT" {
				"text-shadow"
			} else {
				"box-shadow"
			};
			push(d, prop, shadows.join(", "));
		}
	}

	// --- text

	/// Font properties of a text node or text style.
	fn font_decls(&mut self, n: &RNode) -> Decls {
		let mut d = Decls::new();
		let family = self.bound(n, "FONT_FAMILY").or_else(|| {
			n.get("fontName")
				.and_then(|f| f.get("family"))
				.and_then(Value::as_str)
				.map(|f| format!("\"{f}\""))
		});
		if let Some(f) = family {
			push(&mut d, "font-family", f);
		}
		let style = self
			.bound_id(n, "FONT_STYLE")
			.and_then(|id| self.vars.resolve_string(id))
			.or_else(|| {
				n.get("fontName")
					.and_then(|f| f.get("style"))
					.and_then(Value::as_str)
					.map(str::to_string)
			});
		if let Some(style) = style {
			let (weight, italic) = font_weight(&style);
			push(&mut d, "font-weight", weight.to_string());
			if italic {
				push(&mut d, "font-style", "italic");
			}
		}
		if let Some(size) = n.f("fontSize") {
			push(&mut d, "font-size", self.len(n, "FONT_SIZE", size));
		}
		if let Some(lh) = n.get("lineHeight") {
			let v = lh.get("value").and_then(Value::as_f64).unwrap_or(100.0);
			let css = match self.bound(n, "LINE_HEIGHT") {
				Some(var) => Some(var),
				None => match lh.get("units").and_then(Value::as_str) {
					Some("PIXELS") => Some(px(v)),
					Some("PERCENT") if (v - 100.0).abs() > 0.01 => Some(num(v / 100.0)),
					Some("RAW") => Some(num(v)),
					_ => None,
				},
			};
			if let Some(css) = css {
				push(&mut d, "line-height", css);
			}
		}
		if let Some(ls) = n.get("letterSpacing") {
			let v = ls.get("value").and_then(Value::as_f64).unwrap_or(0.0);
			if v.abs() > 0.001 {
				let css = match ls.get("units").and_then(Value::as_str) {
					Some("PIXELS") => px(v),
					_ => format!("{}em", num(v / 100.0)),
				};
				push(&mut d, "letter-spacing", css);
			}
		}
		match n.s("textCase") {
			Some("UPPER") => push(&mut d, "text-transform", "uppercase"),
			Some("LOWER") => push(&mut d, "text-transform", "lowercase"),
			Some("TITLE") => push(&mut d, "text-transform", "capitalize"),
			Some("SMALL_CAPS" | "SMALL_CAPS_FORCED") => {
				push(&mut d, "font-variant", "small-caps")
			}
			_ => {}
		}
		match n.s("textDecoration") {
			Some("UNDERLINE") => push(&mut d, "text-decoration", "underline"),
			Some("STRIKETHROUGH") => push(&mut d, "text-decoration", "line-through"),
			_ => {}
		}
		d
	}

	/// The tag of a text node and the class of its text style.
	fn text_tag(&mut self, n: &RNode) -> (String, Option<String>) {
		let mut d = Decls::new();
		self.text_decls(n, &mut d)
	}

	/// Text declarations; returns the tag and the text style class.
	fn text_decls(&mut self, n: &RNode, d: &mut Decls) -> (String, Option<String>) {
		let style = n
			.get("styleIdForText")
			.and_then(Guid::from_value)
			.filter(|g| self.doc.node(*g).is_some());
		let mut tag = "p".to_string();
		let mut style_class = None;
		match style {
			Some(g) => {
				let name = self.doc.name(g).to_string();
				let class = format!("t-{}", kebab(&name));
				self.text_styles.insert(g, class.clone());
				style_class = Some(class);
				tag = heading_tag(&name).unwrap_or("p").to_string();
				// changing a styled property detaches the style in Figma, so
				// the node's own font values are only stale copies
			}
			None => {
				for (k, v) in self.font_decls(n) {
					push(d, &k, v);
				}
			}
		}

		if let Some(p) = svg::paints(n, "fillPaints").last().cloned()
			&& let Some(c) = self.paint_css(&p, n, false)
		{
			if p["type"] == "SOLID" {
				push(d, "color", c);
			} else {
				push(d, "background", c);
				push(d, "background-clip", "text");
				push(d, "color", "transparent");
			}
		}
		match n.s("textAlignVertical") {
			Some("CENTER") => push(d, "align-content", "center"),
			Some("BOTTOM") => push(d, "align-content", "end"),
			_ => {}
		}
		match n.s("textAlignHorizontal") {
			Some("CENTER") => push(d, "text-align", "center"),
			Some("RIGHT") => push(d, "text-align", "right"),
			Some("JUSTIFIED") => push(d, "text-align", "justify"),
			_ => {}
		}
		(tag, style_class)
	}

	fn text_content(&mut self, n: &RNode) -> String {
		let Some(td) = n.get("textData") else {
			return String::new();
		};
		let chars: Vec<char> = td["characters"].as_str().unwrap_or("").chars().collect();
		let ids: Vec<u64> = td
			.get("characterStyleIDs")
			.and_then(Value::as_array)
			.map(|a| a.iter().map(|v| v.as_u64().unwrap_or(0)).collect())
			.unwrap_or_default();
		let table: HashMap<u64, Value> = td
			.get("styleOverrideTable")
			.and_then(Value::as_array)
			.into_iter()
			.flatten()
			.filter_map(|s| Some((s.get("styleID")?.as_u64()?, s.clone())))
			.collect();

		// runs of characters with the same style
		let mut runs: Vec<(u64, String)> = Vec::new();
		for (i, ch) in chars.iter().enumerate() {
			let id = ids.get(i).copied().unwrap_or(0);
			match runs.last_mut() {
				Some((last, s)) if *last == id => s.push(*ch),
				_ => runs.push((id, ch.to_string())),
			}
		}

		let mut out = String::new();
		for (id, text) in runs {
			let html = escape(&text).replace(['\u{2028}', '\n'], "<br>");
			let over = if id == 0 { None } else { table.get(&id) };
			match over {
				Some(o) => {
					let mut p = o.as_object().cloned().unwrap_or_default();
					p.remove("styleID");
					let span = RNode {
						guid: n.guid,
						ty: "TEXT".into(),
						name: String::new(),
						p,
						children: vec![],
						instance: None,
					};
					let mut decls = self.font_decls(&span);
					if let Some(fill) = svg::paints(&span, "fillPaints").last().cloned()
						&& let Some(c) = self.paint_css(&fill, &span, false)
					{
						push(&mut decls, "color", c);
					}
					let link = span
						.get("hyperlink")
						.and_then(|h| h.get("url"))
						.and_then(Value::as_str)
						.map(str::to_string);
					let style: Vec<String> =
						decls.iter().map(|(k, v)| format!("{k}: {v}")).collect();
					let style = escape(&style.join("; "));
					match link {
						Some(url) => {
							let _ = write!(
								out,
								"<a href=\"{}\" style=\"{style}\">{html}</a>",
								escape(&url)
							);
						}
						None if style.is_empty() => out.push_str(&html),
						None => {
							let _ = write!(out, "<span style=\"{style}\">{html}</span>");
						}
					}
				}
				None => out.push_str(&html),
			}
		}
		out
	}

	// --- assets

	fn asset(&mut self, name: &str, ext: &str, data: &[u8]) -> Result<String> {
		let hash = short_hash(data);
		if let Some(url) = self.written_assets.get(&hash) {
			return Ok(url.clone());
		}
		let name = if name.is_empty() { "asset" } else { name };
		let mut file = format!("{name}.{ext}");
		if self
			.written_assets
			.values()
			.any(|u| u.ends_with(&format!("/{file}")))
		{
			file = format!("{name}-{}.{ext}", &hash[..6]);
		}
		std::fs::create_dir_all(&self.assets_dir)?;
		std::fs::write(self.assets_dir.join(&file), data)?;
		let url = format!("{}/{file}", self.assets_url);
		self.written_assets.insert(hash, url.clone());
		Ok(url)
	}

	fn video_asset(&mut self, paint: &Value) -> Result<String> {
		let hash = hex(paint.pointer("/video/hash"));
		let Some(data) = self.videos.get(&hash) else {
			return Ok(format!("{}/missing-{hash}", self.assets_url));
		};
		let name = paint
			.pointer("/image/name")
			.and_then(Value::as_str)
			.map(kebab)
			.unwrap_or_else(|| hash.clone());
		let data = data.clone();
		self.asset(&name, "mp4", &data)
	}

	fn image_asset(&mut self, paint: &Value) -> Result<String> {
		let hash = hex(paint.pointer("/image/hash"));
		let name = paint
			.pointer("/image/name")
			.and_then(Value::as_str)
			.map(kebab)
			.unwrap_or_default();
		let Some(data) = self.images.get(&hash) else {
			return Ok(format!("{}/missing-{hash}", self.assets_url));
		};
		let ext = sniff(data);
		let data = data.clone();
		self.asset(if name.is_empty() { &hash } else { &name }, ext, &data)
	}
}

struct Change {
	node: *const RNode,
	decls: Decls,
	/// the layer does not exist in the other version
	gone: bool,
}

struct Media {
	query: String,
	label: String,
	rules: Vec<(String, Decls)>,
	only: Vec<String>,
}

/// Paths of layers in `other` that have no match in `frame`.
fn collect_new(frame: &RNode, other: &RNode, path: &str, out: &mut Vec<String>) {
	let mut seen: HashMap<&str, usize> = HashMap::new();
	for c in &other.children {
		let nth = seen.entry(c.name.as_str()).or_default();
		let i = *nth;
		*nth += 1;
		let p = if path.is_empty() {
			c.name.clone()
		} else {
			format!("{path} > {}", c.name)
		};
		match frame.children.iter().filter(|f| f.name == c.name).nth(i) {
			Some(f) => collect_new(f, c, &p, out),
			None => out.push(p),
		}
	}
}

enum Kind {
	Graphic,
	Image(Value),
	Video(Value),
	Text,
	Box,
}

/// (event, action) of the prototype interactions of a node.
fn interactions(n: &RNode) -> impl Iterator<Item = (&str, &Value)> {
	n.get("prototypeInteractions")
		.and_then(Value::as_array)
		.into_iter()
		.flatten()
		.filter(|i| i.get("isDeleted").and_then(Value::as_bool) != Some(true))
		.flat_map(|i| {
			let event = i
				.pointer("/event/interactionType")
				.and_then(Value::as_str)
				.unwrap_or("");
			i.get("actions")
				.and_then(Value::as_array)
				.into_iter()
				.flatten()
				.map(move |a| (event, a))
		})
}

/// CSS transition for a prototype action.
fn transition(action: &Value) -> Option<String> {
	let ty = action.get("transitionType").and_then(Value::as_str)?;
	if ty == "INSTANT_TRANSITION" {
		return None;
	}
	let secs = action.get("transitionDuration").and_then(Value::as_f64)?;
	let easing = match action.get("easingType").and_then(Value::as_str) {
		Some("LINEAR") => "linear".to_string(),
		Some("EASE_IN") => "ease-in".to_string(),
		Some("EASE_OUT") => "ease-out".to_string(),
		Some("EASE_IN_AND_OUT") => "ease-in-out".to_string(),
		Some("IN_CUBIC") => "cubic-bezier(0.32, 0, 0.67, 0)".to_string(),
		Some("OUT_CUBIC") => "cubic-bezier(0.33, 1, 0.68, 1)".to_string(),
		Some("INOUT_CUBIC") => "cubic-bezier(0.65, 0, 0.35, 1)".to_string(),
		Some("EASE_IN_BACK") => "cubic-bezier(0.36, 0, 0.66, -0.56)".to_string(),
		Some("EASE_OUT_BACK") => "cubic-bezier(0.34, 1.56, 0.64, 1)".to_string(),
		Some("CUSTOM_CUBIC_BEZIER") => {
			let f: Vec<String> = action
				.get("easingFunction")
				.and_then(Value::as_array)
				.into_iter()
				.flatten()
				.filter_map(Value::as_f64)
				.map(num)
				.collect();
			if f.len() == 4 {
				format!("cubic-bezier({})", f.join(", "))
			} else {
				"ease".to_string()
			}
		}
		// springs have no css equivalent
		Some(other) => format!("ease-out /* Figma {} */", other.to_ascii_lowercase()),
		None => "ease".to_string(),
	};
	Some(format!("all {}s {easing}", num(secs)))
}

fn push_blend(n: &RNode, d: &mut Decls) {
	if let Some(mode) = n.s("blendMode")
		&& !matches!(mode, "NORMAL" | "PASS_THROUGH")
	{
		push(
			d,
			"mix-blend-mode",
			mode.to_ascii_lowercase().replace('_', "-"),
		);
	}
}

fn push_opacity(n: &RNode, d: &mut Decls) {
	if let Some(o) = n.f("opacity")
		&& o < 0.999
	{
		push(d, "opacity", num(o));
	}
}

/// Whether a node takes part in its parent's auto layout.
fn in_flow(n: &RNode, parent: &RNode) -> bool {
	matches!(parent.s("stackMode"), Some("HORIZONTAL" | "VERTICAL"))
		&& n.s("stackPositioning") != Some("ABSOLUTE")
}

/// Whether a child sticks out of the node's bounds.
fn overflows(n: &RNode) -> bool {
	let (w, h) = n.size();
	n.children.iter().any(|c| {
		let t = c.transform();
		let (cw, ch) = c.size();
		t[2] < -0.5 || t[5] < -0.5 || t[2] + cw > w + 0.5 || t[5] + ch > h + 0.5
	})
}

fn shorthand(p: &[String; 4]) -> String {
	let [t, r, b, l] = p;
	if t == r && r == b && b == l {
		t.clone()
	} else if t == b && r == l {
		format!("{t} {r}")
	} else if r == l {
		format!("{t} {r} {b}")
	} else {
		format!("{t} {r} {b} {l}")
	}
}

fn gradient_angle(p: &Value, n: &RNode) -> f64 {
	let t = p.get("transform");
	let g = |k: &str, d: f64| {
		t.and_then(|t| t.get(k))
			.and_then(Value::as_f64)
			.unwrap_or(d)
	};
	let (a, b, c, d, e, f) = (
		g("m00", 1.0),
		g("m01", 0.0),
		g("m02", 0.0),
		g("m10", 0.0),
		g("m11", 1.0),
		g("m12", 0.0),
	);
	// the transform maps node space (0..1) to gradient space; invert it to
	// get the gradient line in node space
	let det = a * e - b * d;
	if det.abs() < 1e-9 {
		return 180.0;
	}
	let inv = |x: f64, y: f64| {
		let x = x - c;
		let y = y - f;
		((e * x - b * y) / det, (-d * x + a * y) / det)
	};
	let (x0, y0) = inv(0.0, 0.5);
	let (x1, y1) = inv(1.0, 0.5);
	let (w, h) = n.size();
	let dx = (x1 - x0) * w;
	let dy = (y1 - y0) * h;
	let angle = dx.atan2(-dy).to_degrees();
	(angle + 360.0) % 360.0
}

fn heading_tag(style: &str) -> Option<&'static str> {
	let s = style.trim().to_ascii_lowercase();
	let first = s.split([' ', '/', '-']).next().unwrap_or("");
	Some(match first {
		"xl" | "h1" => "h1",
		"h2" => "h2",
		"h3" => "h3",
		"h4" => "h4",
		"h5" => "h5",
		"h6" => "h6",
		_ => return None,
	})
}

fn font_weight(style: &str) -> (u32, bool) {
	let s = style.to_ascii_lowercase().replace([' ', '-'], "");
	let italic = s.contains("italic") || s.contains("oblique");
	let w = if s.contains("thin") || s.contains("hairline") {
		100
	} else if s.contains("extralight") || s.contains("ultralight") {
		200
	} else if s.contains("light") {
		300
	} else if s.contains("semibold") || s.contains("demibold") {
		600
	} else if s.contains("extrabold") || s.contains("ultrabold") {
		800
	} else if s.contains("bold") {
		700
	} else if s.contains("medium") {
		500
	} else if s.contains("black") || s.contains("heavy") {
		900
	} else {
		400
	};
	(w, italic)
}

fn short(s: &str, max: usize) -> String {
	let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
	if s.chars().count() > max {
		format!("{}…", s.chars().take(max).collect::<String>())
	} else {
		s
	}
}

fn hex(v: Option<&Value>) -> String {
	v.and_then(Value::as_array)
		.into_iter()
		.flatten()
		.map(|b| format!("{:02x}", b.as_u64().unwrap_or(0)))
		.collect()
}

fn short_hash(data: &[u8]) -> String {
	// FNV-1a, enough to dedupe assets
	let mut h: u64 = 0xcbf29ce484222325;
	for b in data {
		h ^= *b as u64;
		h = h.wrapping_mul(0x100000001b3);
	}
	format!("{h:016x}")
}

fn sniff(data: &[u8]) -> &'static str {
	if data.starts_with(&[0x89, b'P', b'N', b'G']) {
		"png"
	} else if data.starts_with(&[0xff, 0xd8]) {
		"jpg"
	} else if data.starts_with(b"GIF8") {
		"gif"
	} else if data.len() > 12 && &data[8..12] == b"WEBP" {
		"webp"
	} else if data.starts_with(b"<svg") || data.starts_with(b"<?xml") {
		"svg"
	} else {
		"bin"
	}
}
