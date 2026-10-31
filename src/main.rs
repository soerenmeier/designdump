mod components;
mod css;
mod doc;
mod fig;
mod grid;
mod html;
mod kiwi;
mod map;
mod svg;
mod texts;
mod vars;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use unicode_normalization::UnicodeNormalization;

use crate::css::{escape, kebab, num};
use crate::doc::{Doc, Guid};

#[derive(Parser)]
#[command(about = "Turn Figma .fig files into HTML and CSS that AI agents can read")]
struct Args {
	#[command(subcommand)]
	cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
	/// List pages and their top level frames
	List { file: PathBuf },
	/// Export frames as HTML + CSS with design tokens
	Export {
		file: PathBuf,
		/// Frame name or id (e.g. "Landing page" or 2042:1131), repeatable
		#[arg(short, long)]
		frame: Vec<String>,
		/// Directory with screenshots named after frames (searched
		/// recursively, repeatable); without --frame every frame that has
		/// one is exported
		#[arg(short, long)]
		screenshots: Vec<PathBuf>,
		/// Output directory
		#[arg(short, long, default_value = "out")]
		out: PathBuf,
		/// CSS file with @font-face rules to include in the preview
		#[arg(long)]
		fonts: Option<PathBuf>,
		/// Add media queries from frames with the same name and a smaller
		/// width (tablet / mobile designs)
		#[arg(short, long)]
		responsive: bool,
		/// Project directory: map the Figma tokens to its CSS variables and
		/// SCSS mixins (written at the top of tokens.css)
		#[arg(short, long)]
		project: Option<PathBuf>,
	},
	/// Dump the decoded document (or one node) as JSON
	Dump {
		file: PathBuf,
		#[arg(long)]
		node: Option<String>,
	},
}

fn main() -> Result<()> {
	let args = Args::parse();
	match args.cmd {
		Cmd::List { file } => list(&file),
		Cmd::Export {
			file,
			frame,
			screenshots,
			out,
			fonts,
			responsive,
			project,
		} => export(
			&file,
			&frame,
			&screenshots,
			&out,
			fonts.as_deref(),
			responsive,
			project.as_deref(),
		),
		Cmd::Dump { file, node } => {
			let fig = fig::FigFile::open(&file)?;
			let out = std::io::stdout().lock();
			match node {
				Some(id) => {
					let doc = Doc::new(&fig.message);
					let g: Guid = id.parse().ok().context("invalid node id")?;
					let n = doc.resolve(g).context("node not found")?;
					serde_json::to_writer_pretty(out, &debug_tree(&n))?;
				}
				None => serde_json::to_writer(out, &fig.message)?,
			}
			Ok(())
		}
	}
}

fn debug_tree(n: &doc::RNode) -> serde_json::Value {
	let mut p = n.p.clone();
	for k in [
		"fillGeometry",
		"strokeGeometry",
		"derivedTextData",
		"derivedSymbolData",
		"vectorData",
		"editInfo",
	] {
		p.remove(k);
	}
	p.insert(
		"children".into(),
		n.children.iter().map(debug_tree).collect::<Vec<_>>().into(),
	);
	serde_json::Value::Object(p)
}

/// Top level frames: (page, frame)
fn top_frames(doc: &Doc) -> Vec<(Guid, Guid)> {
	let mut out = Vec::new();
	for page in doc.kids(doc.root) {
		for f in doc.kids(*page) {
			if matches!(doc.ty(*f), "FRAME" | "SYMBOL" | "SECTION") {
				out.push((*page, *f));
			}
		}
	}
	out
}

fn list(file: &Path) -> Result<()> {
	let fig = fig::FigFile::open(file)?;
	let doc = Doc::new(&fig.message);
	let mut last = None;
	for (page, f) in top_frames(&doc) {
		if last != Some(page) {
			println!("{}", doc.name(page));
			last = Some(page);
		}
		let size = doc.node(f).and_then(|n| n.get("size"));
		let w = size.and_then(|s| s["x"].as_f64()).unwrap_or(0.0);
		let h = size.and_then(|s| s["y"].as_f64()).unwrap_or(0.0);
		println!(
			"  {:<12} {:>5} x {:<5} {}",
			f.to_string(),
			num(w),
			num(h),
			doc.name(f)
		);
	}
	Ok(())
}

/// Which mode of a variable collection each frame width was designed for.
fn mode_widths(doc: &Doc) -> HashMap<Guid, f64> {
	let mut widths: HashMap<Guid, f64> = HashMap::new();
	for (_, f) in top_frames(doc) {
		let Some(n) = doc.node(f) else { continue };
		let w = n.get("size").and_then(|s| s["x"].as_f64()).unwrap_or(0.0);
		let entries = n
			.get("variableModeBySetMap")
			.and_then(|m| m.get("entries"))
			.and_then(|e| e.as_array());
		for e in entries.into_iter().flatten() {
			if let Some(mode) = e.get("variableModeID").and_then(Guid::from_value) {
				let slot = widths.entry(mode).or_insert(w);
				*slot = slot.min(w);
			}
		}
	}
	widths
}

struct Job {
	frame: Guid,
	screenshot: Option<PathBuf>,
	/// file name instead of the frame name
	slug: Option<String>,
	/// a smaller version of another exported frame
	related: bool,
}

fn find_frames(
	doc: &Doc,
	wanted: &[String],
	dirs: &[PathBuf],
) -> Result<(Vec<Job>, HashMap<Guid, PathBuf>)> {
	let frames = top_frames(doc);
	let mut jobs = Vec::new();
	for w in wanted {
		if let Ok(g) = w.parse::<Guid>()
			&& doc.node(g).is_some()
		{
			jobs.push(Job {
				frame: g,
				screenshot: None,
				slug: None,
				related: false,
			});
			continue;
		}
		let matches: Vec<Guid> = frames
			.iter()
			.filter(|(_, f)| doc.name(*f) == w)
			.map(|(_, f)| *f)
			.collect();
		match matches.as_slice() {
			[] => bail!("no frame named {w:?}"),
			[g] => jobs.push(Job {
				frame: *g,
				screenshot: None,
				slug: None,
				related: false,
			}),
			_ => {
				let ids: Vec<String> = matches.iter().map(|g| g.to_string()).collect();
				bail!(
					"{} frames are named {w:?}, use an id: {}",
					ids.len(),
					ids.join(", ")
				);
			}
		}
	}

	let shots = match_screenshots(doc, dirs)?;
	if jobs.is_empty() {
		let mut ids: Vec<Guid> = shots.keys().copied().collect();
		ids.sort();
		jobs = ids
			.into_iter()
			.map(|g| Job {
				frame: g,
				screenshot: None,
				slug: None,
				related: false,
			})
			.collect();
	}
	for j in &mut jobs {
		j.screenshot = shots.get(&j.frame).cloned();
	}
	Ok((jobs, shots))
}

fn image_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
	for e in std::fs::read_dir(dir)? {
		let p = e?.path();
		if p.is_dir() {
			image_files(&p, out)?;
		} else if matches!(
			p.extension()
				.and_then(|e| e.to_str())
				.map(str::to_ascii_lowercase)
				.as_deref(),
			Some("png" | "jpg" | "jpeg" | "webp")
		) {
			out.push(p);
		}
	}
	Ok(())
}

/// Match screenshots to frames: by name (Figma adds "-1", "-2" to
/// exports of frames with the same name) and by size, each frame once.
fn match_screenshots(doc: &Doc, dirs: &[PathBuf]) -> Result<HashMap<Guid, PathBuf>> {
	let frames = top_frames(doc);
	let mut files = Vec::new();
	for d in dirs {
		image_files(d, &mut files)?;
	}
	files.sort();

	// (score, frame, file), lower is better
	let mut options: Vec<(f64, Guid, usize)> = Vec::new();
	for (i, path) in files.iter().enumerate() {
		// macOS writes file names decomposed (NFD), Figma names are NFC
		let stem: String = path
			.file_stem()
			.and_then(|s| s.to_str())
			.unwrap_or("")
			.nfc()
			.collect();
		let stem = stem.as_str();
		let base = match stem.rsplit_once('-') {
			Some((b, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => b,
			_ => stem,
		};
		let Ok(dim) = imagesize::size(path) else {
			continue;
		};
		for (_, f) in &frames {
			let name: String = doc.name(*f).nfc().collect();
			if name != stem && name != base {
				continue;
			}
			let size = doc.node(*f).and_then(|n| n.get("size"));
			let w = size.and_then(|s| s["x"].as_f64()).unwrap_or(1.0);
			let h = size.and_then(|s| s["y"].as_f64()).unwrap_or(1.0);
			// exports are at 1x, 2x, ...
			let scale = (dim.width as f64 / w).round().max(1.0);
			let score = (dim.width as f64 / scale - w).abs() / w
				+ (dim.height as f64 / scale - h).abs() / h;
			if score < 0.1 {
				options.push((score, *f, i));
			}
		}
		if !options.iter().any(|o| o.2 == i) {
			eprintln!("no frame for screenshot {}", path.display());
		}
	}
	options.sort_by(|a, b| a.0.total_cmp(&b.0));
	let mut out = HashMap::new();
	let mut used = HashSet::new();
	for (_, f, i) in options {
		if out.contains_key(&f) || used.contains(&i) {
			continue;
		}
		used.insert(i);
		out.insert(f, files[i].clone());
	}
	Ok(out)
}

/// Names of the layers in a subtree, for comparing frames.
fn layer_names(doc: &Doc, g: Guid, depth: usize, out: &mut HashSet<String>) {
	for c in doc.kids(g) {
		out.insert(doc.name(*c).to_string());
		if depth > 0 {
			layer_names(doc, *c, depth - 1, out);
		}
	}
}

/// Frames with the same name and a smaller width, the most similar one per
/// width, with a media query each (breakpoints halfway between widths).
/// Frames with a screenshot win over frames without one.
fn smaller_versions(
	doc: &Doc,
	frame: Guid,
	shots: &HashMap<Guid, PathBuf>,
) -> Vec<(Guid, String)> {
	let width = |g: Guid| {
		doc.node(g)
			.and_then(|n| n.get("size"))
			.and_then(|s| s["x"].as_f64())
			.unwrap_or(0.0)
	};
	let w0 = width(frame);
	let mut names = HashSet::new();
	layer_names(doc, frame, 3, &mut names);

	let mut best: BTreeMap<i64, (bool, f64, Guid)> = BTreeMap::new();
	for (_, f) in top_frames(doc) {
		let w = width(f);
		if f == frame || doc.name(f) != doc.name(frame) || w >= w0 - 1.0 {
			continue;
		}
		let mut other = HashSet::new();
		layer_names(doc, f, 3, &mut other);
		let common = names.intersection(&other).count() as f64;
		let score = common / names.union(&other).count().max(1) as f64;
		if std::env::var_os("FIG_DEBUG").is_some() {
			eprintln!("{f} {w} similarity {score:.2}");
		}
		// unrelated frames that happen to have the same name
		if score < 0.3 {
			continue;
		}
		let shot = shots.contains_key(&f);
		let slot = best.entry(w.round() as i64).or_insert((shot, -1.0, f));
		// a screenshot first, then similarity, on a tie the newer frame
		if (shot, score, f) > *slot {
			*slot = (shot, score, f);
		}
	}

	// widest first
	let mut out = Vec::new();
	let mut larger = w0;
	for (w, (_, _, g)) in best.into_iter().rev() {
		let bp = ((w as f64 + larger) / 2.0).floor();
		out.push((g, format!("(max-width: {}px)", num(bp))));
		larger = w as f64;
	}
	out
}

const BASE_CSS: &str = "*, *::before, *::after {
	box-sizing: border-box;
	margin: 0;
	padding: 0;
}

img {
	display: block;
}
";

fn export(
	file: &Path,
	wanted: &[String],
	shot_dirs: &[PathBuf],
	out: &Path,
	fonts: Option<&Path>,
	responsive: bool,
	project: Option<&Path>,
) -> Result<()> {
	let fig = fig::FigFile::open(file)?;
	let doc = Doc::new(&fig.message);
	let vars = vars::Vars::new(&doc);
	let (mut jobs, shots) = find_frames(&doc, wanted, shot_dirs)?;
	if responsive {
		// smaller versions of another frame come with that frame
		let related: HashSet<Guid> = jobs
			.iter()
			.flat_map(|j| smaller_versions(&doc, j.frame, &shots))
			.map(|(g, _)| g)
			.collect();
		jobs.retain(|j| !related.contains(&j.frame));
	}
	if jobs.is_empty() {
		bail!("nothing to export, give --frame or --screenshots");
	}
	std::fs::create_dir_all(out)?;

	let mut emitter = html::Emitter::new(
		&doc,
		&vars,
		&fig.images,
		&fig.videos,
		&out.join("assets"),
		"assets",
	);
	let mut index = Vec::new();
	let mut slugs: HashMap<String, usize> = HashMap::new();

	// the smaller designs also on their own, the merged media queries are
	// only a best effort
	if responsive {
		let mut extra = Vec::new();
		for job in &jobs {
			for (other, _) in smaller_versions(&doc, job.frame, &shots) {
				let w = doc
					.node(other)
					.and_then(|n| n.get("size"))
					.and_then(|s| s["x"].as_f64())
					.unwrap_or(0.0);
				extra.push(Job {
					frame: other,
					screenshot: shots.get(&other).cloned(),
					slug: Some(format!("{}-{}", kebab(doc.name(other)), num(w))),
					related: true,
				});
			}
		}
		jobs.extend(extra);
	}

	for job in &jobs {
		let node = doc.resolve(job.frame).context("frame is hidden")?;
		let name = doc.name(job.frame).to_string();
		let mut slug = job.slug.clone().unwrap_or_else(|| kebab(&name));
		let n = slugs.entry(slug.clone()).or_default();
		*n += 1;
		if *n > 1 {
			slug = format!("{slug}-{n}");
		}

		emitter.frame_label = (slug.clone(), node.size().0);
		let body = emitter.frame(&node)?;
		let mut related = Vec::new();
		if responsive && !job.related {
			for (other, query) in smaller_versions(&doc, job.frame, &shots) {
				let Some(tree) = doc.resolve(other) else {
					continue;
				};
				let label = format!(
					"\"{}\" designed at {}px ({other}), breakpoint guessed",
					doc.name(other),
					num(tree.size().0)
				);
				emitter.responsive(&node, &tree, &query, &label);
				related.push(other.to_string());
			}
		}
		let css = grid::comment(&node, "/*", "*/") + &emitter.css();

		let shot = match &job.screenshot {
			Some(p) => {
				let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("png");
				let f = format!("{slug}.screenshot.{}", ext.to_ascii_lowercase());
				std::fs::copy(p, out.join(&f))?;
				Some(f)
			}
			None => None,
		};

		let (w, h) = node.size();
		let page = doc.page_of(job.frame).map(|p| doc.name(p)).unwrap_or("");
		let mut html = String::new();
		let _ = write!(
			html,
			"<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{title}</title>\n\
			 <link rel=\"stylesheet\" href=\"base.css\">\n<link rel=\"stylesheet\" href=\"tokens.css\">\n\
			 <link rel=\"stylesheet\" href=\"{slug}.css\">\n</head>\n<body>\n\
			 <!-- Figma frame \"{title}\" ({id}) on page \"{page}\", {w} x {h}px{shot} -->\n{facts}{body}</body>\n</html>\n",
			title = escape(&name),
			facts = grid::comment(&node, "<!--", "-->"),
			id = job.frame,
			page = escape(page),
			w = num(w),
			h = num(h),
			shot = shot
				.as_ref()
				.map(|s| format!(", screenshot {s}"))
				.unwrap_or_default(),
		);
		std::fs::write(out.join(format!("{slug}.html")), html)?;
		std::fs::write(out.join(format!("{slug}.css")), css)?;
		if related.is_empty() {
			println!("{} -> {slug}.html", job.frame);
		} else {
			println!("{} (+ {}) -> {slug}.html", job.frame, related.join(", "));
		}
		index.push((slug, name, shot));
	}

	// text styles first, they use variables too
	let styles = emitter.text_styles_css();
	let mut tokens = String::from("/* Design tokens from Figma variables */\n\n");
	let token_css = vars.tokens_css(&mode_widths(&doc));
	if let Some(dir) = project {
		let p = map::Project::load(dir)?;
		let styles: Vec<(String, String)> = emitter
			.text_styles
			.iter()
			.map(|(g, class)| (class.clone(), doc.name(*g).to_string()))
			.collect();
		tokens.push_str(&map::mapping(&vars, &styles, &p));
	}
	tokens.push_str(&token_css);
	tokens.push_str("\n/* Text styles */\n\n");
	tokens.push_str(&styles);
	std::fs::write(out.join("tokens.css"), tokens)?;
	std::fs::write(
		out.join("components.md"),
		components::markdown(&emitter.uses),
	)?;
	std::fs::write(out.join("texts.md"), texts::markdown(&emitter.texts))?;

	let mut list = String::new();
	for (slug, name, shot) in &index {
		let _ = write!(list, "<li><a href=\"{slug}.html\">{}</a>", escape(name));
		if let Some(shot) = shot {
			let _ = write!(list, " (<a href=\"{shot}\">screenshot</a>)");
		}
		list.push_str("</li>\n");
	}
	std::fs::write(
		out.join("index.html"),
		format!(
			"<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>Frames</title>\n</head>\n\
			 <body style=\"font-family: sans-serif; padding: 2em\">\n<ul>\n{list}</ul>\n</body>\n</html>\n"
		),
	)?;

	// fonts first, they may start with @import
	let mut base = String::new();
	if let Some(f) = fonts {
		base.push_str(&std::fs::read_to_string(f)?);
		base.push('\n');
	}
	base.push_str(BASE_CSS);
	std::fs::write(out.join("base.css"), base)?;
	Ok(())
}
