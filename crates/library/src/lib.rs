//! Crate `library` pour Norylum Server.
//! Gère l'inspection multimédia via ffprobe, la génération d'index technique déterministe
//! et la protection stricte contre le Directory Traversal.

use norylum_foundation::{MediaId, NorylumError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Représentation canonique d'un fichier média indexé dans la bibliothèque Norylum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: MediaId,
    pub title: String,
    pub file_path: PathBuf,
    pub relative_path: String,
    pub file_size: u64,
    pub duration_seconds: f64,
    pub mime_type: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub is_child_safe: bool,
    pub age_rating: Option<u32>,
}

/// Garantit l'isolation stricte des répertoires pour prévenir tout Directory Traversal.
pub struct PathSanitizer;

impl PathSanitizer {
    /// Valide qu'un chemin (absolu ou relatif) réside strictement à l'intérieur de `root_dir`.
    /// Résout les liens symboliques et normalise les chemins pour empêcher toute évasion.
    pub fn sanitize_path(root_dir: &Path, requested_path: &Path) -> Result<PathBuf, NorylumError> {
        let canonical_root = std::fs::canonicalize(root_dir).map_err(|e| {
            NorylumError::Internal(format!(
                "Répertoire racine de la bibliothèque inaccessible ({:?}): {}",
                root_dir, e
            ))
        })?;

        // Reconstitution du chemin complet
        let target = if requested_path.is_absolute() {
            requested_path.to_path_buf()
        } else {
            root_dir.join(requested_path)
        };

        let canonical_target = match std::fs::canonicalize(&target) {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(NorylumError::NotFound(format!(
                    "Fichier média introuvable : {:?}",
                    requested_path
                )));
            }
            Err(e) => return Err(NorylumError::Io(e)),
        };

        // Vérification d'isolation
        if !canonical_target.starts_with(&canonical_root) {
            tracing::error!(
                root = ?canonical_root,
                target = ?canonical_target,
                "Alerte de sécurité : Tentative d'évasion de répertoire détectée !"
            );
            return Err(NorylumError::DirectoryTraversal(format!(
                "Le chemin {:?} sort du périmètre autorisé {:?}",
                requested_path, canonical_root
            )));
        }

        Ok(canonical_target)
    }
}

/// Génère un MediaId déterministe basé sur le chemin relatif normalisé et la taille du fichier.
pub fn generate_deterministic_media_id(relative_path: &str, file_size: u64) -> MediaId {
    let mut hasher = Sha256::new();
    hasher.update(relative_path.replace('\\', "/").as_bytes());
    hasher.update(file_size.to_be_bytes());
    let hash = hasher.finalize();
    let hex = format!("{:x}", hash);
    MediaId::new(format!("med_{}", &hex[..16]))
}

/// Déduit le type MIME à partir de l'extension de fichier.
pub fn detect_mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "ts" => "video/mp2t",
        "wmv" => "video/x-ms-wmv",
        _ => "video/mp4",
    }
}

/// Structures de désérialisation du JSON produit par `ffprobe`.
#[derive(Deserialize, Debug, Default)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    #[serde(default)]
    format: Option<FfprobeFormat>,
}

#[derive(Deserialize, Debug, Default)]
struct FfprobeStream {
    #[serde(default)]
    codec_type: Option<String>,
    #[serde(default)]
    codec_name: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Default)]
struct FfprobeFormat {
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    format_name: Option<String>,
}

/// Métadonnées techniques extraites d'un fichier média.
#[derive(Debug, Clone, Default)]
pub struct TechnicalMetadata {
    pub duration_seconds: f64,
    pub file_size: u64,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Inspecte un fichier multimédia via ffprobe.
pub async fn inspect_media_file(path: &Path) -> Result<TechnicalMetadata, NorylumError> {
    let fs_metadata = tokio::fs::metadata(path).await.map_err(|e| {
        NorylumError::NotFound(format!(
            "Impossible de lire les métadonnées de {:?}: {}",
            path, e
        ))
    })?;

    let file_size = fs_metadata.len();

    // Exécution de ffprobe en mode JSON
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let parsed: FfprobeOutput = serde_json::from_slice(&out.stdout).map_err(|e| {
                tracing::warn!(
                    path = ?path,
                    error = %e,
                    "Échec de désérialisation ffprobe JSON, utilisation des valeurs de secours"
                );
                NorylumError::FfprobeError(e.to_string())
            })?;

            let duration_seconds = parsed
                .format
                .as_ref()
                .and_then(|f| f.duration.as_ref())
                .and_then(|d| d.parse::<f64>().ok())
                .unwrap_or(0.0);

            let mut video_codec = None;
            let mut audio_codec = None;
            let mut width = None;
            let mut height = None;

            for stream in parsed.streams {
                match stream.codec_type.as_deref() {
                    Some("video") if video_codec.is_none() => {
                        video_codec = stream.codec_name;
                        width = stream.width;
                        height = stream.height;
                    }
                    Some("audio") if audio_codec.is_none() => {
                        audio_codec = stream.codec_name;
                    }
                    _ => {}
                }
            }

            Ok(TechnicalMetadata {
                duration_seconds,
                file_size,
                video_codec,
                audio_codec,
                width,
                height,
            })
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            tracing::warn!(
                path = ?path,
                stderr = %stderr,
                "ffprobe s'est terminé avec une erreur, fallback sur métadonnées de base"
            );
            Ok(TechnicalMetadata {
                duration_seconds: 0.0,
                file_size,
                video_codec: None,
                audio_codec: None,
                width: None,
                height: None,
            })
        }
        Err(err) => {
            tracing::warn!(
                path = ?path,
                error = %err,
                "ffprobe non disponible sur la machine hôte, fallback sur métadonnées de base"
            );
            Ok(TechnicalMetadata {
                duration_seconds: 0.0,
                file_size,
                video_codec: None,
                audio_codec: None,
                width: None,
                height: None,
            })
        }
    }
}

/// Moteur de scan et d'indexation de la bibliothèque Norylum.
#[derive(Debug, Clone)]
pub struct MediaScanner {
    root_dir: PathBuf,
}

impl MediaScanner {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
        }
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    /// Parcourt récursivement le dossier racine et indexe l'ensemble des fichiers vidéo.
    pub async fn scan(&self) -> Result<Vec<MediaItem>, NorylumError> {
        let canonical_root = std::fs::canonicalize(&self.root_dir).map_err(|e| {
            NorylumError::Internal(format!(
                "Dossier racine introuvable ({:?}): {}",
                self.root_dir, e
            ))
        })?;

        let mut items = Vec::new();
        let mut dir_queue = vec![canonical_root.clone()];

        while let Some(current_dir) = dir_queue.pop() {
            let mut entries = tokio::fs::read_dir(&current_dir)
                .await
                .map_err(NorylumError::Io)?;

            while let Some(entry) = entries.next_entry().await.map_err(NorylumError::Io)? {
                let path = entry.path();

                if path.is_dir() {
                    dir_queue.push(path);
                } else if path.is_file() {
                    let ext = path
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_lowercase();

                    let is_video = matches!(
                        ext.as_str(),
                        "mp4" | "mkv" | "avi" | "mov" | "webm" | "m4v" | "ts" | "wmv"
                    );

                    if is_video {
                        // Vérification stricte anti-traversal
                        let safe_path = PathSanitizer::sanitize_path(&canonical_root, &path)?;
                        let relative_path = safe_path
                            .strip_prefix(&canonical_root)
                            .map_err(|_| {
                                NorylumError::DirectoryTraversal("Préfixe invalide".into())
                            })?
                            .to_string_lossy()
                            .to_string();

                        let metadata = inspect_media_file(&safe_path).await?;
                        let media_id =
                            generate_deterministic_media_id(&relative_path, metadata.file_size);
                        let title = safe_path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Sans titre")
                            .to_string();

                        let mime_type = detect_mime_type(&safe_path).to_string();

                        // Règle de classification par défaut :
                        // Contenu marqué pour adulte si le nom de fichier contient explicitement "adult" ou "18+"
                        let lower_title = title.to_lowercase();
                        let is_child_safe =
                            !lower_title.contains("adult") && !lower_title.contains("18+");
                        let age_rating =
                            if lower_title.contains("adult") || lower_title.contains("18+") {
                                Some(18)
                            } else if lower_title.contains("16+") {
                                Some(16)
                            } else if lower_title.contains("12+") {
                                Some(12)
                            } else {
                                Some(0)
                            };

                        let parsed = norylum_metadata::parse_relative_path(&relative_path);
                        let final_title = match parsed {
                            norylum_metadata::ParsedName::Movie { title: m_title, .. } => m_title,
                            _ => title,
                        };

                        items.push(MediaItem {
                            id: media_id,
                            title: final_title,
                            file_path: safe_path,
                            relative_path,
                            file_size: metadata.file_size,
                            duration_seconds: metadata.duration_seconds,
                            mime_type,
                            video_codec: metadata.video_codec,
                            audio_codec: metadata.audio_codec,
                            width: metadata.width,
                            height: metadata.height,
                            is_child_safe,
                            age_rating,
                        });
                    }
                }
            }
        }

        tracing::info!(
            count = items.len(),
            root = ?canonical_root,
            "Scan de la bibliothèque média terminé avec succès"
        );

        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_id_generation() {
        let id1 = generate_deterministic_media_id("movies/inception.mp4", 1_000_000);
        let id2 = generate_deterministic_media_id("movies/inception.mp4", 1_000_000);
        let id3 = generate_deterministic_media_id("movies/other.mp4", 1_000_000);

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        assert!(id1.as_str().starts_with("med_"));
    }

    #[test]
    fn test_path_sanitizer_directory_traversal() {
        let temp_dir = std::env::temp_dir();
        let safe_sub = temp_dir.join(format!("norylum_test_safe_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&safe_sub);

        let test_file = safe_sub.join("video.mp4");
        std::fs::write(&test_file, b"dummy content").unwrap();

        // 1. Accès légitime
        let res = PathSanitizer::sanitize_path(&safe_sub, Path::new("video.mp4"));
        assert!(res.is_ok());

        // 2. Tentative d'évasion
        let traversal_path = Path::new("../../etc/passwd");
        let res = PathSanitizer::sanitize_path(&safe_sub, traversal_path);
        assert!(matches!(
            res,
            Err(NorylumError::DirectoryTraversal(_)) | Err(NorylumError::NotFound(_))
        ));

        // Nettoyage
        let _ = std::fs::remove_dir_all(safe_sub);
    }

    #[tokio::test]
    async fn test_scanner_with_dummy_media() {
        let base_tmp = std::env::temp_dir().join(format!("norylum_scan_{}", uuid::Uuid::new_v4()));
        let sub_dir = base_tmp.join("cartoons");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let video1 = sub_dir.join("tom_and_jerry.mp4");
        std::fs::write(&video1, b"mp4 data simulation").unwrap();

        let video2 = base_tmp.join("adult_movie_18+.mkv");
        std::fs::write(&video2, b"mkv adult simulation").unwrap();

        let text_file = base_tmp.join("notes.txt");
        std::fs::write(&text_file, b"non-media file").unwrap();

        let scanner = MediaScanner::new(&base_tmp);
        let items = scanner.scan().await.unwrap();

        assert_eq!(items.len(), 2);

        let cartoon = items.iter().find(|i| i.title == "tom_and_jerry").unwrap();
        assert!(cartoon.is_child_safe);
        assert_eq!(cartoon.mime_type, "video/mp4");

        let adult = items.iter().find(|i| i.title == "adult_movie_18+").unwrap();
        assert!(!adult.is_child_safe);
        assert_eq!(adult.age_rating, Some(18));
        assert_eq!(adult.mime_type, "video/x-matroska");

        // Clean up
        let _ = std::fs::remove_dir_all(base_tmp);
    }
}
