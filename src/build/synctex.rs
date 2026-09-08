use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

use flate2::{Compression, bufread::GzDecoder, write::GzEncoder};

/// Resolve engine-relative input records before moving `SyncTeX` beside the PDF.
/// Editors need not have the project's working directory. Recompute byte-distance
/// anchors as well, so native `SyncTeX` readers can still seek within the stream.
pub(super) fn for_publication(bytes: &[u8], root: &Path) -> io::Result<Vec<u8>> {
    let mut input = BufReader::new(GzDecoder::new(bytes));
    let mut output = GzEncoder::new(Vec::new(), Compression::default());
    let mut line = Vec::new();
    let mut offset = 0;
    let mut previous_anchor = 0;
    let mut changed = false;
    while input.read_until(b'\n', &mut line)? != 0 {
        if let Ok(text) = std::str::from_utf8(&line) {
            let content = text.trim_end_matches(['\r', '\n']);
            let ending = &text[content.len()..];
            if let Some(record) = content.strip_prefix("Input:") {
                if let Some((tag, filename)) = record.split_once(':') {
                    let filename = Path::new(filename);
                    if !filename.as_os_str().is_empty() && filename.is_relative() {
                        let absolute = std::path::absolute(root.join(filename))?;
                        let absolute = super::process::engine_path(&absolute);
                        line = format!("Input:{tag}:{}{ending}", absolute.to_string_lossy())
                            .into_bytes();
                        changed = true;
                    }
                }
            } else if content
                .strip_prefix('!')
                .is_some_and(|value| value.parse::<usize>().is_ok())
            {
                let distance = offset - previous_anchor;
                previous_anchor = offset;
                line = format!("!{distance}{ending}").into_bytes();
            }
        }
        output.write_all(&line)?;
        offset += line.len();
        line.clear();
    }
    let rewritten = output.finish()?;
    Ok(if changed { rewritten } else { bytes.to_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn compress(text: &str) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn relative_inputs_and_all_anchor_distances_follow_published_location() {
        let root = tempfile::tempdir().unwrap();
        let data = compress(
            "SyncTeX Version:1\nInput:1:./chapter.tex\n!38\n{1\nInput:2:figures/data.tex\n!36\n}1\n",
        );
        let result = for_publication(&data, root.path()).unwrap();
        let mut text = String::new();
        GzDecoder::new(result.as_slice())
            .read_to_string(&mut text)
            .unwrap();
        assert!(text.contains(&format!(
            "Input:1:{}",
            super::super::process::engine_path(&root.path().join("chapter.tex")).to_string_lossy()
        )));
        assert!(!text.contains("Input:2:figures/"));
        let mut offset = 0;
        let mut previous = 0;
        for line in text.split_inclusive('\n') {
            if let Some(distance) = line.strip_prefix('!') {
                assert_eq!(distance.trim().parse::<usize>().unwrap(), offset - previous);
                previous = offset;
            }
            offset += line.len();
        }
    }

    #[test]
    fn absolute_inputs_keep_the_original_compressed_bytes() {
        let root = tempfile::tempdir().unwrap();
        let text = format!(
            "SyncTeX Version:1\nInput:1:{}\n",
            root.path().join("main.tex").display()
        );
        let data = compress(&text);
        assert_eq!(for_publication(&data, root.path()).unwrap(), data);
        assert!(for_publication(b"invalid gzip", root.path()).is_err());
    }
}
