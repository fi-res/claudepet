//! Files the user feeds Clawd by dropping them on him. Text files (and folder
//! listings) are read here, capped so a big log can't eat the user's token budget,
//! and go into the chat prompt while Clawd "remembers" them.

use std::{fs, io::Read, path::Path};

use serde::{Deserialize, Serialize};

/// How much of one file is read, and how much of all fed files goes into a prompt.
pub const MAX_FILE_CHARS: usize = 12_000;
pub const MAX_PROMPT_CHARS: usize = 24_000;
const MAX_FOLDER_ENTRIES: usize = 80;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FedFile {
    pub name: String,
    pub path: String,
    /// "text", "folder" or "binary" (images, archives... Clawd can't read those).
    pub kind: String,
    pub size: u64,
    /// The text, or None for binary files.
    pub text: Option<String>,
    pub truncated: bool,
}

fn clip(text: &str, max: usize) -> (String, bool) {
    match text.char_indices().nth(max) {
        Some((i, _)) => (text[..i].to_string(), true),
        None => (text.to_string(), false),
    }
}

/// Text from bytes: UTF-8 (with or without BOM) or UTF-16 with a BOM. Anything with
/// NUL bytes or otherwise not text counts as binary.
fn as_text(bytes: &[u8]) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return Some(String::from_utf16_lossy(&units));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return Some(String::from_utf16_lossy(&units));
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if bytes.contains(&0) {
        return None;
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text.to_string()),
        // The read may have stopped in the middle of a multi-byte character.
        Err(e) if e.error_len().is_none() && e.valid_up_to() + 4 > bytes.len() => {
            Some(String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned())
        }
        Err(_) => None,
    }
}

pub fn read(path: &Path) -> Result<FedFile, String> {
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let base = |kind: &str, text: Option<String>, truncated: bool| FedFile {
        name: name.clone(),
        path: path.display().to_string(),
        kind: kind.into(),
        size: meta.len(),
        text,
        truncated,
    };
    if meta.is_dir() {
        let mut entries: Vec<String> = fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| {
                let dir = e.file_type().is_ok_and(|t| t.is_dir());
                format!("{}{}", e.file_name().to_string_lossy(), if dir { "/" } else { "" })
            })
            .collect();
        entries.sort();
        let truncated = entries.len() > MAX_FOLDER_ENTRIES;
        entries.truncate(MAX_FOLDER_ENTRIES);
        return Ok(base("folder", Some(entries.join("\n")), truncated));
    }
    // Four bytes per character at most, plus a little to tell a cut-off file.
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take((MAX_FILE_CHARS * 4 + 4) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    match as_text(&bytes) {
        Some(text) => {
            let (text, cut) = clip(&text, MAX_FILE_CHARS);
            Ok(base("text", Some(text), cut || (bytes.len() as u64) < meta.len()))
        }
        None => Ok(base("binary", None, false)),
    }
}

/// The part of the chat prompt that holds the fed files.
pub fn prompt_section(files: &[FedFile]) -> String {
    if files.is_empty() {
        return String::new();
    }
    let mut out = String::from("The user fed you these files; answer questions about them from their contents:\n");
    let mut budget = MAX_PROMPT_CHARS;
    for f in files {
        let header = match f.kind.as_str() {
            "folder" => format!("\n--- folder {} (its entries) ---\n", f.name),
            "binary" => format!("\n--- {} ({} bytes, not a text file: you can't see inside) ---\n", f.name, f.size),
            _ => format!("\n--- {} ---\n", f.name),
        };
        out.push_str(&header);
        if let Some(text) = &f.text {
            let (part, cut) = clip(text, budget);
            budget -= part.chars().count();
            out.push_str(&part);
            if cut || f.truncated {
                out.push_str("\n[…the rest is cut off]");
            }
            out.push('\n');
        }
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_text_from_binary() {
        assert_eq!(as_text("привет".as_bytes()).as_deref(), Some("привет"));
        assert_eq!(as_text(&[0xEF, 0xBB, 0xBF, b'h', b'i']).as_deref(), Some("hi"));
        assert_eq!(as_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]).as_deref(), Some("hi"));
        assert_eq!(as_text(&[0x89, b'P', b'N', b'G', 0, 1, 2]), None);
        // Cut in the middle of "я" (2 bytes) is still text.
        let mut cut = "abя".as_bytes().to_vec();
        cut.pop();
        assert_eq!(as_text(&cut).as_deref(), Some("ab"));
    }

    #[test]
    fn reads_files_and_folders() {
        let dir = std::env::temp_dir().join(format!("clawd-feed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("notes.txt"), "line one\nline two").unwrap();
        fs::write(dir.join("big.log"), "x".repeat(MAX_FILE_CHARS + 50)).unwrap();
        fs::write(dir.join("pic.png"), [0x89, b'P', b'N', b'G', 0, 0, 0]).unwrap();

        let notes = read(&dir.join("notes.txt")).unwrap();
        assert_eq!((notes.kind.as_str(), notes.text.as_deref(), notes.truncated), ("text", Some("line one\nline two"), false));
        let big = read(&dir.join("big.log")).unwrap();
        assert!(big.truncated && big.text.unwrap().chars().count() == MAX_FILE_CHARS);
        assert_eq!(read(&dir.join("pic.png")).unwrap().kind, "binary");
        let folder = read(&dir).unwrap();
        assert_eq!(folder.kind, "folder");
        assert!(folder.text.unwrap().contains("sub/"));

        let section = prompt_section(&[notes, read(&dir.join("pic.png")).unwrap()]);
        assert!(section.contains("--- notes.txt ---\nline one"));
        assert!(section.contains("not a text file"));
        assert_eq!(prompt_section(&[]), "");
        let _ = fs::remove_dir_all(dir);
    }
}
