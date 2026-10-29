//! Maps Figma tokens to the variables and mixins an existing project
//! already has, so an agent reuses them instead of inventing new ones.

use std::fmt::Write;
use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use crate::css::{color, kebab};
use crate::vars::Vars;

pub struct Project {
	/// custom properties with their first (base) value
	vars: Vec<(String, String)>,
	mixins: Vec<String>,
}

impl Project {
	pub fn load(dir: &Path) -> Result<Self> {
		let mut p = Self {
			vars: Vec::new(),
			mixins: Vec::new(),
		};
		p.walk(dir)?;
		Ok(p)
	}

	fn walk(&mut self, dir: &Path) -> Result<()> {
		for e in std::fs::read_dir(dir)? {
			let path = e?.path();
			let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
			if path.is_dir() {
				if !name.starts_with('.')
					&& !matches!(
						name,
						"node_modules" | "dist" | "build" | "vendor" | "cache"
					) {
					self.walk(&path)?;
				}
				continue;
			}
			if !matches!(
				path.extension().and_then(|e| e.to_str()),
				Some("scss" | "css" | "sass")
			) {
				continue;
			}
			let text = std::fs::read_to_string(&path).unwrap_or_default();
			self.parse(&text);
		}
		Ok(())
	}

	fn parse(&mut self, text: &str) {
		for line in text.lines() {
			let line = line.trim();
			if let Some(rest) = line.strip_prefix("@mixin ") {
				let name: String = rest
					.chars()
					.take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
					.collect();
				if !self.mixins.contains(&name) {
					self.mixins.push(name);
				}
				continue;
			}
			let Some(rest) = line.strip_prefix("--") else {
				continue;
			};
			let Some((name, value)) = rest.split_once(':') else {
				continue;
			};
			let value = value.trim().trim_end_matches(';').trim();
			if name.contains(' ') || value.is_empty() {
				continue;
			}
			if !self.vars.iter().any(|(n, _)| n == name) {
				self.vars.push((name.to_string(), value.to_string()));
			}
		}
	}

	fn base_px(&self, name: &str) -> Option<f64> {
		let v = &self.vars.iter().find(|(n, _)| n == name)?.1;
		if let Some(r) = v.strip_suffix("rem") {
			return r.trim().parse::<f64>().ok().map(|f| f * 16.0);
		}
		v.strip_suffix("px")?.trim().parse().ok()
	}

	fn color(&self, name: &str) -> Option<String> {
		normalize_color(&self.vars.iter().find(|(n, _)| n == name)?.1)
	}
}

fn normalize_color(v: &str) -> Option<String> {
	let v = v.trim().to_ascii_lowercase();
	let hex = v.strip_prefix('#')?;
	match hex.len() {
		3 => Some(format!(
			"#{}",
			hex.chars().flat_map(|c| [c, c]).collect::<String>()
		)),
		6 => Some(format!("#{hex}")),
		_ => None,
	}
}

fn words(s: &str) -> Vec<String> {
	kebab(s).split('-').map(str::to_string).collect()
}

/// A comment block for tokens.css: which Figma token is which project
/// variable or mixin.
pub fn mapping(
	vars: &Vars,
	text_styles: &[(String, String)],
	project: &Project,
) -> String {
	let mut lines = Vec::new();
	let mut missing = Vec::new();

	let mut ids = vars.used();
	ids.sort_by_key(|id| vars.vars[id].css.clone());
	for id in ids {
		let v = &vars.vars[&id];
		let figma_words = words(&v.name);
		let values = vars.mode_values(id);

		let found = if project.vars.iter().any(|(n, _)| *n == v.css) {
			Some((v.css.clone(), "same name"))
		} else if let Some(c) = values.first().and_then(|x| x.get("colorValue")) {
			// colors by value, the name with most words in common wins
			let want = color(c, 1.0);
			project
				.vars
				.iter()
				.filter(|(n, _)| project.color(n).as_deref() == Some(want.as_str()))
				.max_by_key(|(n, _)| {
					words(n).iter().filter(|w| figma_words.contains(w)).count()
				})
				.map(|(n, _)| (n.clone(), "same color"))
		} else if let Some(t) = values
			.first()
			.and_then(|x| x.get("textValue"))
			.and_then(Value::as_str)
		{
			// font families by the font name in the value
			project
				.vars
				.iter()
				.find(|(_, val)| {
					val.split(',').next().is_some_and(|f| {
						f.trim().trim_matches(['\'', '"']).eq_ignore_ascii_case(t)
					})
				})
				.map(|(n, _)| (n.clone(), "same font"))
		} else if figma_words
			.iter()
			.any(|w| w.contains("lineheight") || w == "leading")
		{
			// a line height named like a font size would match by mistake
			None
		} else {
			// lengths: same last name segment and a value of one of the modes
			let last = kebab(v.name.rsplit('/').next().unwrap_or(""));
			let px: Vec<f64> = values
				.iter()
				.filter_map(|x| x.get("floatValue").and_then(Value::as_f64))
				.collect();
			project
				.vars
				.iter()
				.find(|(n, _)| {
					*n == last
						&& project
							.base_px(n)
							.is_some_and(|b| px.iter().any(|p| (p - b).abs() < 0.5))
				})
				.map(|(n, _)| (n.clone(), "same name and size"))
		};
		match found {
			Some((p, why)) => lines.push(format!("--{} -> var(--{p}) ({why})", v.css)),
			None => missing.push(format!("--{}", v.css)),
		}
	}

	for (class, style) in text_styles {
		let k = kebab(style);
		let candidates = [k.clone(), format!("h-{k}")];
		match candidates.iter().find(|c| project.mixins.contains(c)) {
			Some(m) => lines.push(format!(".{class} (\"{style}\") -> @include {m}")),
			None => missing.push(format!(".{class} (\"{style}\")")),
		}
	}

	let mut out = String::from(
		"/* Mapping to the project's variables and mixins, by name or value.\n   \
		 Use the project's names in code. */\n/*\n",
	);
	for l in lines {
		let _ = writeln!(out, "{l}");
	}
	if !missing.is_empty() {
		let _ = writeln!(out, "\nNo match in the project:\n{}", missing.join("\n"));
	}
	out.push_str("*/\n\n");
	out
}
