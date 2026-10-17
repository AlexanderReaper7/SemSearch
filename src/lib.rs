//! `semsearch`: feeds the hister instance and queries it. The CLI is
//! main.rs; the desktop window in app/ links this library.
//!
//! Every source lives in one instance of the hister fork, and a source is a
//! document type: `code`, `local` for files, `web`. See docs/decisions.md.

pub mod ask;
pub mod chunk;
pub mod code;
pub mod hister;
pub mod history;
pub mod time;

/// A file in semsearch's data directory, `$XDG_DATA_HOME/semsearch`.
pub fn data_file(name: &str) -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME is set")).join(".local/share"));
    base.join("semsearch").join(name)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Web,
    Files,
    Code,
}

impl Source {
    /// The query filter that selects this source.
    pub fn filter(self) -> &'static str {
        match self {
            Source::Web => "type:web",
            Source::Files => "type:file",
            Source::Code => "type:code",
        }
    }

    /// The source of a document, from hister's numeric type.
    pub fn of_type(t: Option<u64>) -> Option<Source> {
        match t? {
            0 => Some(Source::Web),
            1 | 2 => Some(Source::Files),
            3 => Some(Source::Code),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Source::Web => "web",
            Source::Files => "files",
            Source::Code => "code",
        }
    }
}
