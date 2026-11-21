//! Looking at a file that isn't text: a picture, a video, a sound.
//!
//! Pictures are decoded here and handed to the renderer as a texture.
//! Video and audio are not: playing them would mean a media stack Qell
//! has no business carrying, so it asks `ffprobe` what the file is and
//! `ffmpeg` for a frame of it, the way the rest of Qell leans on the
//! tools you already have. Neither is installed for you; when they
//! aren't there the panel says so and what to type.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Beyond this a picture is reported rather than decoded: a hundred
/// megapixels would be a gigabyte of texture.
const MAX_PIXELS: u64 = 80_000_000;
/// How long `ffprobe` or `ffmpeg` may take before it is given up on.
const TIMEOUT: Duration = Duration::from_secs(20);

/// What a file is, as far as previewing goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    Image,
    Video,
    Audio,
}

impl Media {
    /// What this file is by its extension, or `None` for everything
    /// Qell should open as text.
    pub fn of(path: &Path) -> Option<Media> {
        let ext = path.extension()?.to_string_lossy().to_lowercase();
        Some(match ext.as_str() {
            // SVG is text, and more useful as text.
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff" | "avif" | "heic" => Media::Image,
            "mp4" | "mov" | "m4v" | "mkv" | "webm" | "avi" | "mpg" | "mpeg" | "wmv" | "flv" => Media::Video,
            "mp3" | "wav" | "flac" | "ogg" | "oga" | "m4a" | "aac" | "aiff" | "aif" | "opus" | "wma" => Media::Audio,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Media::Image => "image",
            Media::Video => "video",
            Media::Audio => "audio",
        }
    }
}

/// A decoded picture, ready to become a texture.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    /// Straight RGBA, one byte a channel.
    pub rgba: Vec<u8>,
}

/// Decode a picture from disk.
pub fn read_image(path: &Path) -> Result<Picture, String> {
    let reader = image::ImageReader::open(path).map_err(|e| format!("{e}"))?.with_guessed_format().map_err(|e| format!("{e}"))?;
    if let Some((w, h)) = reader.into_dimensions().ok().filter(|(w, h)| u64::from(*w) * u64::from(*h) > MAX_PIXELS) {
        return Err(format!("{w}×{h} is too large to show ({} megapixels).", u64::from(w) * u64::from(h) / 1_000_000));
    }
    let image = image::ImageReader::open(path)
        .map_err(|e| format!("{e}"))?
        .with_guessed_format()
        .map_err(|e| format!("{e}"))?
        .decode()
        .map_err(|e| tidy(&e.to_string()))?;
    let rgba = image.to_rgba8();
    Ok(Picture { width: rgba.width() as usize, height: rgba.height() as usize, rgba: rgba.into_raw() })
}

/// Decoder errors are written for programmers; this keeps the sentence.
fn tidy(message: &str) -> String {
    let message = message.trim();
    let first = message.split(" at ").next().unwrap_or(message);
    let mut chars = first.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => "The file could not be decoded.".to_string(),
    }
}

/// What `ffprobe` says about a video or a sound file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Info {
    pub seconds: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// `h264`, `aac`…
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    /// Frames a second, when it says.
    pub fps: Option<f64>,
}

impl Info {
    /// `1:23:45` or `4:07`, the way players write it.
    pub fn duration(&self) -> Option<String> {
        let total = self.seconds?.max(0.0) as u64;
        let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
        Some(if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") })
    }
}

/// Read what `ffprobe -show_streams -show_format -of json` printed.
pub fn parse_probe(json: &str) -> Info {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Info::default() };
    let mut info = Info::default();
    let text = |s: &serde_json::Value, key: &str| s[key].as_str().map(str::to_string);
    let number = |s: &serde_json::Value, key: &str| s[key].as_str().and_then(|t| t.parse::<f64>().ok());
    info.seconds = number(&v["format"], "duration");
    for stream in v["streams"].as_array().map(Vec::as_slice).unwrap_or_default() {
        match text(stream, "codec_type").as_deref() {
            Some("video") if info.video_codec.is_none() => {
                info.video_codec = text(stream, "codec_name");
                info.width = stream["width"].as_u64().map(|n| n as u32);
                info.height = stream["height"].as_u64().map(|n| n as u32);
                info.fps = text(stream, "avg_frame_rate").as_deref().and_then(ratio);
                info.seconds = info.seconds.or_else(|| number(stream, "duration"));
            }
            Some("audio") if info.audio_codec.is_none() => {
                info.audio_codec = text(stream, "codec_name");
                info.seconds = info.seconds.or_else(|| number(stream, "duration"));
            }
            _ => {}
        }
    }
    info
}

/// `30000/1001` as 29.97. ffprobe writes `0/0` when it doesn't know.
fn ratio(text: &str) -> Option<f64> {
    let (num, den) = text.split_once('/')?;
    let (num, den): (f64, f64) = (num.parse().ok()?, den.parse().ok()?);
    (den != 0.0 && num != 0.0).then_some(num / den)
}

/// Run a command with a deadline, giving back its stdout as bytes.
fn run(program: &str, args: &[&str]) -> Result<Vec<u8>, Error> {
    let mut child =
        match Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Error::NotInstalled(program.to_string())),
            Err(e) => return Err(Error::Failed(e.to_string())),
        };
    let (tx, rx) = std::sync::mpsc::channel();
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    std::thread::Builder::new()
        .name("ffmpeg".into())
        .spawn(move || {
            use std::io::Read;
            let (mut out, mut err) = (Vec::new(), String::new());
            if let Some(mut s) = stdout {
                let _ = s.read_to_end(&mut out);
            }
            if let Some(mut s) = stderr {
                let _ = s.read_to_string(&mut err);
            }
            let _ = tx.send((out, err));
        })
        .map_err(|e| Error::Failed(e.to_string()))?;
    let Ok((out, err)) = rx.recv_timeout(TIMEOUT) else {
        let _ = child.kill();
        return Err(Error::Failed(format!("{program} took too long")));
    };
    match child.wait() {
        Ok(status) if status.success() => Ok(out),
        Ok(_) => Err(Error::Failed(last_line(&err))),
        Err(e) => Err(Error::Failed(e.to_string())),
    }
}

/// ffmpeg says the useful part last.
fn last_line(stderr: &str) -> String {
    stderr.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("it failed without saying why").to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The program isn't on the PATH; `.0` is which one.
    NotInstalled(String),
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotInstalled(program) => write!(
                f,
                "{program} isn't installed. Qell reads video and sound through it; install ffmpeg the way you install \
                 your other tools (brew install ffmpeg, or your distribution's package), then reopen the file. Qell \
                 never installs anything for you."
            ),
            Error::Failed(message) => write!(f, "{message}"),
        }
    }
}

/// Ask ffprobe what this file is.
pub fn probe(path: &Path) -> Result<Info, Error> {
    let out = run("ffprobe", &["-v", "error", "-show_streams", "-show_format", "-of", "json", &path.to_string_lossy()])?;
    Ok(parse_probe(&String::from_utf8_lossy(&out)))
}

/// One frame, `at` seconds in, as a decoded picture.
pub fn frame(path: &Path, at: f64) -> Result<Picture, Error> {
    let seek = format!("{at:.2}");
    // -ss before -i seeks by keyframe, which is instant even in a long
    // file, and good enough for a look.
    let out = run(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-ss",
            &seek,
            "-i",
            &path.to_string_lossy(),
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-vcodec",
            "png",
            "-",
        ],
    )?;
    if out.is_empty() {
        return Err(Error::Failed("no frame there".into()));
    }
    let image = image::load_from_memory(&out).map_err(|e| Error::Failed(tidy(&e.to_string())))?;
    let rgba = image.to_rgba8();
    Ok(Picture { width: rgba.width() as usize, height: rgba.height() as usize, rgba: rgba.into_raw() })
}

/// Hand the file to whatever the system opens it with.
pub fn open_externally(path: &Path) -> Result<(), String> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    Command::new(program)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{program}: {e}"))
}

/// A size as people read it.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_sorted_by_their_extension() {
        assert_eq!(Media::of(Path::new("a/b/photo.PNG")), Some(Media::Image));
        assert_eq!(Media::of(Path::new("reel.mp4")), Some(Media::Video));
        assert_eq!(Media::of(Path::new("take.wav")), Some(Media::Audio));
        // Text stays text, SVG included: it is more useful as code.
        assert_eq!(Media::of(Path::new("icon.svg")), None);
        assert_eq!(Media::of(Path::new("main.go")), None);
        assert_eq!(Media::of(Path::new("Makefile")), None);
    }

    #[test]
    fn a_probe_gives_the_shape_of_the_file() {
        let json = r#"{
            "streams": [
                {"codec_type": "video", "codec_name": "h264", "width": 1080, "height": 1920, "avg_frame_rate": "30000/1001"},
                {"codec_type": "audio", "codec_name": "aac"}
            ],
            "format": {"duration": "83.4"}
        }"#;
        let info = parse_probe(json);
        assert_eq!(info.video_codec.as_deref(), Some("h264"));
        assert_eq!(info.audio_codec.as_deref(), Some("aac"));
        assert_eq!((info.width, info.height), (Some(1080), Some(1920)));
        assert_eq!(info.fps.map(|f| (f * 100.0).round() / 100.0), Some(29.97));
        assert_eq!(info.duration().as_deref(), Some("1:23"));
    }

    #[test]
    fn a_sound_file_has_no_picture_in_it() {
        let json = r#"{"streams": [{"codec_type": "audio", "codec_name": "flac", "duration": "4000.0"}], "format": {}}"#;
        let info = parse_probe(json);
        assert_eq!(info.video_codec, None);
        assert_eq!(info.duration().as_deref(), Some("1:06:40"));
        // Nothing at all is not a crash.
        assert_eq!(parse_probe("not json"), Info::default());
    }

    #[test]
    fn frame_rates_that_mean_nothing_are_left_out() {
        assert_eq!(ratio("30000/1001").map(|f| f.round()), Some(30.0));
        assert_eq!(ratio("0/0"), None);
        assert_eq!(ratio("25"), None);
    }

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(2048), "2.0 KB");
        assert_eq!(bytes(5 * 1024 * 1024 + 300_000), "5.3 MB");
    }

    #[test]
    fn a_missing_ffmpeg_says_what_to_install() {
        let err = run("ffprobe-that-does-not-exist", &["-version"]).unwrap_err();
        assert_eq!(err, Error::NotInstalled("ffprobe-that-does-not-exist".into()));
        assert!(err.to_string().contains("brew install ffmpeg"));
    }

    #[test]
    fn a_picture_comes_back_as_pixels() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("red.png");
        let mut image = image::RgbaImage::new(3, 2);
        for p in image.pixels_mut() {
            *p = image::Rgba([255, 0, 0, 255]);
        }
        image.save(&path).expect("write the png");
        let picture = read_image(&path).expect("decode it");
        assert_eq!((picture.width, picture.height), (3, 2));
        assert_eq!(picture.rgba.len(), 3 * 2 * 4);
        assert_eq!(&picture.rgba[..4], [255, 0, 0, 255]);
        // Something that isn't a picture at all fails with a sentence.
        std::fs::write(dir.path().join("x.png"), b"not a png").expect("write");
        let err = read_image(&dir.path().join("x.png")).err().expect("not a picture");
        assert!(err.chars().next().is_some_and(char::is_uppercase), "{err}");
    }
}
