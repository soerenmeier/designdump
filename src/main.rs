mod css;
mod doc;
mod fig;
mod html;
mod kiwi;
mod svg;
mod vars;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

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
		/// Directory with screenshots named after frames; exports every
		/// frame that has one
		#[arg(short, long)]
		screenshots: Option<PathBuf>,
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
		} => export(
			&file,
			&frame,
			screenshots.as_deref(),
			&out,
			fonts.as_deref(),
			responsive,
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

fn find_frames(doc: &Doc, wanted: &[String], shots: Option<&Path>) -> Result<Vec<Job>> {
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

	let Some(dir) = shots else { return Ok(jobs) };
	let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
		.filter_map(|e| e.ok().map(|e| e.path()))
		.filter(|p| {
			matches!(
				p.extension()
					.and_then(|e| e.to_str())
					.map(str::to_ascii_lowercase)
					.as_deref(),
				Some("png" | "jpg" | "jpeg" | "webp")
			)
		})
		.collect();
	entries.sort();
	for path in entries {
		let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
		let candidates: Vec<Guid> = frames
			.iter()
			.filter(|(_, f)| doc.name(*f) == stem)
			.map(|(_, f)| *f)
			.collect();
		if candidates.is_empty() {
			eprintln!("no frame for screenshot {}", path.display());
			continue;
		}
		// several frames with the same name: take the one whose aspect
		// ratio matches the screenshot
		let dim = imagesize::size(&path).ok();
		let best = candidates
			.iter()
			.min_by(|a, b| {
				let score = |g: &Guid| {
					let size = doc.node(*g).and_then(|n| n.get("size"));
					let w = size.and_then(|s| s["x"].as_f64()).unwrap_or(1.0);
					let h = size.and_then(|s| s["y"].as_f64()).unwrap_or(1.0);
					match dim {
						Some(d) => ((d.height as f64 / d.width as f64) - h / w).abs(),
						None => 0.0,
					}
				};
				score(a).total_cmp(&score(b))
			})
			.copied()
			.unwrap();
		match jobs.iter_mut().find(|j| j.frame == best) {
			Some(j) => j.screenshot = Some(path),
			None => jobs.push(Job {
				frame: best,
				screenshot: Some(path),
				slug: None,
				related: false,
			}),
		}
	}
	Ok(jobs)
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
fn smaller_versions(doc: &Doc, frame: Guid) -> Vec<(Guid, String)> {
	let width = |g: Guid| {
		doc.node(g)
			.and_then(|n| n.get("size"))
			.and_then(|s| s["x"].as_f64())
			.unwrap_or(0.0)
	};
	let w0 = width(frame);
	let mut names = HashSet::new();
	layer_names(doc, frame, 3, &mut names);

	let mut best: BTreeMap<i64, (f64, Guid)> = BTreeMap::new();
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
		let slot = best.entry(w.round() as i64).or_insert((-1.0, f));
		// on a tie the newer frame
		if score > slot.0 || (score == slot.0 && f > slot.1) {
			*slot = (score, f);
		}
	}

	// widest first
	let mut out = Vec::new();
	let mut larger = w0;
	for (w, (_, g)) in best.into_iter().rev() {
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
	shots: Option<&Path>,
	out: &Path,
	fonts: Option<&Path>,
	responsive: bool,
) -> Result<()> {
	let fig = fig::FigFile::open(file)?;
	let doc = Doc::new(&fig.message);
	let vars = vars::Vars::new(&doc);
	let jobs = find_frames(&doc, wanted, shots)?;
	let jobs: Vec<Job> = jobs;
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
	let mut jobs = jobs;
	if responsive {
		let mut extra = Vec::new();
		for job in &jobs {
			for (other, _) in smaller_versions(&doc, job.frame) {
				let w = doc
					.node(other)
					.and_then(|n| n.get("size"))
					.and_then(|s| s["x"].as_f64())
					.unwrap_or(0.0);
				extra.push(Job {
					frame: other,
					screenshot: None,
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

		let body = emitter.frame(&node)?;
		let mut related = Vec::new();
		if responsive && !job.related {
			for (other, query) in smaller_versions(&doc, job.frame) {
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
		let css = emitter.css();

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
			 <!-- Figma frame \"{title}\" ({id}) on page \"{page}\", {w} x {h}px{shot} -->\n{body}</body>\n</html>\n",
			title = escape(&name),
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
	tokens.push_str(&vars.tokens_css(&mode_widths(&doc)));
	tokens.push_str("\n/* Text styles */\n\n");
	tokens.push_str(&styles);
	std::fs::write(out.join("tokens.css"), tokens)?;

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
