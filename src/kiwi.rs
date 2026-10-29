//! Generic decoder for the Kiwi binary format used by Figma's `.fig` files.
//!
//! Every `.fig` file carries its own schema, so we decode the schema first and
//! then decode the data into a dynamic JSON value. That keeps us independent of
//! the exact Figma schema version.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Number, Value};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
	Enum,
	Struct,
	Message,
}

#[derive(Debug, Clone)]
pub struct Field {
	pub name: String,
	/// negative = builtin type, otherwise index into definitions
	pub type_id: i32,
	pub is_array: bool,
	pub value: u32,
}

#[derive(Debug, Clone)]
pub struct Definition {
	pub name: String,
	pub kind: Kind,
	pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Schema {
	pub defs: Vec<Definition>,
}

impl Schema {
	pub fn parse(data: &[u8]) -> Result<Self> {
		let mut r = Reader::new(data);
		let count = r.var_uint()?;
		let mut defs = Vec::with_capacity(count as usize);
		for _ in 0..count {
			let name = r.string()?;
			let kind = match r.byte()? {
				0 => Kind::Enum,
				1 => Kind::Struct,
				2 => Kind::Message,
				k => bail!("unknown definition kind {k}"),
			};
			let field_count = r.var_uint()?;
			let mut fields = Vec::with_capacity(field_count as usize);
			for _ in 0..field_count {
				let name = r.string()?;
				let type_id = r.var_int()?;
				let is_array = r.byte()? != 0;
				let value = r.var_uint()?;
				fields.push(Field {
					name,
					type_id,
					is_array,
					value,
				});
			}
			defs.push(Definition { name, kind, fields });
		}
		Ok(Self { defs })
	}

	pub fn find(&self, name: &str) -> Option<usize> {
		self.defs.iter().position(|d| d.name == name)
	}

	pub fn decode(&self, root: &str, data: &[u8]) -> Result<Value> {
		let idx = self.find(root).context("root type not in schema")?;
		let mut r = Reader::new(data);
		self.decode_def(idx, &mut r)
	}

	fn decode_def(&self, idx: usize, r: &mut Reader) -> Result<Value> {
		let def = &self.defs[idx];
		match def.kind {
			Kind::Enum => {
				let v = r.var_uint()?;
				let name = def
					.fields
					.iter()
					.find(|f| f.value == v)
					.map(|f| f.name.clone())
					.unwrap_or_else(|| v.to_string());
				Ok(Value::String(name))
			}
			Kind::Struct => {
				let mut map = Map::new();
				for f in &def.fields {
					map.insert(f.name.clone(), self.decode_field(f, r)?);
				}
				Ok(Value::Object(map))
			}
			Kind::Message => {
				let mut map = Map::new();
				loop {
					let id = r.var_uint()?;
					if id == 0 {
						break;
					}
					let f =
						def.fields.iter().find(|f| f.value == id).with_context(|| {
							format!("unknown field {id} in {}", def.name)
						})?;
					map.insert(f.name.clone(), self.decode_field(f, r)?);
				}
				Ok(Value::Object(map))
			}
		}
	}

	fn decode_field(&self, f: &Field, r: &mut Reader) -> Result<Value> {
		if f.is_array {
			let len = r.var_uint()? as usize;
			// byte arrays are raw bytes, keep them compact
			if f.type_id == -2 {
				let bytes = r.bytes(len)?;
				return Ok(Value::Array(
					bytes.iter().map(|b| Value::from(*b)).collect(),
				));
			}
			let mut arr = Vec::with_capacity(len.min(1 << 16));
			for _ in 0..len {
				arr.push(self.decode_type(f.type_id, r)?);
			}
			Ok(Value::Array(arr))
		} else {
			self.decode_type(f.type_id, r)
		}
	}

	fn decode_type(&self, type_id: i32, r: &mut Reader) -> Result<Value> {
		Ok(match type_id {
			-1 => Value::Bool(r.byte()? != 0),
			-2 => Value::from(r.byte()?),
			-3 => Value::from(r.var_int()?),
			-4 => Value::from(r.var_uint()?),
			-5 => {
				let f = r.float()?;
				Number::from_f64(f as f64)
					.map(Value::Number)
					.unwrap_or(Value::Null)
			}
			-6 => Value::String(r.string()?),
			-7 => Value::from(r.var_int64()?),
			-8 => Value::from(r.var_uint64()?),
			t if t >= 0 => self.decode_def(t as usize, r)?,
			t => bail!("unknown builtin type {t}"),
		})
	}
}

pub struct Reader<'a> {
	data: &'a [u8],
	pos: usize,
}

impl<'a> Reader<'a> {
	pub fn new(data: &'a [u8]) -> Self {
		Self { data, pos: 0 }
	}

	pub fn byte(&mut self) -> Result<u8> {
		let b = *self.data.get(self.pos).context("unexpected end of data")?;
		self.pos += 1;
		Ok(b)
	}

	fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
		let end = self.pos.checked_add(len).context("overflow")?;
		let s = self
			.data
			.get(self.pos..end)
			.context("unexpected end of data")?;
		self.pos = end;
		Ok(s)
	}

	pub fn var_uint(&mut self) -> Result<u32> {
		let mut v: u32 = 0;
		let mut shift = 0;
		loop {
			let b = self.byte()?;
			v |= ((b & 127) as u32) << shift;
			shift += 7;
			if b & 128 == 0 || shift >= 35 {
				break;
			}
		}
		Ok(v)
	}

	pub fn var_int(&mut self) -> Result<i32> {
		let v = self.var_uint()?;
		Ok(((v >> 1) as i32) ^ -((v & 1) as i32))
	}

	fn var_uint64(&mut self) -> Result<u64> {
		let mut v: u64 = 0;
		let mut shift = 0;
		loop {
			let b = self.byte()?;
			v |= ((b & 127) as u64) << shift;
			shift += 7;
			if b & 128 == 0 || shift >= 70 {
				break;
			}
		}
		Ok(v)
	}

	fn var_int64(&mut self) -> Result<i64> {
		let v = self.var_uint64()?;
		Ok(((v >> 1) as i64) ^ -((v & 1) as i64))
	}

	fn float(&mut self) -> Result<f32> {
		let first = self.byte()?;
		if first == 0 {
			return Ok(0.0);
		}
		let rest = self.bytes(3)?;
		let bits = first as u32
			| (rest[0] as u32) << 8
			| (rest[1] as u32) << 16
			| (rest[2] as u32) << 24;
		let bits = bits.rotate_left(23);
		Ok(f32::from_bits(bits))
	}

	fn string(&mut self) -> Result<String> {
		let start = self.pos;
		while self.byte()? != 0 {}
		Ok(String::from_utf8_lossy(&self.data[start..self.pos - 1]).into_owned())
	}
}
