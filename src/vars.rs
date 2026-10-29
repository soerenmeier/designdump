//! Figma variables (design tokens) as CSS custom properties.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;

use serde_json::Value;

use crate::css::{color, kebab, num};
use crate::doc::{Doc, Guid};

pub struct Var {
	pub name: String,
	pub css: String,
	pub set: Guid,
	pub scopes: Vec<String>,
	/// mode id -> variableData
	pub values: Vec<(Guid, Value)>,
}

pub struct VarSet {
	pub name: String,
	/// (mode id, mode name), default first
	pub modes: Vec<(Guid, String)>,
}

pub struct Vars {
	pub vars: HashMap<Guid, Var>,
	pub sets: HashMap<Guid, VarSet>,
	used: RefCell<BTreeSet<Guid>>,
}

impl Vars {
	pub fn new(doc: &Doc) -> Self {
		let mut sets = HashMap::new();
		let mut vars = HashMap::new();
		for (g, n) in &doc.nodes {
			match n.get("type").and_then(Value::as_str) {
				Some("VARIABLE_SET") => {
					let mut modes: Vec<(String, Guid, String)> = n
						.get("variableSetModes")
						.and_then(Value::as_array)
						.into_iter()
						.flatten()
						.filter_map(|m| {
							Some((
								m.get("sortPosition")?.as_str()?.to_string(),
								Guid::from_value(m.get("id")?)?,
								m.get("name")?.as_str()?.to_string(),
							))
						})
						.collect();
					modes.sort();
					sets.insert(
						*g,
						VarSet {
							name: str_of(n.get("name")),
							modes: modes.into_iter().map(|(_, g, n)| (g, n)).collect(),
						},
					);
				}
				Some("VARIABLE") => {
					let values = n
						.get("variableDataValues")
						.and_then(|v| v.get("entries"))
						.and_then(Value::as_array)
						.into_iter()
						.flatten()
						.filter_map(|e| {
							Some((
								Guid::from_value(e.get("modeID")?)?,
								e.get("variableData")?.clone(),
							))
						})
						.collect();
					vars.insert(
						*g,
						Var {
							name: str_of(n.get("name")),
							css: String::new(),
							set: n
								.get("variableSetID")
								.and_then(Guid::from_value)
								.unwrap_or(Guid(0, 0)),
							scopes: n
								.get("variableScopes")
								.and_then(Value::as_array)
								.into_iter()
								.flatten()
								.filter_map(|s| s.as_str().map(str::to_string))
								.collect(),
							values,
						},
					);
				}
				_ => {}
			}
		}

		// css names, prefixed with the collection where names collide
		let mut count: HashMap<String, usize> = HashMap::new();
		for v in vars.values() {
			*count.entry(kebab(&v.name)).or_default() += 1;
		}
		for v in vars.values_mut() {
			let base = kebab(&v.name);
			v.css = if count[&base] > 1 {
				let set = sets.get(&v.set).map(|s| kebab(&s.name)).unwrap_or_default();
				format!("{set}-{base}")
			} else {
				base
			};
		}
		// still colliding (same name in collections with the same name)
		let mut seen: HashMap<String, usize> = HashMap::new();
		let mut ids: Vec<Guid> = vars.keys().copied().collect();
		ids.sort();
		for id in ids {
			let v = vars.get_mut(&id).unwrap();
			let n = seen.entry(v.css.clone()).or_default();
			*n += 1;
			if *n > 1 {
				v.css = format!("{}-{}", v.css, n);
			}
		}

		Self {
			vars,
			sets,
			used: RefCell::new(BTreeSet::new()),
		}
	}

	/// `var(--name)` for a variable, remembering that it is used.
	pub fn reference(&self, id: Guid) -> Option<String> {
		let v = self.vars.get(&id)?;
		self.used.borrow_mut().insert(id);
		Some(format!("var(--{})", v.css))
	}

	/// The variable a `variableData` value aliases to.
	pub fn alias_of(data: &Value) -> Option<Guid> {
		if data.get("dataType").and_then(Value::as_str) != Some("ALIAS") {
			return None;
		}
		data.pointer("/value/alias").and_then(Guid::from_value)
	}

	/// Resolve a variable to its value in the default mode, following
	/// aliases.
	pub fn resolve(&self, mut id: Guid) -> Option<&Value> {
		for _ in 0..16 {
			let v = self.vars.get(&id)?;
			let data = &self.default_value(v)?.1;
			match Self::alias_of(data) {
				Some(next) => id = next,
				None => return data.get("value"),
			}
		}
		None
	}

	/// The value of every mode of a variable, aliases resolved.
	pub fn mode_values(&self, id: Guid) -> Vec<&Value> {
		let Some(v) = self.vars.get(&id) else {
			return Vec::new();
		};
		v.values
			.iter()
			.filter_map(|(_, data)| match Self::alias_of(data) {
				Some(a) => self.resolve(a),
				None => data.get("value"),
			})
			.collect()
	}

	/// Variables referenced so far.
	pub fn used(&self) -> Vec<Guid> {
		self.used.borrow().iter().copied().collect()
	}

	pub fn resolve_string(&self, id: Guid) -> Option<String> {
		self.resolve(id)?
			.get("textValue")
			.and_then(Value::as_str)
			.map(str::to_string)
	}

	fn default_value<'a>(&self, v: &'a Var) -> Option<&'a (Guid, Value)> {
		let default = self.sets.get(&v.set).and_then(|s| s.modes.first());
		default
			.and_then(|(m, _)| v.values.iter().find(|(id, _)| id == m))
			.or_else(|| v.values.first())
	}

	fn value_css(&self, v: &Var, data: &Value) -> Option<String> {
		if let Some(a) = Self::alias_of(data) {
			return self.reference(a);
		}
		let val = data.get("value")?;
		match data.get("dataType").and_then(Value::as_str)? {
			"COLOR" => Some(color(val.get("colorValue")?, 1.0)),
			"FLOAT" => {
				let f = val.get("floatValue")?.as_f64()?;
				let unitless = v.scopes.iter().any(|s| {
					matches!(s.as_str(), "OPACITY" | "FONT_WEIGHT" | "FONT_VARIATIONS")
				});
				Some(if unitless {
					num(f)
				} else {
					format!("{}px", num(f))
				})
			}
			"STRING" => {
				let s = val.get("textValue")?.as_str()?;
				Some(format!("\"{}\"", s.replace('"', "'")))
			}
			"BOOLEAN" => Some(
				if val
					.get("boolValue")
					.and_then(Value::as_bool)
					.unwrap_or(false)
				{
					"1".into()
				} else {
					"0".into()
				},
			),
			_ => None,
		}
	}

	/// CSS with all used variables. `breakpoints` maps a mode to the frame
	/// width it was designed for, which gives a guessed media query.
	pub fn tokens_css(&self, breakpoints: &HashMap<Guid, f64>) -> String {
		// pull in alias targets until nothing changes
		loop {
			let before = self.used.borrow().len();
			let ids: Vec<Guid> = self.used.borrow().iter().copied().collect();
			for id in ids {
				let v = &self.vars[&id];
				for (_, data) in &v.values {
					let _ = self.value_css(v, data);
				}
			}
			if self.used.borrow().len() == before {
				break;
			}
		}

		let used: BTreeSet<Guid> = self.used.borrow().clone();
		let mut by_set: BTreeMap<String, Vec<&Var>> = BTreeMap::new();
		for id in used.iter() {
			let v = &self.vars[id];
			let set = self
				.sets
				.get(&v.set)
				.map(|s| s.name.clone())
				.unwrap_or_default();
			by_set.entry(set).or_default().push(v);
		}

		let mut out = String::new();
		out.push_str(":root {\n");
		for (set, vars) in &mut by_set {
			vars.sort_by(|a, b| a.name.cmp(&b.name));
			let _ = writeln!(out, "\t/* {set} */");
			for v in vars.iter() {
				if let Some((_, data)) = self.default_value(v)
					&& let Some(css) = self.value_css(v, data)
				{
					let _ = writeln!(out, "\t--{}: {};", v.css, css);
				}
			}
		}
		out.push_str("}\n");

		// other modes
		let mut sets: Vec<(&Guid, &VarSet)> = self
			.sets
			.iter()
			.filter(|(_, s)| s.modes.len() > 1)
			.collect();
		sets.sort_by(|a, b| a.1.name.cmp(&b.1.name));
		for (set_id, set) in sets {
			let vars: Vec<&Var> = used
				.iter()
				.map(|id| &self.vars[id])
				.filter(|v| v.set == *set_id)
				.collect();
			if vars.is_empty() {
				continue;
			}
			let default_width = breakpoints.get(&set.modes[0].0).copied();
			let mut prev_width = default_width;
			for (mode, mode_name) in set.modes.iter().skip(1) {
				let mut body = String::new();
				for v in &vars {
					let Some((_, data)) = v.values.iter().find(|(m, _)| m == mode) else {
						continue;
					};
					let default = self.default_value(v).map(|d| &d.1);
					if default == Some(data) {
						continue;
					}
					if let Some(css) = self.value_css(v, data) {
						let _ = writeln!(body, "\t--{}: {};", v.css, css);
					}
				}
				if body.is_empty() {
					continue;
				}
				let attr =
					format!("[data-{}=\"{}\"]", kebab(&set.name), kebab(mode_name));
				let width = breakpoints.get(mode).copied();
				match (width, prev_width) {
					(Some(w), Some(p)) => {
						let bp = ((w + p) / 2.0).floor();
						let _ = writeln!(
							out,
							"\n/* {} mode \"{}\", designed at {}px; breakpoint guessed */\n\
							 @media (max-width: {}px) {{\n:root {{\n{}}}\n}}\n{} {{\n{}}}",
							set.name,
							mode_name,
							num(w),
							num(bp),
							body,
							attr,
							body
						);
						prev_width = Some(w);
					}
					_ => {
						let _ = writeln!(
							out,
							"\n/* {} mode \"{}\" */\n{} {{\n{}}}",
							set.name, mode_name, attr, body
						);
					}
				}
			}
			// the default mode can be selected explicitly as well
			let _ = default_width;
		}
		out
	}

	/// Data attribute selecting a mode, for frames that pick one.
	pub fn mode_attr(&self, set: Guid, mode: Guid) -> Option<(String, String)> {
		let s = self.sets.get(&set)?;
		let (_, name) = s.modes.iter().find(|(m, _)| *m == mode)?;
		Some((format!("data-{}", kebab(&s.name)), kebab(name)))
	}
}

fn str_of(v: Option<&Value>) -> String {
	v.and_then(Value::as_str).unwrap_or("").to_string()
}
