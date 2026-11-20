use std::{
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Family {
    Image,
    Audio,
    Video,
    Data,
    /// Documents and text: Markdown, HTML, DOCX, PDF...
    Text,
    Archive,
}

impl Family {
    pub fn name(self) -> &'static str {
        match self {
            Family::Image => "Image",
            Family::Audio => "Audio",
            Family::Video => "Video",
            Family::Data => "Data",
            Family::Text => "Documents",
            Family::Archive => "Archive",
        }
    }
}

macro_rules! formats {
    ($( $variant:ident => ($name:literal, $family:ident, [$($ext:literal),+]) ),+ $(,)?) => {
        /// Every format uberconv knows about. Adding a variant here is the only
        /// thing needed before a converter can declare edges for it.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Format { $($variant),+ }

        impl Format {
            pub const ALL: &'static [Format] = &[$(Format::$variant),+];

            pub fn name(self) -> &'static str {
                match self { $(Format::$variant => $name),+ }
            }

            pub fn family(self) -> Family {
                match self { $(Format::$variant => Family::$family),+ }
            }

            /// File extensions, preferred one first, lowercase, no dot.
            pub fn extensions(self) -> &'static [&'static str] {
                match self { $(Format::$variant => &[$($ext),+]),+ }
            }
        }
    };
}

formats! {
    Png      => ("PNG",      Image, ["png"]),
    Jpeg     => ("JPEG",     Image, ["jpg", "jpeg"]),
    Gif      => ("GIF",      Image, ["gif"]),
    Bmp      => ("BMP",      Image, ["bmp"]),
    Tiff     => ("TIFF",     Image, ["tiff", "tif"]),
    WebP     => ("WebP",     Image, ["webp"]),
    Ico      => ("ICO",      Image, ["ico"]),
    Avif     => ("AVIF",     Image, ["avif"]),
    // Input only: decoded through libheif (bundled) or Windows' own HEIF codec.
    Heic     => ("HEIC",     Image, ["heic", "heif"]),
    Svg      => ("SVG",      Image, ["svg"]),
    Mp3      => ("MP3",      Audio, ["mp3"]),
    Wav      => ("WAV",      Audio, ["wav"]),
    Flac     => ("FLAC",     Audio, ["flac"]),
    Ogg      => ("Ogg Vorbis", Audio, ["ogg", "oga"]),
    Opus     => ("Opus",     Audio, ["opus"]),
    M4a      => ("M4A",      Audio, ["m4a"]),
    Aac      => ("AAC",      Audio, ["aac"]),
    Mp4      => ("MP4",      Video, ["mp4", "m4v"]),
    Mkv      => ("MKV",      Video, ["mkv"]),
    WebM     => ("WebM",     Video, ["webm"]),
    Mov      => ("MOV",      Video, ["mov"]),
    Avi      => ("AVI",      Video, ["avi"]),
    Json     => ("JSON",     Data,  ["json"]),
    Yaml     => ("YAML",     Data,  ["yaml", "yml"]),
    Toml     => ("TOML",     Data,  ["toml"]),
    Csv      => ("CSV",      Data,  ["csv"]),
    Markdown => ("Markdown", Text,  ["md", "markdown"]),
    Html     => ("HTML",     Text,  ["html", "htm"]),
    Text     => ("Text",     Text,  ["txt"]),
    Docx     => ("Word",     Text,  ["docx"]),
    Pdf      => ("PDF",      Text,  ["pdf"]),
    Zip      => ("ZIP",      Archive, ["zip"]),
    Tar      => ("TAR",      Archive, ["tar"]),
    TarGz    => ("TAR.GZ",   Archive, ["tar.gz", "tgz"]),
}

impl Format {
    /// Preferred extension (no dot).
    pub fn ext(self) -> &'static str {
        self.extensions()[0]
    }

    pub fn from_extension(ext: &str) -> Option<Format> {
        let ext = ext.trim_start_matches('.');
        Format::ALL
            .iter()
            .copied()
            .find(|f| f.extensions().iter().any(|e| e.eq_ignore_ascii_case(ext)))
    }

    /// The format and the matched extension of a file name. Handles compound
    /// extensions: `backup.tar.gz` is TAR.GZ (the longest match wins), not "gz".
    fn match_name(name: &str) -> Option<(Format, &'static str)> {
        let lower = name.to_ascii_lowercase();
        Format::ALL
            .iter()
            .copied()
            .flat_map(|f| f.extensions().iter().map(move |e| (f, *e)))
            .filter(|(_, e)| lower.len() > e.len() + 1 && lower.ends_with(&format!(".{e}")))
            .max_by_key(|(_, e)| e.len())
    }

    pub fn from_path(path: &Path) -> Option<Format> {
        Format::match_name(path.file_name()?.to_str()?).map(|(f, _)| f)
    }

    /// File name without its (possibly compound) extension: `a.tar.gz` -> `a`.
    /// Unknown extensions are stripped the usual way.
    pub fn stem_of(path: &Path) -> String {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return "output".to_string();
        };
        match Format::match_name(name) {
            Some((_, ext)) => name[..name.len() - ext.len() - 1].to_string(),
            None => path.file_stem().and_then(|s| s.to_str()).unwrap_or(name).to_string(),
        }
    }

    /// `dir/photo.png` + JPEG -> `dir/photo.jpg` (compound-extension aware).
    pub fn with_format_extension(path: &Path, to: Format) -> PathBuf {
        path.with_file_name(format!("{}.{}", Format::stem_of(path), to.ext()))
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Format {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let t = s.trim().trim_start_matches('.');
        Format::from_extension(t)
            .or_else(|| Format::ALL.iter().copied().find(|f| f.name().eq_ignore_ascii_case(t)))
            .ok_or_else(|| Error::UnknownFormat(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compound_extensions() {
        assert_eq!(Format::from_path(Path::new("/x/backup.tar.gz")), Some(Format::TarGz));
        assert_eq!(Format::from_path(Path::new("BACKUP.TGZ")), Some(Format::TarGz));
        assert_eq!(Format::from_path(Path::new("a.tar")), Some(Format::Tar));
        assert_eq!(Format::from_path(Path::new("plain.gz")), None);
        assert_eq!(Format::from_path(Path::new(".png")), None); // hidden file, no stem
        assert_eq!(Format::stem_of(Path::new("dir/backup.tar.gz")), "backup");
        assert_eq!(Format::stem_of(Path::new("my.photo.final.jpeg")), "my.photo.final");
        assert_eq!(Format::stem_of(Path::new("notes.xyz")), "notes");
        assert_eq!(
            Format::with_format_extension(Path::new("d/a.tar.gz"), Format::Zip),
            PathBuf::from("d/a.zip")
        );
        assert_eq!(
            Format::with_format_extension(Path::new("d/a.zip"), Format::TarGz),
            PathBuf::from("d/a.tar.gz")
        );
        assert_eq!("tar.gz".parse::<Format>().unwrap(), Format::TarGz);
    }
}
