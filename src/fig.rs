//! Loading `.fig` files: an optional zip around `canvas.fig`, which holds a
//! compressed Kiwi schema and the compressed document.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::kiwi::Schema;

pub struct FigFile {
	pub message: Value,
	/// image hash (hex) -> file bytes
	pub images: HashMap<String, Vec<u8>>,
	/// video hash (hex) -> file bytes
	pub videos: HashMap<String, Vec<u8>>,
}

impl FigFile {
	pub fn open(path: &Path) -> Result<Self> {
		let data =
			std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;

		let mut images = HashMap::new();
		let mut videos = HashMap::new();
		let canvas = if data.starts_with(b"PK") {
			let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&data))?;
			let mut canvas = Vec::new();
			for i in 0..zip.len() {
				let mut file = zip.by_index(i)?;
				let name = file.name()?.to_string();
				if name == "canvas.fig" {
					file.read_to_end(&mut canvas)?;
				} else if let Some(hash) = name.strip_prefix("images/") {
					let mut buf = Vec::new();
					file.read_to_end(&mut buf)?;
					images.insert(hash.to_string(), buf);
				} else if let Some(hash) = name.strip_prefix("videos/") {
					let mut buf = Vec::new();
					file.read_to_end(&mut buf)?;
					videos.insert(hash.to_string(), buf);
				}
			}
			if canvas.is_empty() {
				bail!("no canvas.fig in zip");
			}
			canvas
		} else {
			data
		};

		let message = parse_canvas(&canvas)?;
		Ok(Self {
			message,
			images,
			videos,
		})
	}
}

fn parse_canvas(data: &[u8]) -> Result<Value> {
	if !data.starts_with(b"fig-kiwi") && !data.starts_with(b"fig-jam.") {
		bail!("not a fig-kiwi file");
	}
	let mut chunks = Vec::new();
	let mut pos = 12;
	while pos + 4 <= data.len() {
		let len = u32::from_le_bytes(data[pos..pos + 4].try_into()?) as usize;
		pos += 4;
		let chunk = data.get(pos..pos + len).context("truncated chunk")?;
		chunks.push(decompress(chunk)?);
		pos += len;
	}
	if chunks.len() < 2 {
		bail!("expected schema and data chunks");
	}
	let schema = Schema::parse(&chunks[0])?;
	let message = schema.decode("Message", &chunks[1])?;
	Ok(message)
}

fn decompress(chunk: &[u8]) -> Result<Vec<u8>> {
	if chunk.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
		return Ok(zstd::decode_all(chunk)?);
	}
	let mut out = Vec::new();
	flate2::read::DeflateDecoder::new(chunk).read_to_end(&mut out)?;
	Ok(out)
}
