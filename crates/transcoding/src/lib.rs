//! Transcodage vidéo à la volée via ffmpeg avec détection automatique de l'accélérateur GPU.
//!
//! Ordre de préférence : QSV (Intel) → NVENC (NVIDIA) → VAAPI (Linux GPU) →
//!                       VideoToolbox (macOS) → logiciel (libx264).

use std::path::Path;
use std::process::Stdio;
use tokio::process::{Child, Command};

/// Accélérateur matériel disponible sur cet hôte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwAccel {
    Qsv,
    Nvenc,
    Vaapi,
    VideoToolbox,
    Software,
}

impl HwAccel {
    /// Codec ffmpeg pour l'encodeur H.264 correspondant à cet accélérateur.
    pub fn encoder(&self) -> &'static str {
        match self {
            Self::Qsv => "h264_qsv",
            Self::Nvenc => "h264_nvenc",
            Self::Vaapi => "h264_vaapi",
            Self::VideoToolbox => "h264_videotoolbox",
            Self::Software => "libx264",
        }
    }

    /// Flags ffmpeg supplémentaires requis par cet accélérateur (entrée, pré-filtres).
    fn extra_input_args(&self) -> &'static [&'static str] {
        match self {
            Self::Qsv => &["-hwaccel", "qsv", "-hwaccel_output_format", "qsv"],
            Self::Nvenc => &["-hwaccel", "cuda"],
            Self::Vaapi => &["-hwaccel", "vaapi", "-hwaccel_device", "/dev/dri/renderD128",
                            "-hwaccel_output_format", "vaapi"],
            Self::VideoToolbox => &["-hwaccel", "videotoolbox"],
            Self::Software => &[],
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Qsv => "qsv",
            Self::Nvenc => "nvenc",
            Self::Vaapi => "vaapi",
            Self::VideoToolbox => "videotoolbox",
            Self::Software => "software",
        }
    }
}

/// Détecte le premier accélérateur matériel disponible en testant ffmpeg.
/// Retourne toujours au moins `HwAccel::Software` (libx264 est toujours présent).
pub async fn detect_hw_accel() -> HwAccel {
    // Tente chaque accélérateur dans l'ordre de préférence.
    for (accel, encoder) in [
        (HwAccel::Qsv, "h264_qsv"),
        (HwAccel::Nvenc, "h264_nvenc"),
        (HwAccel::Vaapi, "h264_vaapi"),
        (HwAccel::VideoToolbox, "h264_videotoolbox"),
    ] {
        if probe_encoder(encoder).await {
            tracing::info!(accel = accel.as_str(), "Accélérateur GPU détecté pour le transcodage");
            return accel;
        }
    }

    tracing::info!("Aucun accélérateur GPU disponible, transcodage logiciel (libx264)");
    HwAccel::Software
}

/// Vérifie qu'un encodeur ffmpeg est opérationnel en lançant un encode nul d'une seconde.
async fn probe_encoder(encoder: &str) -> bool {
    let result = Command::new("ffmpeg")
        .args([
            "-f", "lavfi",
            "-i", "color=black:size=64x64:rate=1",
            "-t", "0.1",
            "-c:v", encoder,
            "-f", "null",
            "-",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    matches!(result, Ok(s) if s.success())
}

/// Paramètres d'une session de transcodage.
pub struct TranscodeRequest<'a> {
    pub input: &'a Path,
    /// Position de départ en secondes (pour la reprise depuis un horodatage).
    pub start_secs: f64,
    pub accel: HwAccel,
    /// Hauteur cible en pixels (ex: 1080, 720, 480). La largeur est calculée automatiquement.
    pub target_height: u32,
}

/// Lance ffmpeg en sous-processus et retourne le handle.
/// Le stdout du processus est un flux fMP4 (fragmented MP4) lisible directement par un
/// navigateur via `<video>` ou par un client HTTP Range.
/// Tuer le `Child` retourné annule le transcodage proprement.
pub fn spawn_transcode(req: &TranscodeRequest<'_>) -> std::io::Result<Child> {
    let encoder = req.accel.encoder();
    let start = format!("{:.3}", req.start_secs);
    // scale2ref : redimensionne en conservant le ratio, hauteur cible divisible par 2.
    let scale_filter = format!(
        "scale=-2:{height}",
        height = req.target_height
    );

    let mut cmd = Command::new("ffmpeg");

    // Seek rapide côté input (avant décodage = seek par keyframe).
    cmd.args(["-ss", &start]);

    // Flags d'accélération matérielle.
    for arg in req.accel.extra_input_args() {
        cmd.arg(arg);
    }

    cmd.args(["-i", req.input.to_str().unwrap_or("")]);

    // Filtre de redimensionnement pour VAAPI (hwupload requis).
    if req.accel == HwAccel::Vaapi {
        cmd.args(["-vf", &format!("hwupload,scale_vaapi=-2:{}", req.target_height)]);
    } else {
        cmd.args(["-vf", &scale_filter]);
    }

    cmd.args([
        // Vidéo
        "-c:v", encoder,
        "-preset", if req.accel == HwAccel::Software { "fast" } else { "p4" },
        "-crf", "23",
        // Audio : réencoder en AAC 192 kbps pour compatibilité universelle.
        "-c:a", "aac",
        "-b:a", "192k",
        // Sortie : fragmented MP4 streamé sur stdout.
        "-movflags", "frag_keyframe+empty_moov+faststart",
        "-f", "mp4",
        "pipe:1",
    ]);

    cmd.stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_names_are_non_empty() {
        for accel in [
            HwAccel::Qsv,
            HwAccel::Nvenc,
            HwAccel::Vaapi,
            HwAccel::VideoToolbox,
            HwAccel::Software,
        ] {
            assert!(!accel.encoder().is_empty());
            assert!(!accel.as_str().is_empty());
        }
    }

    #[tokio::test]
    async fn detect_always_returns_a_value() {
        // Ce test ne vérifie pas l'accélérateur exact (dépend du matériel),
        // mais garantit que detect_hw_accel() ne panique jamais.
        let _accel = detect_hw_accel().await;
    }
}
