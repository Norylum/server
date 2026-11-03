//! Inspect a single published item without rescanning the whole library.
use crate::{
    detect_mime_type, generate_deterministic_media_id, inspect_media_file, MediaItem, MediaScanner,
};
use norylum_foundation::NorylumError;
use norylum_metadata::{parse_relative_path, ParsedName};
use std::{
    fs,
    path::{Component, Path},
};

impl MediaScanner {
    pub async fn scan_one(&self, relative: &str) -> Result<MediaItem, NorylumError> {
        let rel = Path::new(relative);
        if relative.is_empty()
            || relative.len() > 4096
            || !rel.components().all(|c| match c {
                Component::Normal(v) => v
                    .to_str()
                    .is_some_and(|s| !s.starts_with('.') && !s.chars().any(char::is_control)),
                _ => false,
            })
        {
            return Err(NorylumError::Validation("Chemin du media invalide".into()));
        }
        let root = fs::canonicalize(&self.root_dir)?;
        let mut cursor = root.clone();
        for component in rel.components() {
            cursor.push(component.as_os_str());
            if fs::symlink_metadata(&cursor)?.file_type().is_symlink() {
                return Err(NorylumError::Validation("Lien symbolique refuse".into()));
            }
        }
        let file = fs::canonicalize(root.join(rel))?;
        if !file.starts_with(&root) || !fs::metadata(&file)?.is_file() {
            return Err(NorylumError::Validation("Media hors bibliotheque".into()));
        }
        let ext = file
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(
            ext.as_str(),
            "mkv" | "mp4" | "m4v" | "mov" | "avi" | "webm" | "ts" | "wmv"
        ) {
            return Err(NorylumError::Validation("Extension non video".into()));
        }
        let technical = inspect_media_file(&file).await?;
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Sans titre");
        let lower = stem.to_lowercase();
        let is_child_safe = !lower.contains("adult") && !lower.contains("18+");
        let age_rating = Some(if !is_child_safe {
            18
        } else if lower.contains("16+") {
            16
        } else if lower.contains("12+") {
            12
        } else {
            0
        });
        let title = match parse_relative_path(relative) {
            ParsedName::Movie { title, .. } => title,
            _ => stem.to_owned(),
        };
        Ok(MediaItem {
            id: generate_deterministic_media_id(relative, technical.file_size),
            title,
            file_path: file,
            relative_path: relative.to_owned(),
            file_size: technical.file_size,
            duration_seconds: technical.duration_seconds,
            mime_type: detect_mime_type(Path::new(relative)).to_owned(),
            video_codec: technical.video_codec,
            audio_codec: technical.audio_codec,
            width: technical.width,
            height: technical.height,
            is_child_safe,
            age_rating,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn rejects_traversal_and_hidden_paths() {
        let scanner = MediaScanner::new(std::env::temp_dir());
        assert!(scanner.scan_one("../passwd").await.is_err());
        assert!(scanner.scan_one(".norylum-staging/film.mkv").await.is_err());
        assert!(scanner.scan_one("/tmp/movie.mkv").await.is_err());
    }
}
