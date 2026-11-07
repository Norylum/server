//! Metadata V2: offline, checked container tag writer.
//! This command MUST NOT run while Norylum or a torrent
//! client is serving the media file.

#[path = "../instance_lock.rs"]
mod instance_lock;

use norylum_foundation::{MediaId, NorylumError};
use norylum_library::PathSanitizer;
use norylum_persistence::{Database, EmbeddedMediaTags};
use serde_json::{json, Value};
use std::{
    env, fs,
    fs::File,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use uuid::Uuid;
use xmltree::{Element, XMLNode};

fn internal(message: impl Into<String>) -> NorylumError {
    NorylumError::Internal(message.into())
}

fn checked_command(command: &mut Command, description: &str) -> Result<(), NorylumError> {
    let output = command
        .output()
        .map_err(|e| internal(format!("{description} : {e}")))?;

    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr);
        return Err(internal(format!(
            "{description} : {}",
            details.chars().take(1500).collect::<String>()
        )));
    }

    Ok(())
}

fn inspect(path: &Path) -> Result<Value, NorylumError> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-show_chapters",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|e| internal(format!("ffprobe indisponible : {e}")))?;

    if !output.status.success() {
        return Err(internal("ffprobe ne peut pas lire le media"));
    }

    let probe: Value =
        serde_json::from_slice(&output.stdout).map_err(|e| internal(e.to_string()))?;

    let streams = probe["streams"]
        .as_array()
        .ok_or_else(|| internal("Flux multimedia non detectes"))?;

    if streams.is_empty() {
        return Err(internal("Fichier sans piste multimedia"));
    }

    Ok(probe)
}

fn stream_signature(probe: &Value) -> Result<Vec<Value>, NorylumError> {
    let streams = probe["streams"]
        .as_array()
        .ok_or_else(|| internal("Flux multimedia invalides"))?;

    Ok(streams
        .iter()
        .map(|stream| {
            json!({
                "type": stream.get("codec_type"),
                "codec": stream.get("codec_name"),
                "width": stream.get("width"),
                "height": stream.get("height"),
                "channels": stream.get("channels"),
                "sample_rate": stream.get("sample_rate"),
            })
        })
        .collect())
}

fn chapter_count(probe: &Value) -> usize {
    probe["chapters"].as_array().map_or(0, Vec::len)
}

fn expected_tags(tags: &EmbeddedMediaTags) -> Vec<(&'static str, String)> {
    let mut expected = vec![
        ("TMDB", tags.tmdb_ref.clone()),
        ("NORYLUM_PROVENANCE", "Norylum Server".into()),
    ];

    if let Some(season) = tags.season {
        expected.push(("TV_SEASON", season.to_string()));
    }

    if let Some(episode) = tags.episode {
        expected.push(("TV_EPISODE", episode.to_string()));
    }

    expected
}

fn child_text<'a>(element: &'a Element, name: &str) -> Option<std::borrow::Cow<'a, str>> {
    element.get_child(name)?.get_text()
}

fn has_tag(root: &Element, name: &str, value: &str) -> bool {
    root.children.iter().any(|node| {
        let XMLNode::Element(tag) = node else {
            return false;
        };

        tag.name == "Tag"
            && tag.children.iter().any(|child| {
                let XMLNode::Element(simple) = child else {
                    return false;
                };

                simple.name == "Simple"
                    && child_text(simple, "Name").is_some_and(|n| n.eq_ignore_ascii_case(name))
                    && child_text(simple, "String").is_some_and(|v| v == value)
            })
    })
}

fn xml_text_element(name: &str, content: &str) -> Element {
    let mut element = Element::new(name);
    element.children.push(XMLNode::Text(content.into()));
    element
}

fn merge_global_tags(
    root: &mut Element,
    expected: &[(&'static str, String)],
) -> Result<(), NorylumError> {
    if root.name != "Tags" {
        return Err(internal("Format XML Matroska inattendu"));
    }

    // Preserve unrelated global metadata.
    // Only replace our own fields and the TMDB reference.
    for node in &mut root.children {
        let XMLNode::Element(tag) = node else {
            continue;
        };

        if tag.name != "Tag" {
            continue;
        }

        tag.children.retain(|child| {
            let XMLNode::Element(simple) = child else {
                return true;
            };

            if simple.name != "Simple" {
                return true;
            }

            let name = child_text(simple, "Name");

            !expected.iter().any(|(key, _)| {
                name.as_ref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(key))
            })
        });
    }

    let mut tag = Element::new("Tag");
    tag.children.push(XMLNode::Element(Element::new("Targets")));

    for (name, value) in expected {
        let mut simple = Element::new("Simple");

        simple
            .children
            .push(XMLNode::Element(xml_text_element("Name", name)));

        simple
            .children
            .push(XMLNode::Element(xml_text_element("String", value)));

        tag.children.push(XMLNode::Element(simple));
    }

    root.children.push(XMLNode::Element(tag));

    Ok(())
}

fn read_tags(path: &Path) -> Result<Element, NorylumError> {
    if !path.exists() || fs::metadata(path)?.len() == 0 {
        return Ok(Element::new("Tags"));
    }

    Element::parse(File::open(path)?).map_err(|e| internal(format!("Lecture XML Matroska : {e}")))
}

fn write_tags(path: &Path, root: &Element) -> Result<(), NorylumError> {
    let mut file = File::create(path)?;

    root.write(&mut file)
        .map_err(|e| internal(format!("Ecriture XML Matroska : {e}")))?;

    file.sync_all()?;
    Ok(())
}

fn extract_mkv_tags(source: &Path, output: &Path) -> Result<(), NorylumError> {
    let mut command = Command::new("mkvextract");

    command
        .arg(source)
        .arg("tags")
        .arg("--no-track-tags")
        .arg(output);

    checked_command(&mut command, "Extraction des tags MKV")
}

fn prepare_mkv(
    source: &Path,
    staged: &Path,
    tags_xml: &Path,
    tags: &EmbeddedMediaTags,
) -> Result<(), NorylumError> {
    fs::copy(source, staged)?;

    extract_mkv_tags(source, tags_xml)?;

    let mut root = read_tags(tags_xml)?;
    merge_global_tags(&mut root, &expected_tags(tags))?;
    write_tags(tags_xml, &root)?;

    let mut command = Command::new("mkvpropedit");

    command
        .arg(staged)
        .arg("--tags")
        .arg(format!("global:{}", tags_xml.display()));

    checked_command(&mut command, "Modification des tags MKV")
}

fn prepare_mp4(source: &Path, staged: &Path, tags: &EmbeddedMediaTags) -> Result<(), NorylumError> {
    let mut command = Command::new("ffmpeg");

    command
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-n", "-i"])
        .arg(source)
        .args([
            "-map",
            "0",
            "-c",
            "copy",
            "-map_metadata",
            "0",
            "-map_chapters",
            "0",
        ])
        .arg("-metadata")
        .arg(format!("TMDB={}", tags.tmdb_ref))
        .arg("-metadata")
        .arg("NORYLUM_PROVENANCE=Norylum Server");

    if let Some(season) = tags.season {
        command.arg("-metadata").arg(format!("TV_SEASON={season}"));
    }

    if let Some(episode) = tags.episode {
        command
            .arg("-metadata")
            .arg(format!("TV_EPISODE={episode}"));
    }

    command
        .args(["-movflags", "+use_metadata_tags", "-f", "mp4"])
        .arg(staged);

    checked_command(&mut command, "Remultiplexage MP4 sans reencodage")
}

fn verify_mp4_tags(probe: &Value, tags: &EmbeddedMediaTags) -> Result<(), NorylumError> {
    let actual = probe["format"]["tags"]
        .as_object()
        .ok_or_else(|| internal("Tags MP4 introuvables"))?;

    for (key, value) in expected_tags(tags) {
        let matched = actual
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case(key) && v.as_str() == Some(value.as_str()));

        if !matched {
            return Err(internal(format!("Tag MP4 non verifie : {key}")));
        }
    }

    Ok(())
}

fn verify_mkv_tags(
    staged: &Path,
    check_xml: &Path,
    tags: &EmbeddedMediaTags,
) -> Result<(), NorylumError> {
    extract_mkv_tags(staged, check_xml)?;

    let root = read_tags(check_xml)?;

    for (name, value) in expected_tags(tags) {
        if !has_tag(&root, name, &value) {
            return Err(internal(format!("Tag MKV non verifie : {name}")));
        }
    }

    Ok(())
}

/// Temporary files are removed on every normal return,
/// including errors before the final replacement.
struct TemporaryFiles(Vec<PathBuf>);

impl Drop for TemporaryFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

fn usage() {
    println!(
        "Utilisation : \
         norylum-metadata-tags --media-id med_ID [--apply]"
    );
}

async fn run() -> Result<(), NorylumError> {
    let mut media_id: Option<String> = None;
    let mut apply = false;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--media-id" => {
                media_id = args.next();
            }
            "--apply" => {
                apply = true;
            }
            "--help" | "-h" => {
                usage();
                return Ok(());
            }
            _ => {
                return Err(NorylumError::Validation("Argument inconnu".into()));
            }
        }
    }

    let id = media_id.ok_or_else(|| NorylumError::Validation("--media-id requis".into()))?;

    let media_id = MediaId::new(id);

    let root = env::var("NORYLUM_MEDIA_DIR").unwrap_or_else(|_| "./media".into());

    let root = fs::canonicalize(root)?;

    // Preview is read-only. Applying changes takes a TaggerLock so two
    // concurrent tagging runs cannot interfere. The server keeps its own
    // InstanceLock on a different file, so they can run simultaneously.
    let _tagger_lock = if apply {
        Some(instance_lock::TaggerLock::acquire(&root).map_err(|e| {
            NorylumError::Conflict(format!(
                "Un tagging Norylum est deja en cours : {e}"
            ))
        })?)
    } else {
        None
    };

    let db_path = root.join("norylum.sqlite3");

    if !db_path.is_file() {
        return Err(NorylumError::NotFound(
            "Base SQLite Norylum introuvable".into(),
        ));
    }

    let db = Database::open(db_path)?;

    let item = db
        .get_media_by_id(&media_id)
        .await?
        .ok_or_else(|| NorylumError::NotFound("Media non indexe".into()))?;

    let tags = db
        .embedded_tags_for_media(&media_id)
        .await?
        .ok_or_else(|| {
            NorylumError::NotFound(
                "Association TMDB absente : \
                 terminer Metadata V1 d'abord"
                    .into(),
            )
        })?;

    let source = PathSanitizer::sanitize_path(&root, &item.file_path)?;

    let extension = source
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if !matches!(extension.as_str(), "mkv" | "mp4" | "m4v") {
        return Err(NorylumError::Validation(
            "Seuls MKV, MP4 et M4V sont pris en charge".into(),
        ));
    }

    let original_info = fs::metadata(&source)?;

    if !original_info.is_file() {
        return Err(NorylumError::Validation(
            "Le media doit etre un fichier regulier".into(),
        ));
    }

    let before = inspect(&source)?;

    println!("Media : {}", media_id);
    println!("Fichier : {}", source.display());
    println!("TMDB : {}", tags.tmdb_ref);
    println!("Provenance : Norylum Server");

    if let Some(season) = tags.season {
        println!("Saison : {season}");
    }

    if let Some(episode) = tags.episode {
        println!("Episode : {episode}");
    }

    if !apply {
        println!("PREVISUALISATION : aucune modification.");
        println!(
            "Relancer avec --apply, serveur arrete, \
             pour ecrire les tags."
        );
        return Ok(());
    }

    let age = original_info.modified()?.elapsed().unwrap_or_default();

    if age < Duration::from_secs(30) {
        return Err(NorylumError::Validation(
            "Fichier modifie trop recemment : \
             attendre la fin de l'ingestion"
                .into(),
        ));
    }

    let parent = source
        .parent()
        .ok_or_else(|| internal("Repertoire parent introuvable"))?;

    let filename = source
        .file_name()
        .ok_or_else(|| internal("Nom de fichier invalide"))?
        .to_string_lossy();

    let nonce = Uuid::new_v4().simple();

    let staged = parent.join(format!(".{filename}.norylum-{nonce}.work"));

    let tags_xml = parent.join(format!(".norylum-tags-{nonce}.xml"));

    let check_xml = parent.join(format!(".norylum-verify-{nonce}.xml"));

    let backup = parent.join(format!(".{filename}.norylum-{nonce}.bak"));

    if staged.exists() || backup.exists() {
        return Err(internal("Collision de nom temporaire"));
    }

    let _temp_cleanup = TemporaryFiles(vec![staged.clone(), tags_xml.clone(), check_xml.clone()]);

    if extension == "mkv" {
        prepare_mkv(&source, &staged, &tags_xml, &tags)?;
    } else {
        prepare_mp4(&source, &staged, &tags)?;
    }

    let after = inspect(&staged)?;

    if stream_signature(&before)? != stream_signature(&after)? {
        return Err(internal("Les types ou codecs des pistes ont change"));
    }

    if chapter_count(&before) != chapter_count(&after) {
        return Err(internal("Le nombre de chapitres a change"));
    }

    if extension == "mkv" {
        verify_mkv_tags(&staged, &check_xml, &tags)?;
    } else {
        verify_mp4_tags(&after, &tags)?;
    }

    // Check the original again before any replacement.
    let current_info = fs::metadata(&source)?;

    if current_info.len() != original_info.len()
        || current_info.modified()? != original_info.modified()?
    {
        return Err(internal("Le fichier a change pendant l'operation"));
    }

    fs::set_permissions(&staged, original_info.permissions())?;

    // Both renames happen on the same filesystem.
    // The original stays available as a .bak file.
    fs::rename(&source, &backup)?;

    if let Err(error) = fs::rename(&staged, &source) {
        let restored = fs::rename(&backup, &source);

        return Err(internal(format!(
            "Remplacement refuse : {error}. \
             Restauration : {restored:?}"
        )));
    }

    let updated_size = fs::metadata(&source)?.len();

    let size = i64::try_from(updated_size).map_err(|e| internal(e.to_string()))?;

    // The path and media ID remain unchanged.
    db.with_connection(|conn| {
        conn.execute(
            "UPDATE media_items
             SET file_size = ?2,
                 updated_at = strftime(
                     '%Y-%m-%dT%H:%M:%fZ', 'now'
                 )
             WHERE id = ?1",
            rusqlite::params![media_id.as_str(), size,],
        )
        .map_err(|e| internal(e.to_string()))?;

        Ok(())
    })
    .await?;

    println!("Tags verifies et fichier remplace.");
    println!("Sauvegarde originale : {}", backup.display());

    println!(
        "Conserver la sauvegarde jusqu'a verification \
         manuelle de la lecture."
    );

    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Erreur Metadata V2 : {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xml_merge_preserves_unrelated_tags() {
        let old = r#"
        <Tags>
            <Tag>
                <Targets/>
                <Simple>
                    <Name>ARTIST</Name>
                    <String>Existing metadata</String>
                </Simple>
                <Simple>
                    <Name>TMDB</Name>
                    <String>movie/1</String>
                </Simple>
            </Tag>
        </Tags>
        "#;

        let mut xml = Element::parse(old.as_bytes()).unwrap();

        let tags = EmbeddedMediaTags {
            tmdb_ref: "movie/603".into(),
            provenance: "Norylum Server".into(),
            season: None,
            episode: None,
        };

        merge_global_tags(&mut xml, &expected_tags(&tags)).unwrap();

        assert!(has_tag(&xml, "ARTIST", "Existing metadata"));

        assert!(has_tag(&xml, "TMDB", "movie/603"));

        assert!(!has_tag(&xml, "TMDB", "movie/1"));

        assert!(has_tag(&xml, "NORYLUM_PROVENANCE", "Norylum Server"));
    }

    #[test]
    fn test_tv_tags_have_episode_numbers() {
        let tags = EmbeddedMediaTags {
            tmdb_ref: "tv/1396".into(),
            provenance: "Norylum Server".into(),
            season: Some(2),
            episode: Some(5),
        };

        let expected = expected_tags(&tags);

        assert!(expected.contains(&("TMDB", "tv/1396".into())));

        assert!(expected.contains(&("TV_SEASON", "2".into())));

        assert!(expected.contains(&("TV_EPISODE", "5".into())));
    }
}
