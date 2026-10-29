//! Document index and instance expansion.
//!
//! Figma stores instances without children: they reference a component
//! (`SYMBOL`) and carry overrides keyed by guid paths, the layout Figma
//! derived for them and component property values. `Doc::resolve` turns a
//! node into a plain tree where every instance is expanded.

use std::collections::HashMap;
use std::rc::Rc;

use serde_json::{Map, Value};

pub type Obj = Map<String, Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Guid(pub u32, pub u32);

impl Guid {
	pub fn from_value(v: &Value) -> Option<Self> {
		let v = v.get("guid").unwrap_or(v);
		Some(Self(
			v.get("sessionID")?.as_u64()? as u32,
			v.get("localID")?.as_u64()? as u32,
		))
	}
}

impl std::fmt::Display for Guid {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "{}:{}", self.0, self.1)
	}
}

impl std::str::FromStr for Guid {
	type Err = ();

	fn from_str(s: &str) -> Result<Self, ()> {
		let (a, b) = s.split_once(':').ok_or(())?;
		Ok(Self(a.parse().map_err(|_| ())?, b.parse().map_err(|_| ())?))
	}
}

pub struct Doc {
	pub nodes: HashMap<Guid, Obj>,
	pub children: HashMap<Guid, Vec<Guid>>,
	pub parent: HashMap<Guid, Guid>,
	pub blobs: Vec<Vec<u8>>,
	pub root: Guid,
}

/// An instance that was expanded, with what a developer needs to know.
#[derive(Debug, Clone)]
pub struct InstanceInfo {
	pub component: String,
	pub variant: Option<String>,
	pub props: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct RNode {
	pub guid: Guid,
	pub ty: String,
	pub name: String,
	pub p: Obj,
	pub children: Vec<RNode>,
	pub instance: Option<InstanceInfo>,
}

impl RNode {
	pub fn get(&self, key: &str) -> Option<&Value> {
		self.p.get(key)
	}

	pub fn f(&self, key: &str) -> Option<f64> {
		self.p.get(key).and_then(Value::as_f64)
	}

	pub fn s(&self, key: &str) -> Option<&str> {
		self.p.get(key).and_then(Value::as_str)
	}

	pub fn size(&self) -> (f64, f64) {
		let s = self.p.get("size");
		(
			s.and_then(|s| s.get("x"))
				.and_then(Value::as_f64)
				.unwrap_or(0.0),
			s.and_then(|s| s.get("y"))
				.and_then(Value::as_f64)
				.unwrap_or(0.0),
		)
	}

	/// The 2x3 transform relative to the parent: (m00, m01, m02, m10, m11, m12)
	pub fn transform(&self) -> [f64; 6] {
		let t = self.p.get("transform");
		let g = |k: &str, d: f64| {
			t.and_then(|t| t.get(k))
				.and_then(Value::as_f64)
				.unwrap_or(d)
		};
		[
			g("m00", 1.0),
			g("m01", 0.0),
			g("m02", 0.0),
			g("m10", 0.0),
			g("m11", 1.0),
			g("m12", 0.0),
		]
	}
}

#[derive(Clone, Default)]
struct Ctx {
	/// override keys from the outermost instance, None outside instances
	path: Option<Vec<Guid>>,
	overrides: Rc<HashMap<Vec<Guid>, Obj>>,
	derived: Rc<HashMap<Vec<Guid>, Obj>>,
	/// component property values of the innermost instance by def id
	props: Rc<HashMap<Guid, Value>>,
	depth: usize,
}

const DERIVED_KEYS: &[&str] = &[
	"size",
	"transform",
	"fillGeometry",
	"strokeGeometry",
	"derivedTextData",
];

impl Doc {
	pub fn new(message: &Value) -> Self {
		let mut nodes = HashMap::new();
		let mut children: HashMap<Guid, Vec<(String, Guid)>> = HashMap::new();
		let mut parent = HashMap::new();
		let mut root = Guid(0, 0);

		for n in message["nodeChanges"].as_array().into_iter().flatten() {
			let Some(obj) = n.as_object() else { continue };
			let Some(guid) = obj.get("guid").and_then(Guid::from_value) else {
				continue;
			};
			if obj.get("type").and_then(Value::as_str) == Some("DOCUMENT") {
				root = guid;
			}
			if let Some(pi) = obj.get("parentIndex")
				&& let Some(p) = pi.get("guid").and_then(Guid::from_value)
			{
				let pos = pi["position"].as_str().unwrap_or("").to_string();
				children.entry(p).or_default().push((pos, guid));
				parent.insert(guid, p);
			}
			nodes.insert(guid, obj.clone());
		}

		let children = children
			.into_iter()
			.map(|(k, mut v)| {
				v.sort();
				(k, v.into_iter().map(|(_, g)| g).collect())
			})
			.collect();

		let blobs = message["blobs"]
			.as_array()
			.into_iter()
			.flatten()
			.map(|b| {
				b["bytes"]
					.as_array()
					.into_iter()
					.flatten()
					.map(|v| v.as_u64().unwrap_or(0) as u8)
					.collect()
			})
			.collect();

		Self {
			nodes,
			children,
			parent,
			blobs,
			root,
		}
	}

	pub fn node(&self, g: Guid) -> Option<&Obj> {
		self.nodes.get(&g)
	}

	pub fn name(&self, g: Guid) -> &str {
		self.nodes
			.get(&g)
			.and_then(|n| n.get("name"))
			.and_then(Value::as_str)
			.unwrap_or("")
	}

	pub fn ty(&self, g: Guid) -> &str {
		self.nodes
			.get(&g)
			.and_then(|n| n.get("type"))
			.and_then(Value::as_str)
			.unwrap_or("")
	}

	pub fn kids(&self, g: Guid) -> &[Guid] {
		self.children.get(&g).map(Vec::as_slice).unwrap_or(&[])
	}

	/// The page (canvas) a node is on.
	pub fn page_of(&self, mut g: Guid) -> Option<Guid> {
		loop {
			if self.ty(g) == "CANVAS" {
				return Some(g);
			}
			g = *self.parent.get(&g)?;
		}
	}

	fn key(&self, g: Guid) -> Guid {
		self.nodes
			.get(&g)
			.and_then(|n| n.get("overrideKey"))
			.and_then(Guid::from_value)
			.unwrap_or(g)
	}

	/// Resolve a node into a tree with all instances expanded. Invisible
	/// nodes are dropped.
	pub fn resolve(&self, g: Guid) -> Option<RNode> {
		self.build(g, &Ctx::default())
	}

	fn build(&self, g: Guid, ctx: &Ctx) -> Option<RNode> {
		let mut p = self.nodes.get(&g)?.clone();

		let mut full_path = None;
		if let Some(path) = &ctx.path {
			let mut full = path.clone();
			full.push(self.key(g));
			if let Some(o) = ctx.overrides.get(&full) {
				for (k, v) in o {
					if k != "guidPath" {
						p.insert(k.clone(), v.clone());
					}
				}
			}
			if let Some(d) = ctx.derived.get(&full) {
				for k in DERIVED_KEYS {
					if let Some(v) = d.get(*k) {
						p.insert(k.to_string(), v.clone());
					}
				}
			}
			full_path = Some(full);
		}

		self.apply_prop_refs(&mut p, &ctx.props);

		if p.get("visible").and_then(Value::as_bool) == Some(false) {
			return None;
		}

		let ty = p
			.get("type")
			.and_then(Value::as_str)
			.unwrap_or("")
			.to_string();
		let name = p
			.get("name")
			.and_then(Value::as_str)
			.unwrap_or("")
			.to_string();

		if ty == "INSTANCE" && ctx.depth < 32 {
			return Some(self.build_instance(g, p, ty, name, ctx, full_path));
		}

		let children = self
			.kids(g)
			.iter()
			.filter_map(|c| self.build(*c, ctx))
			.collect();

		Some(RNode {
			guid: g,
			ty,
			name,
			p,
			children,
			instance: None,
		})
	}

	fn build_instance(
		&self,
		g: Guid,
		p: Obj,
		ty: String,
		name: String,
		ctx: &Ctx,
		full_path: Option<Vec<Guid>>,
	) -> RNode {
		let sym = p
			.get("overriddenSymbolID")
			.and_then(Guid::from_value)
			.or_else(|| {
				p.get("symbolData")?
					.get("symbolID")
					.and_then(Guid::from_value)
			});

		let prefix = full_path.unwrap_or_default();

		// overrides addressed at the component itself apply to the instance
		let mut p = p;
		if let Some(sym) = sym {
			let own = p
				.get("symbolData")
				.and_then(|s| s.get("symbolOverrides"))
				.and_then(Value::as_array)
				.cloned()
				.unwrap_or_default();
			for o in own {
				let path: Vec<Guid> = o
					.pointer("/guidPath/guids")
					.and_then(Value::as_array)
					.into_iter()
					.flatten()
					.filter_map(Guid::from_value)
					.collect();
				if path == [sym] || path == [self.key(sym)] {
					for (k, v) in o.as_object().into_iter().flatten() {
						if k != "guidPath" {
							p.insert(k.clone(), v.clone());
						}
					}
				}
			}
		}

		// outer overrides win over the ones of this instance
		let mut overrides = (*ctx.overrides).clone();
		let mut derived = (*ctx.derived).clone();
		let own = p
			.get("symbolData")
			.and_then(|s| s.get("symbolOverrides"))
			.and_then(Value::as_array);
		merge_paths(&mut overrides, &prefix, own.into_iter().flatten());
		let own = p.get("derivedSymbolData").and_then(Value::as_array);
		merge_paths(&mut derived, &prefix, own.into_iter().flatten());

		let (props, info) = match sym {
			Some(sym) => self.instance_props(sym, &p, &ctx.props),
			None => (HashMap::new(), None),
		};

		let inner = Ctx {
			path: Some(prefix),
			overrides: Rc::new(overrides),
			derived: Rc::new(derived),
			props: Rc::new(props),
			depth: ctx.depth + 1,
		};

		let children = sym
			.map(|s| {
				self.kids(s)
					.iter()
					.filter_map(|c| self.build(*c, &inner))
					.collect()
			})
			.unwrap_or_default();

		RNode {
			guid: g,
			ty,
			name,
			p,
			children,
			instance: info,
		}
	}

	/// Collect the property values an instance gives its component and a
	/// description of the instance.
	fn instance_props(
		&self,
		sym: Guid,
		inst: &Obj,
		outer: &HashMap<Guid, Value>,
	) -> (HashMap<Guid, Value>, Option<InstanceInfo>) {
		let sym_node = self.nodes.get(&sym);
		let set = self
			.parent
			.get(&sym)
			.filter(|p| {
				self.nodes
					.get(p)
					.and_then(|n| n.get("isStateGroup"))
					.and_then(Value::as_bool)
					.unwrap_or(false)
			})
			.copied();

		// definitions with names and defaults, by id
		let mut defs: HashMap<Guid, &Value> = HashMap::new();
		for owner in [Some(sym), set].into_iter().flatten() {
			let list = self
				.nodes
				.get(&owner)
				.and_then(|n| n.get("componentPropDefs"))
				.and_then(Value::as_array);
			for d in list.into_iter().flatten() {
				if let Some(id) = d.get("id").and_then(Guid::from_value) {
					defs.insert(id, d);
				}
			}
		}

		// symbol level def id -> set level def id
		let mut parent_of: HashMap<Guid, Guid> = HashMap::new();
		for (id, d) in &defs {
			if let Some(pid) = d.get("parentPropDefId").and_then(Guid::from_value) {
				parent_of.insert(*id, pid);
			}
		}

		let mut values: HashMap<Guid, Value> = HashMap::new();
		// defaults
		for (id, d) in &defs {
			if let Some(v) = d.get("varValue") {
				values.insert(*id, v.clone());
			}
		}
		for (child, par) in &parent_of {
			if let Some(v) = values.get(par).cloned() {
				values.entry(*child).or_insert(v);
			}
		}

		let assigned = inst
			.get("componentPropAssignments")
			.and_then(Value::as_array);
		let mut set_values: Vec<(Guid, Value)> = Vec::new();
		for a in assigned.into_iter().flatten() {
			let Some(id) = a.get("defID").and_then(Guid::from_value) else {
				continue;
			};
			let Some(mut v) = a.get("varValue").cloned() else {
				continue;
			};
			// a value can pass on a property of the outer component
			if v.get("dataType").and_then(Value::as_str) == Some("PROP_REF") {
				let r = v
					.pointer("/value/propRefValue/defId")
					.and_then(Guid::from_value);
				match r.and_then(|r| outer.get(&r)) {
					Some(o) => v = o.clone(),
					None => continue,
				}
			}
			set_values.push((id, v));
		}
		for (id, v) in set_values {
			values.insert(id, v.clone());
			if let Some(par) = parent_of.get(&id) {
				values.insert(*par, v.clone());
			}
			for (child, par) in &parent_of {
				if *par == id {
					values.insert(*child, v.clone());
				}
			}
		}

		// description for the html output
		let (component, variant) = match set {
			Some(set) => (
				self.name(set).to_string(),
				sym_node
					.and_then(|n| n.get("name"))
					.and_then(Value::as_str)
					.map(str::to_string),
			),
			None => (self.name(sym).to_string(), None),
		};
		let mut props = Vec::new();
		let mut named: Vec<(&str, Guid, &Value)> = defs
			.iter()
			.filter(|(_, d)| d.get("name").is_some())
			.map(|(id, d)| {
				let sort = d.get("sortPosition").and_then(Value::as_str).unwrap_or("");
				(sort, *id, *d)
			})
			.collect();
		named.sort_by(|a, b| a.0.cmp(b.0));
		for (_, id, d) in named {
			let name = d["name"].as_str().unwrap_or("");
			let ty = d.get("type").and_then(Value::as_str).unwrap_or("");
			// variants are already in the variant name
			if ty == "VARIANT" {
				continue;
			}
			if let Some(v) = values.get(&id)
				&& let Some(s) = self.prop_value_string(v)
			{
				props.push((name.to_string(), s));
			}
		}

		(
			values,
			Some(InstanceInfo {
				component,
				variant,
				props,
			}),
		)
	}

	fn prop_value_string(&self, v: &Value) -> Option<String> {
		let val = v.get("value")?;
		if let Some(t) = val.pointer("/textDataValue/characters") {
			return t.as_str().map(|s| s.trim().to_string());
		}
		if let Some(b) = val.get("boolValue") {
			return Some(b.as_bool().unwrap_or(false).to_string());
		}
		if let Some(g) = val.get("symbolIdValue").and_then(Guid::from_value) {
			return Some(self.name(g).to_string());
		}
		if let Some(s) = val.get("textValue") {
			return s.as_str().map(str::to_string);
		}
		None
	}

	/// Apply component property references (text, visibility, instance
	/// swap) of a node inside a component.
	fn apply_prop_refs(&self, p: &mut Obj, props: &HashMap<Guid, Value>) {
		if props.is_empty() {
			return;
		}
		let mut refs: Vec<(String, Guid)> = Vec::new();
		let entries = p
			.get("parameterConsumptionMap")
			.and_then(|m| m.get("entries"))
			.and_then(Value::as_array);
		for e in entries.into_iter().flatten() {
			let data = &e["variableData"];
			if data.get("dataType").and_then(Value::as_str) != Some("PROP_REF") {
				continue;
			}
			let (Some(field), Some(id)) = (
				e.get("variableField").and_then(Value::as_str),
				data.pointer("/value/propRefValue/defId")
					.and_then(Guid::from_value),
			) else {
				continue;
			};
			refs.push((field.to_string(), id));
		}
		let old = p.get("componentPropRefs").and_then(Value::as_array);
		for r in old.into_iter().flatten() {
			if let (Some(field), Some(id)) = (
				r.get("componentPropNodeField").and_then(Value::as_str),
				r.get("defID").and_then(Guid::from_value),
			) {
				refs.push((field.to_string(), id));
			}
		}

		for (field, id) in refs {
			let Some(v) = props.get(&id) else { continue };
			let val = &v["value"];
			match field.as_str() {
				"TEXT_DATA" => {
					if let Some(t) = val.get("textDataValue") {
						p.insert("textData".into(), t.clone());
					}
				}
				"VISIBLE" => {
					if let Some(b) = val.get("boolValue") {
						p.insert("visible".into(), b.clone());
					}
				}
				"OVERRIDDEN_SYMBOL_ID" => {
					if let Some(s) = val.get("symbolIdValue") {
						p.insert("overriddenSymbolID".into(), s.clone());
					}
				}
				_ => {}
			}
		}
	}
}

fn merge_paths<'a>(
	map: &mut HashMap<Vec<Guid>, Obj>,
	prefix: &[Guid],
	entries: impl Iterator<Item = &'a Value>,
) {
	for e in entries {
		let Some(obj) = e.as_object() else { continue };
		let guids: Vec<Guid> = obj
			.get("guidPath")
			.and_then(|g| g.get("guids"))
			.and_then(Value::as_array)
			.into_iter()
			.flatten()
			.filter_map(Guid::from_value)
			.collect();
		if guids.is_empty() {
			continue;
		}
		let mut path = prefix.to_vec();
		path.extend(guids);
		let slot = map.entry(path).or_default();
		for (k, v) in obj {
			slot.entry(k.clone()).or_insert_with(|| v.clone());
		}
	}
}
