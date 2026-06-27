use std::path::{Path, PathBuf};
use std::process::Stdio;

use bytes::Bytes;
use image::ImageFormat;
use tokio::process::Command;

use crate::config::AppConfig;
use crate::error::{AppError, AppResult};

pub fn is_pdf_content_type(content_type: &str) -> bool {
    matches!(
        content_type.trim().to_ascii_lowercase().as_str(),
        "application/pdf" | "application/x-pdf"
    )
}

pub fn is_media_content_type(content_type: &str) -> bool {
    content_type.starts_with("video/") || content_type.starts_with("audio/")
}

pub async fn generate_pdf_first_page_jpeg(data: &Bytes, config: &AppConfig) -> AppResult<Bytes> {
    let dir = temp_dir()?;
    let input = dir.path().join("input.pdf");
    tokio::fs::write(&input, data)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let jpeg_path = match try_pdftoppm(&input, dir.path(), config).await {
        Ok(path) => path,
        Err(e1) => match try_mutool(&input, dir.path(), config).await {
            Ok(path) => path,
            Err(e2) => try_ghostscript(&input, dir.path(), config).await.map_err(|e3| {
                tracing::warn!(
                    pdftoppm = %e1,
                    mutool = %e2,
                    ghostscript = %e3,
                    "PDF auto-preview failed"
                );
                AppError::BadRequest(
                    "Could not generate a preview from this PDF. Upload an optional preview image, or try a different file.".into(),
                )
            })?,
        },
    };

    let raw = tokio::fs::read(&jpeg_path)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    thumbnail_jpeg(Bytes::from(raw), 400)
}

/// Extract a page-limited PDF sample (default: page 1 only) for detail-page embeds.
pub async fn generate_pdf_page_sample(data: &Bytes, config: &AppConfig) -> AppResult<Bytes> {
    let dir = temp_dir()?;
    let input = dir.path().join("input.pdf");
    let output = dir.path().join("sample.pdf");
    tokio::fs::write(&input, data)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let status = Command::new(&config.gs_bin)
        .args([
            "-dSAFER",
            "-dBATCH",
            "-dNOPAUSE",
            "-sDEVICE=pdfwrite",
            "-dFirstPage=1",
            "-dLastPage=1",
            &format!("-sOutputFile={}", output.display()),
        ])
        .arg(&input)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| AppError::BadRequest(format!("ghostscript not runnable: {e}")))?;

    if !status.status.success() {
        return Err(AppError::BadRequest(
            String::from_utf8_lossy(&status.stderr).trim().to_string(),
        ));
    }

    let sample = tokio::fs::read(&output)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    if sample.is_empty() {
        return Err(AppError::BadRequest(
            "PDF sample extraction produced empty output".into(),
        ));
    }
    Ok(Bytes::from(sample))
}

pub async fn generate_media_clip(
    data: &Bytes,
    content_type: &str,
    config: &AppConfig,
) -> AppResult<(Bytes, String)> {
    let dir = temp_dir()?;
    let in_ext = media_input_extension(content_type);
    let input = dir.path().join(format!("input.{in_ext}"));
    tokio::fs::write(&input, data)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let out_ext = media_output_extension(content_type);
    let output = dir.path().join(format!("clip.{out_ext}"));
    let secs = config.preview_media_seconds.to_string();
    let audio_only = content_type.starts_with("audio/");

    let copy_ok = run_ffmpeg(&config.ffmpeg_bin, &input, &output, &secs, true, audio_only)
        .await
        .is_ok();

    if !copy_ok {
        run_ffmpeg(
            &config.ffmpeg_bin,
            &input,
            &output,
            &secs,
            false,
            audio_only,
        )
        .await
        .map_err(|e| {
            AppError::BadRequest(format!(
                "media preview clip failed (install {bin} and ensure codecs are available): {e}",
                bin = config.ffmpeg_bin
            ))
        })?;
    }

    let clip = tokio::fs::read(&output)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    if clip.len() as u64 > config.max_preview_bytes {
        return Err(AppError::BadRequest(format!(
            "generated preview clip is {} bytes (max preview {} bytes) — upload a shorter preview file",
            clip.len(),
            config.max_preview_bytes
        )));
    }

    Ok((
        Bytes::from(clip),
        media_output_content_type(content_type, out_ext),
    ))
}

fn temp_dir() -> AppResult<tempfile::TempDir> {
    tempfile::tempdir().map_err(|e| AppError::Internal(e.into()))
}

async fn try_pdftoppm(input: &Path, dir: &Path, config: &AppConfig) -> AppResult<PathBuf> {
    let prefix = dir.join("page");
    let status = Command::new(&config.pdftoppm_bin)
        .args([
            "-jpeg",
            "-singlefile",
            "-f",
            "1",
            "-l",
            "1",
            "-scale-to",
            "800",
        ])
        .arg(input)
        .arg(&prefix)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| AppError::BadRequest(format!("pdftoppm not runnable: {e}")))?;

    if !status.status.success() {
        return Err(AppError::BadRequest(
            String::from_utf8_lossy(&status.stderr).trim().to_string(),
        ));
    }

    find_first_jpeg(dir, "page")
}

async fn try_mutool(input: &Path, dir: &Path, config: &AppConfig) -> AppResult<PathBuf> {
    let output = dir.join("page.jpg");
    let status = Command::new(&config.mutool_bin)
        .args(["draw", "-o"])
        .arg(&output)
        .args(["-F", "jpeg", "-w", "800"])
        .arg(input)
        .arg("1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| AppError::BadRequest(format!("mutool not runnable: {e}")))?;

    if !status.status.success() {
        return Err(AppError::BadRequest(
            String::from_utf8_lossy(&status.stderr).trim().to_string(),
        ));
    }
    Ok(output)
}

async fn try_ghostscript(input: &Path, dir: &Path, config: &AppConfig) -> AppResult<PathBuf> {
    let output = dir.join("page.jpg");
    let status = Command::new(&config.gs_bin)
        .args([
            "-dSAFER",
            "-dBATCH",
            "-dNOPAUSE",
            "-sDEVICE=jpeg",
            "-r150",
            "-dFirstPage=1",
            "-dLastPage=1",
            &format!("-sOutputFile={}", output.display()),
        ])
        .arg(input)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| AppError::BadRequest(format!("ghostscript not runnable: {e}")))?;

    if !status.status.success() {
        return Err(AppError::BadRequest(
            String::from_utf8_lossy(&status.stderr).trim().to_string(),
        ));
    }
    Ok(output)
}

fn find_first_jpeg(dir: &Path, prefix: &str) -> AppResult<PathBuf> {
    for candidate in [
        dir.join(format!("{prefix}.jpg")),
        dir.join(format!("{prefix}.jpeg")),
        dir.join(format!("{prefix}-1.jpg")),
        dir.join(format!("{prefix}-01.jpg")),
        dir.join(format!("{prefix}-001.jpg")),
    ] {
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg") {
                return Ok(path);
            }
        }
    }
    Err(AppError::BadRequest(
        "PDF preview tool produced no JPEG output".into(),
    ))
}

async fn run_ffmpeg(
    bin: &str,
    input: &Path,
    output: &Path,
    secs: &str,
    stream_copy: bool,
    audio_only: bool,
) -> AppResult<()> {
    let mut cmd = Command::new(bin);
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(["-t", secs]);

    if stream_copy {
        cmd.arg("-c").arg("copy");
    } else if audio_only {
        cmd.args(["-c:a", "aac", "-b:a", "128k"]);
    } else {
        cmd.args([
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "28",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-movflags",
            "+faststart",
        ]);
    }

    let status = cmd
        .arg(output)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| AppError::BadRequest(format!("ffmpeg not runnable: {e}")))?;

    if status.status.success() {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            String::from_utf8_lossy(&status.stderr).trim().to_string(),
        ))
    }
}

fn thumbnail_jpeg(data: Bytes, max_px: u32) -> AppResult<Bytes> {
    let img = image::load_from_memory(&data)
        .map_err(|e| AppError::BadRequest(format!("PDF page is not a valid image: {e}")))?;
    let thumb = img.thumbnail(max_px, max_px);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut buf, ImageFormat::Jpeg)
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(Bytes::from(buf.into_inner()))
}

fn media_input_extension(content_type: &str) -> &'static str {
    match content_type {
        "video/webm" => "webm",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/ogg" => "ogg",
        "audio/mp4" | "audio/aac" | "audio/x-m4a" => "m4a",
        ct if ct.starts_with("video/") => "mp4",
        ct if ct.starts_with("audio/") => "mp3",
        _ => "bin",
    }
}

fn media_output_extension(content_type: &str) -> &'static str {
    match content_type {
        "video/webm" => "webm",
        "video/mp4" | "video/quicktime" => "mp4",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" => "m4a",
        "audio/flac" | "audio/x-flac" => "m4a",
        "audio/ogg" => "ogg",
        "audio/mp4" | "audio/aac" | "audio/x-m4a" => "m4a",
        ct if ct.starts_with("video/") => "mp4",
        ct if ct.starts_with("audio/") => "m4a",
        _ => "bin",
    }
}

fn media_output_content_type(content_type: &str, ext: &str) -> String {
    match ext {
        "webm" => "video/webm".into(),
        "mp4" => "video/mp4".into(),
        "mp3" => "audio/mpeg".into(),
        "ogg" => "audio/ogg".into(),
        "m4a" => "audio/mp4".into(),
        _ if content_type.starts_with("video/") => "video/mp4".into(),
        _ if content_type.starts_with("audio/") => "audio/mp4".into(),
        _ => content_type.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_mime_detection() {
        assert!(is_pdf_content_type("application/pdf"));
        assert!(!is_pdf_content_type("application/zip"));
    }

    #[test]
    fn media_extension_mapping() {
        assert_eq!(media_output_extension("video/mp4"), "mp4");
        assert_eq!(media_output_extension("audio/mpeg"), "mp3");
    }
}
