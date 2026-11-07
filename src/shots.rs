//! How Figma names exported screenshots, to match them to frames: a "/" in
//! a frame name becomes a folder ("Account / Favorites" →
//! `Account/Favorites.png`), characters that aren't allowed in file names
//! become "_" ("Editor: New" → `Editor_ New.png`), a second frame with the
//! same name gets "-1".

use std::path::{Component, Path};

use unicode_normalization::UnicodeNormalization;

/// Characters Figma's export replaces with "_" (not allowed in file names
/// on some system).
const REPLACED: &[char] = &['\\', ':', '*', '?', '"', '<', '>', '|'];

/// The path Figma's export gives a frame, without the extension:
/// `Account/Favorites 0.1` for "Account / Favorites 0.1".
pub fn export_name(frame: &str) -> String {
	let name: String = frame.nfc().collect();
	name.split('/')
		.map(|part| {
			part.trim()
				.chars()
				.map(|c| {
					if REPLACED.contains(&c) || c.is_control() {
						'_'
					} else {
						c
					}
				})
				.collect::<String>()
		})
		.collect::<Vec<_>>()
		.join("/")
}

/// Whether a folder name is a width class (`desktop/`, `tablet/`,
/// `mobile/`), which isn't part of the frame name.
pub fn is_width_folder(name: &str) -> bool {
	matches!(
		name.to_ascii_lowercase().as_str(),
		"desktop" | "tablet" | "mobile" | "phone"
	)
}

/// The frame name a screenshot stands for, as `export_name` writes it: its
/// path below the screenshots folder without width folders and extension;
/// and the same without Figma's "-1", "-2" suffix.
pub fn shot_names(root: &Path, path: &Path) -> (String, String) {
	let rel = path.strip_prefix(root).unwrap_or(path);
	let mut parts: Vec<String> = Vec::new();
	let count = rel.components().count();
	for (i, c) in rel.components().enumerate() {
		let Component::Normal(s) = c else {
			continue;
		};
		let s = s.to_string_lossy();
		if i + 1 == count {
			let stem = Path::new(s.as_ref())
				.file_stem()
				.map(|s| s.to_string_lossy().into_owned())
				.unwrap_or_default();
			parts.push(stem);
		} else if !is_width_folder(&s) {
			parts.push(s.into_owned());
		}
	}
	// macOS writes file names decomposed (NFD), Figma names are NFC
	let stem: String = parts.join("/").nfc().collect();
	let base = match stem.rsplit_once('-') {
		Some((b, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
			b.to_string()
		}
		_ => stem.clone(),
	};
	(stem, base)
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::path::PathBuf;

	fn names(path: &str) -> (String, String) {
		shot_names(Path::new("shots"), &PathBuf::from("shots").join(path))
	}

	#[test]
	fn plain_names() {
		assert_eq!(export_name("Landing page"), "Landing page");
		assert_eq!(
			names("Landing page.png"),
			("Landing page".into(), "Landing page".into())
		);
		assert_eq!(names("Landing page-1.png").1, "Landing page");
	}

	#[test]
	fn slash_becomes_a_folder() {
		assert_eq!(
			export_name("Account / Favorites 0.1"),
			"Account/Favorites 0.1"
		);
		assert_eq!(
			export_name("Account/Favorites 0.1"),
			"Account/Favorites 0.1"
		);
		assert_eq!(
			names("Account/Favorites 0.1.png").0,
			"Account/Favorites 0.1"
		);
		assert_eq!(
			names("Account/Favorites 0.1-1.png").1,
			"Account/Favorites 0.1"
		);
	}

	#[test]
	fn unsafe_characters_become_underscores() {
		assert_eq!(export_name("Recipe Editor: New"), "Recipe Editor_ New");
		assert_eq!(export_name("A? B* <C>"), "A_ B_ _C_");
		assert_eq!(names("Recipe Editor_ New-1.png").1, "Recipe Editor_ New");
	}

	#[test]
	fn width_folders_are_not_part_of_the_name() {
		assert_eq!(names("desktop/Landing page.png").0, "Landing page");
		assert_eq!(
			names("mobile/Account/Favorites 0.1.png").0,
			"Account/Favorites 0.1"
		);
		assert_eq!(names("Tablet/Home-2.png").1, "Home");
	}

	#[test]
	fn decomposed_umlauts() {
		// "Über uns" as macOS writes it (U + combining diaeresis)
		assert_eq!(names("U\u{308}ber uns.png").0, export_name("Über uns"));
	}
}
