//! Crate `metadata` pour Norylum Server.
//! Fournit le parsing des noms de fichiers multimédias et l'enrichissement documentaire.

pub mod parser;

pub use parser::{extract_episode_title, parse_relative_path, ParsedName};
