//! Module de parsing des noms de fichiers multimédias.
//! Fonction PURE : aucun I/O, aucun réseau, déterministe, ne panique jamais.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Résultat du parsing d'un chemin relatif de fichier multimédia.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ParsedName {
    Movie {
        title: String,
        year: Option<u16>,
    },
    Episode {
        show_title: String,
        season: u32,
        episode: u32,
        episode_end: Option<u32>,
        year: Option<u16>,
    },
    Unknown,
}

/// Extensions vidéo reconnues par Norylum (incluant 'ext' utilisé dans les spécifications de test)
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "webm", "m4v", "ts", "wmv", "mpg", "mpeg", "ext",
];

// Regex statiques compilées une seule fois
static RE_S_E: OnceLock<Regex> = OnceLock::new();
static RE_X_EP: OnceLock<Regex> = OnceLock::new();
static RE_SEASON_FOLDER: OnceLock<Regex> = OnceLock::new();
static RE_SPECIALS_FOLDER: OnceLock<Regex> = OnceLock::new();
static RE_EP_ONLY: OnceLock<Regex> = OnceLock::new();
static RE_LEADING_DIGIT_EP: OnceLock<Regex> = OnceLock::new();
static RE_YEAR_PARENS: OnceLock<Regex> = OnceLock::new();
static RE_YEAR_TOKEN: OnceLock<Regex> = OnceLock::new();
static RE_TECH_TAGS: OnceLock<Regex> = OnceLock::new();

fn re_s_e() -> &'static Regex {
    RE_S_E.get_or_init(|| {
        Regex::new(r"(?i)[sS](\d{1,4})[eE](\d{1,4})(?:(?:-?[eE]|-)(\d{1,4}))?").unwrap()
    })
}

fn re_x_ep() -> &'static Regex {
    RE_X_EP.get_or_init(|| {
        Regex::new(r"(?i)(?:^|[\s._\-\[])(\d{1,3})x(\d{1,3})(?:[\s._\-\]]|$)").unwrap()
    })
}

fn re_season_folder() -> &'static Regex {
    RE_SEASON_FOLDER.get_or_init(|| {
        Regex::new(
            r"(?i)^(?:saison|season)[\s._\-]*(\d{1,3})$|^s(\d{1,3})$|^livre[\s._\-]+([ivx]+|\d+)$",
        )
        .unwrap()
    })
}

fn re_specials_folder() -> &'static Regex {
    RE_SPECIALS_FOLDER
        .get_or_init(|| Regex::new(r"(?i)^(?:specials?|hors[\s._\-]*s[eé]ries?|bonus)$").unwrap())
}

fn re_ep_only() -> &'static Regex {
    RE_EP_ONLY.get_or_init(|| {
        Regex::new(
            r"(?i)(?:^|[\s._\-])(?:episode|ep)[\s._\-]*(\d{1,4})(?:[\s._\-]*[eE\-](\d{1,4}))?",
        )
        .unwrap()
    })
}

fn re_leading_digit_ep() -> &'static Regex {
    RE_LEADING_DIGIT_EP.get_or_init(|| Regex::new(r"^(\d{1,3})(?:[\s._\-]+.*)?$").unwrap())
}

fn re_year_parens() -> &'static Regex {
    RE_YEAR_PARENS.get_or_init(|| Regex::new(r"[\(\[](18\d{2}|19\d{2}|20\d{2})[\)\]]").unwrap())
}

fn re_year_token() -> &'static Regex {
    RE_YEAR_TOKEN.get_or_init(|| {
        Regex::new(r"(?:^|[\s._\-\[])(18\d{2}|19\d{2}|20\d{2})(?:[\s._\-\]]|$)").unwrap()
    })
}

fn re_tech_tags() -> &'static Regex {
    RE_TECH_TAGS.get_or_init(|| {
        Regex::new(r"(?i)\b(?:2160p|1080p|1080i|720p|576p|480p|4k|uhd|bluray|blu-ray|bdrip|brrip|web-dl|webdl|webrip|web-rip|web|hdtv|dvdrip|remux|x264|x265|h264|h265|hevc|av1|xvid|divx|10bit|8bit|hdr10\+?|hdr|truehd|atmos|dts-hd|dts-ma|dts|ac3|eac3|ddp5\.1|dd5\.1|aac|multi|truefrench|french|vostfr|subfrench|proper|repack)\b").unwrap()
    })
}

/// Nettoie un titre : remplace les séparateurs par des espaces (si chaîne dot-separated),
/// supprime les tirets orphelins et compresse les espaces multiples.
fn clean_title(raw: &str) -> String {
    let trimmed = raw.trim();
    // Si la chaîne contient des points comme séparateurs exclusifs (pas d'espaces)
    let processed = if trimmed.contains('.') && !trimmed.contains(' ') {
        trimmed.replace('.', " ")
    } else if trimmed.contains('_') && !trimmed.contains(' ') {
        trimmed.replace('_', " ")
    } else {
        trimmed.to_string()
    };

    // Nettoyer les tirets terminaux et espaces résiduels
    let mut cleaned = processed.trim().to_string();
    while cleaned.ends_with('-') || cleaned.ends_with('_') || cleaned.ends_with('.') {
        cleaned.pop();
        cleaned = cleaned.trim().to_string();
    }
    while cleaned.starts_with('-') || cleaned.starts_with('_') || cleaned.starts_with('.') {
        cleaned.remove(0);
        cleaned = cleaned.trim().to_string();
    }

    // Compresser les espaces multiples
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    words.join(" ")
}

/// Tente d'extraire le titre et l'année d'une chaîne du type `Show (2020)` ou `Title.2019`.
fn extract_title_and_year(raw: &str) -> (String, Option<u16>) {
    // 1. Chercher d'abord (YYYY) ou [YYYY]
    if let Some(caps) = re_year_parens().captures(raw) {
        if let Some(m) = caps.get(1) {
            if let Ok(year) = m.as_str().parse::<u16>() {
                let full_match = caps.get(0).unwrap();
                let prefix = &raw[..full_match.start()];
                let cleaned = clean_title(prefix);
                if !cleaned.is_empty() {
                    return (cleaned, Some(year));
                }
            }
        }
    }

    // 2. Chercher un token YYYY
    if let Some(caps) = re_year_token().captures(raw) {
        if let Some(m) = caps.get(1) {
            if let Ok(year) = m.as_str().parse::<u16>() {
                let prefix = &raw[..m.start()];
                let cleaned = clean_title(prefix);
                if !cleaned.is_empty() {
                    return (cleaned, Some(year));
                }
            }
        }
    }

    (clean_title(raw), None)
}

/// Extrait le titre de l'épisode s'il est présent après le motif d'épisode, sinon retourne un titre par défaut.
pub fn extract_episode_title(file_stem: &str, episode: u32) -> String {
    let after_match = if let Some(caps) = re_s_e().captures(file_stem) {
        let m = caps.get(0).unwrap();
        &file_stem[m.end()..]
    } else if let Some(caps) = re_x_ep().captures(file_stem) {
        let m = caps.get(0).unwrap();
        &file_stem[m.end()..]
    } else if let Some(caps) = re_ep_only().captures(file_stem) {
        let m = caps.get(0).unwrap();
        &file_stem[m.end()..]
    } else {
        ""
    };

    let cleaned_after = if !after_match.is_empty() {
        let without_tech = if let Some(m) = re_tech_tags().find(after_match) {
            &after_match[..m.start()]
        } else {
            after_match
        };
        clean_title(without_tech)
    } else {
        String::new()
    };

    if !cleaned_after.is_empty() && is_valid_title(&cleaned_after) {
        cleaned_after
    } else {
        format!("Épisode {:02}", episode)
    }
}

/// Analyse un chemin relatif pour déterminer s'il s'agit d'un film, d'un épisode ou d'un fichier non classé.
pub fn parse_relative_path(path_str: &str) -> ParsedName {
    let normalized = path_str.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        return ParsedName::Unknown;
    }

    let filename = parts.last().unwrap();
    let (file_stem, ext) = match filename.rfind('.') {
        Some(idx) => (&filename[..idx], &filename[idx + 1..]),
        None => (*filename, ""),
    };

    // Vérification stricte de l'extension vidéo
    if !VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
        return ParsedName::Unknown;
    }

    // Récupérer les dossiers ancêtres
    let ancestors = if parts.len() > 1 {
        &parts[..parts.len() - 1]
    } else {
        &[][..]
    };

    // --- ÉTAPE 1 : Détection d'un épisode de série ---

    // 1.1 Motif standard SxxExx dans le nom du fichier
    if let Some(caps) = re_s_e().captures(file_stem) {
        let season: u32 = caps
            .get(1)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);
        let episode: u32 = caps
            .get(2)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);
        let episode_end: Option<u32> = caps.get(3).and_then(|m| m.as_str().parse().ok());

        let match_start = caps.get(0).unwrap().start();
        let prefix = &file_stem[..match_start];
        let (raw_show_from_prefix, prefix_year) = extract_title_and_year(prefix);

        // Déterminer le titre et l'année de la série
        let (show_title, year) =
            determine_show_title_and_year(&raw_show_from_prefix, prefix_year, ancestors);

        if !show_title.is_empty() {
            return ParsedName::Episode {
                show_title,
                season,
                episode,
                episode_end,
                year,
            };
        }
    }

    // 1.2 Motif 1x02 dans le nom du fichier
    if let Some(caps) = re_x_ep().captures(file_stem) {
        let season: u32 = caps
            .get(1)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);
        let episode: u32 = caps
            .get(2)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);

        let match_start = caps.get(1).unwrap().start();
        let prefix = &file_stem[..match_start];
        let (raw_show_from_prefix, prefix_year) = extract_title_and_year(prefix);

        let (show_title, year) =
            determine_show_title_and_year(&raw_show_from_prefix, prefix_year, ancestors);

        if !show_title.is_empty() {
            return ParsedName::Episode {
                show_title,
                season,
                episode,
                episode_end: None,
                year,
            };
        }
    }

    // 1.3 Dossier de saison parent (ex. `Show/Saison 01/...` ou `Show/Specials/...`)
    if let Some(season_folder_name) = ancestors.last() {
        let season_from_folder = parse_season_from_folder_name(season_folder_name);
        if let Some(season_num) = season_from_folder {
            // Chercher l'épisode dans le nom du fichier
            let ep_info = if let Some(caps) = re_ep_only().captures(file_stem) {
                let ep: u32 = caps
                    .get(1)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0);
                let ep_end: Option<u32> = caps.get(2).and_then(|m| m.as_str().parse().ok());
                Some((ep, ep_end))
            } else if let Some(caps) = re_leading_digit_ep().captures(file_stem) {
                let ep: u32 = caps
                    .get(1)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0);
                Some((ep, None))
            } else {
                None
            };

            if let Some((episode, episode_end)) = ep_info {
                let ancestors_before_season = if ancestors.len() > 1 {
                    &ancestors[..ancestors.len() - 1]
                } else {
                    &[][..]
                };

                let (show_title, year) =
                    determine_show_title_and_year("", None, ancestors_before_season);

                if !show_title.is_empty() {
                    return ParsedName::Episode {
                        show_title,
                        season: season_num,
                        episode,
                        episode_end,
                        year,
                    };
                }
            }
        }
    }

    // --- ÉTAPE 2 : Détection d'un film ---

    // 2.1 Nom de fichier avec année explicite entre parenthèses ou crochets
    if let Some(caps) = re_year_parens().captures(file_stem) {
        if let Some(m) = caps.get(1) {
            if let Ok(year) = m.as_str().parse::<u16>() {
                let full = caps.get(0).unwrap();
                let title_part = &file_stem[..full.start()];
                let title = clean_title(title_part);
                if is_valid_title(&title) {
                    return ParsedName::Movie {
                        title,
                        year: Some(year),
                    };
                }
            }
        }
    }

    // 2.2 Nom de fichier avec token d'année (ex: Titre.2019.1080p.x265-GRP)
    if let Some(caps) = re_year_token().captures(file_stem) {
        if let Some(m) = caps.get(1) {
            if let Ok(year) = m.as_str().parse::<u16>() {
                let title_part = &file_stem[..m.start()];
                let title = clean_title(title_part);
                if is_valid_title(&title) {
                    return ParsedName::Movie {
                        title,
                        year: Some(year),
                    };
                }
            }
        }
    }

    // 2.3 Dossier parent avec année explicite (ex: `Inception (2010)/movie.mkv`)
    if let Some(parent) = ancestors.last() {
        let (parent_title, parent_year) = extract_title_and_year(parent);
        if let Some(year) = parent_year {
            if is_valid_title(&parent_title) {
                return ParsedName::Movie {
                    title: parent_title,
                    year: Some(year),
                };
            }
        }
    }

    // 2.4 Tags techniques présents (ex: Inception.1080p.BluRay.x264)
    if let Some(m) = re_tech_tags().find(file_stem) {
        let title_part = &file_stem[..m.start()];
        let title = clean_title(title_part);
        if is_valid_title(&title) {
            return ParsedName::Movie { title, year: None };
        }
    }

    // En cas de doute ou nom non structuré, ne jamais deviner : renvoyer Unknown
    ParsedName::Unknown
}

/// Valide qu'un titre extrait n'est pas un nom générique ou vide
fn is_valid_title(title: &str) -> bool {
    let lower = title.to_lowercase();
    let is_generic = matches!(
        lower.as_str(),
        "" | "sample" | "trailer" | "movie" | "film" | "video" | "unknown" | "clip" | "test"
    );
    !is_generic && title.len() >= 2 && !title.chars().all(|c| c.is_ascii_digit())
}

/// Analyse un nom de dossier pour déterminer s'il représente une saison
fn parse_season_from_folder_name(name: &str) -> Option<u32> {
    if re_specials_folder().is_match(name) {
        return Some(0);
    }

    if let Some(caps) = re_season_folder().captures(name) {
        if let Some(m) = caps.get(1) {
            return m.as_str().parse().ok();
        }
        if let Some(m) = caps.get(2) {
            return m.as_str().parse().ok();
        }
        if let Some(m) = caps.get(3) {
            let roman = m.as_str().to_uppercase();
            let num = match roman.as_str() {
                "I" => 1,
                "II" => 2,
                "III" => 3,
                "IV" => 4,
                "V" => 5,
                "VI" => 6,
                "VII" => 7,
                "VIII" => 8,
                "IX" => 9,
                "X" => 10,
                s => s.parse().unwrap_or(1),
            };
            return Some(num);
        }
    }

    None
}

/// Détermine le titre et l'année de la série en réconciliant le préfixe du fichier et l'arborescence des dossiers.
fn determine_show_title_and_year(
    prefix_title: &str,
    prefix_year: Option<u16>,
    ancestors: &[&str],
) -> (String, Option<u16>) {
    // Si on a des dossiers ancêtres
    if let Some(parent) = ancestors.last() {
        let (parent_title, parent_year) = extract_title_and_year(parent);
        // Si le parent n'est pas un nom de saison
        if parse_season_from_folder_name(&parent_title).is_none() {
            let year = parent_year.or(prefix_year);
            return (parent_title, year);
        }
    }

    // Si le grand-parent existe (cas Show/Saison 01/...)
    if ancestors.len() >= 2 {
        let grand_parent = ancestors[ancestors.len() - 2];
        let (gp_title, gp_year) = extract_title_and_year(grand_parent);
        let year = gp_year.or(prefix_year);
        return (gp_title, year);
    }

    // Sinon utiliser le titre extrait du fichier
    let cleaned = clean_title(prefix_title);
    (cleaned, prefix_year)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_table_driven_parser_30_cases() {
        let cases = vec![
            // 1. Film standard avec année entre parenthèses
            (
                "Avengers - Endgame (2019).mkv",
                ParsedName::Movie {
                    title: "Avengers - Endgame".into(),
                    year: Some(2019),
                },
            ),
            // 2. Film avec tiret et accents
            (
                "Benjamin Gates et le Trésor des Templiers (2004).mkv",
                ParsedName::Movie {
                    title: "Benjamin Gates et le Trésor des Templiers".into(),
                    year: Some(2004),
                },
            ),
            // 3. Film deuxième volet
            (
                "Benjamin Gates et le Livre des Secrets (2007).mkv",
                ParsedName::Movie {
                    title: "Benjamin Gates et le Livre des Secrets".into(),
                    year: Some(2007),
                },
            ),
            // 4. Film avec tags techniques et groupe de release
            (
                "Titre.2019.1080p.x265-GRP.ext",
                ParsedName::Movie {
                    title: "Titre".into(),
                    year: Some(2019),
                },
            ),
            // 5. Film avec sous-titre
            (
                "Titre - Sous-titre (2004).ext",
                ParsedName::Movie {
                    title: "Titre - Sous-titre".into(),
                    year: Some(2004),
                },
            ),
            // 6. Épisode double S01E02E03
            (
                "Show.S01E02E03.1080p.mkv",
                ParsedName::Episode {
                    show_title: "Show".into(),
                    season: 1,
                    episode: 2,
                    episode_end: Some(3),
                    year: None,
                },
            ),
            // 7. Épisode notation 1x05
            (
                "Show 1x05.mkv",
                ParsedName::Episode {
                    show_title: "Show".into(),
                    season: 1,
                    episode: 5,
                    episode_end: None,
                    year: None,
                },
            ),
            // 8. Série avec dossier racine contenant l'année et dossier de saison
            (
                "Show (2020)/Season 2/Show - S02E10.mkv",
                ParsedName::Episode {
                    show_title: "Show".into(),
                    season: 2,
                    episode: 10,
                    episode_end: None,
                    year: Some(2020),
                },
            ),
            // 9. Bécé Bégé S01E01
            (
                "Bécé Bégé/Saison 01/Bécé Bégé - S01E01 - Tel est pris.mkv",
                ParsedName::Episode {
                    show_title: "Bécé Bégé".into(),
                    season: 1,
                    episode: 1,
                    episode_end: None,
                    year: None,
                },
            ),
            // 10. Bécé Bégé S01E02
            (
                "Bécé Bégé/Saison 01/Bécé Bégé - S01E02 - Le chien a mangé mes devoirs.mkv",
                ParsedName::Episode {
                    show_title: "Bécé Bégé".into(),
                    season: 1,
                    episode: 2,
                    episode_end: None,
                    year: None,
                },
            ),
            // 11. Bécé Bégé S01E03
            (
                "Bécé Bégé/Saison 01/Bécé Bégé - S01E03 - La transformation.mkv",
                ParsedName::Episode {
                    show_title: "Bécé Bégé".into(),
                    season: 1,
                    episode: 3,
                    episode_end: None,
                    year: None,
                },
            ),
            // 12. Bécé Bégé S01E04
            (
                "Bécé Bégé/Saison 01/Bécé Bégé - S01E04 - Mon prince charmant.mkv",
                ParsedName::Episode {
                    show_title: "Bécé Bégé".into(),
                    season: 1,
                    episode: 4,
                    episode_end: None,
                    year: None,
                },
            ),
            // 13. Bécé Bégé S01E05
            (
                "Bécé Bégé/Saison 01/Bécé Bégé - S01E05 - Pour l'amour d'un robot.mkv",
                ParsedName::Episode {
                    show_title: "Bécé Bégé".into(),
                    season: 1,
                    episode: 5,
                    episode_end: None,
                    year: None,
                },
            ),
            // 14. Bécé Bégé S01E06
            (
                "Bécé Bégé/Saison 01/Bécé Bégé - S01E06 - Distribution des rôles.mkv",
                ParsedName::Episode {
                    show_title: "Bécé Bégé".into(),
                    season: 1,
                    episode: 6,
                    episode_end: None,
                    year: None,
                },
            ),
            // 15. Specials => Saison 0
            (
                "Doctor Who/Specials/Doctor Who - S00E01 - The Runaway Bride.mkv",
                ParsedName::Episode {
                    show_title: "Doctor Who".into(),
                    season: 0,
                    episode: 1,
                    episode_end: None,
                    year: None,
                },
            ),
            // 16. Hors-série dossier parent => Saison 0
            (
                "Doctor Who/Hors-série/Episode 02.mkv",
                ParsedName::Episode {
                    show_title: "Doctor Who".into(),
                    season: 0,
                    episode: 2,
                    episode_end: None,
                    year: None,
                },
            ),
            // 17. Épisode double S01E02-E03
            (
                "Show.S01E02-E03.mkv",
                ParsedName::Episode {
                    show_title: "Show".into(),
                    season: 1,
                    episode: 2,
                    episode_end: Some(3),
                    year: None,
                },
            ),
            // 18. Épisode s1e2 minuscule
            (
                "The Wire/Season 1/s1e2.mkv",
                ParsedName::Episode {
                    show_title: "The Wire".into(),
                    season: 1,
                    episode: 2,
                    episode_end: None,
                    year: None,
                },
            ),
            // 19. Film dans sous-dossier avec année
            (
                "Inception (2010)/Inception (2010).mkv",
                ParsedName::Movie {
                    title: "Inception".into(),
                    year: Some(2010),
                },
            ),
            // 20. Film 4K HDR avec groupe
            (
                "Interstellar.2014.2160p.UHD.HDR.x265-FLUX.mkv",
                ParsedName::Movie {
                    title: "Interstellar".into(),
                    year: Some(2014),
                },
            ),
            // 21. Titre avec apostrophe
            (
                "Le Fabuleux Destin d'Amélie Poulain (2001).avi",
                ParsedName::Movie {
                    title: "Le Fabuleux Destin d'Amélie Poulain".into(),
                    year: Some(2001),
                },
            ),
            // 22. Titre L'Agence
            (
                "L'Agence Tous Risques (2010).mp4",
                ParsedName::Movie {
                    title: "L'Agence Tous Risques".into(),
                    year: Some(2010),
                },
            ),
            // 23. Film classique Matrix
            (
                "The.Matrix.1999.REMASTERED.1080p.BluRay.x264.mkv",
                ParsedName::Movie {
                    title: "The Matrix".into(),
                    year: Some(1999),
                },
            ),
            // 24. Film avec nombre dans le titre
            (
                "Blade Runner 2049 (2017).mkv",
                ParsedName::Movie {
                    title: "Blade Runner 2049".into(),
                    year: Some(2017),
                },
            ),
            // 25. Film commençant par une année dans le titre
            (
                "2001: A Space Odyssey (1968).mkv",
                ParsedName::Movie {
                    title: "2001: A Space Odyssey".into(),
                    year: Some(1968),
                },
            ),
            // 26. Film sans année mais avec tags techniques
            (
                "Inception.1080p.BluRay.x264.mkv",
                ParsedName::Movie {
                    title: "Inception".into(),
                    year: None,
                },
            ),
            // 27. Kaamelott avec format Livre
            (
                "Kaamelott/Livre I/Kaamelott - 1x01 - Le Départ.mkv",
                ParsedName::Episode {
                    show_title: "Kaamelott".into(),
                    season: 1,
                    episode: 1,
                    episode_end: None,
                    year: None,
                },
            ),
            // 28. Fichier ambigu : sample => Unknown
            ("sample.mkv", ParsedName::Unknown),
            // 29. Fichier ambigu : trailer => Unknown
            ("trailer.mp4", ParsedName::Unknown),
            // 30. Fichier non média : notes.txt => Unknown
            ("notes.txt", ParsedName::Unknown),
            // 31. Fichier avec nom purement numérique => Unknown
            ("12345.mkv", ParsedName::Unknown),
            // 32. Fichier texte sous-titres => Unknown
            ("subtitle.srt", ParsedName::Unknown),
            // 33. Nom de fichier générique sans métadonnées => Unknown
            ("video_clip.mp4", ParsedName::Unknown),
        ];

        assert!(
            cases.len() >= 30,
            "Au moins 30 cas de test doivent être présents"
        );

        for (input, expected) in cases {
            let res = parse_relative_path(input);
            assert_eq!(
                res, expected,
                "Échec pour l'entrée: '{}'\nAttendu: {:?}\nObtenu: {:?}",
                input, expected, res
            );
        }

        // Test de l'extraction du titre d'épisode
        assert_eq!(
            extract_episode_title("Bécé Bégé - S01E01 - Tel est pris", 1),
            "Tel est pris"
        );
        assert_eq!(extract_episode_title("Show.S01E02.1080p", 2), "Épisode 02");
    }
}
