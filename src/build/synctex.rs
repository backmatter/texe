use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

use flate2::{Compression, bufread::GzDecoder, write::GzEncoder};

const MAX_EXPANDED_BYTES: usize = 128 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 1024 * 1024;

fn read_line(input: &mut impl BufRead, line: &mut Vec<u8>, total: &mut usize) -> io::Result<bool> {
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Ok(!line.is_empty());
        }
        let end = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1);
        let count = end.unwrap_or(available.len());
        if line.len().saturating_add(count) > MAX_LINE_BYTES
            || total.saturating_add(count) > MAX_EXPANDED_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SyncTeX output exceeded its expanded size limit",
            ));
        }
        line.extend_from_slice(&available[..count]);
        *total += count;
        input.consume(count);
        if end.is_some() {
            return Ok(true);
        }
    }
}

/// Resolve engine-relative input records before moving `SyncTeX` beside the PDF.
/// Editors need not have the project's working directory. Recompute byte-distance
/// anchors as well, so native `SyncTeX` readers can still seek within the stream.
pub(super) fn for_publication(bytes: &[u8], root: &Path) -> io::Result<Vec<u8>> {
    let mut input = BufReader::new(GzDecoder::new(bytes));
    let mut output = GzEncoder::new(Vec::new(), Compression::default());
    let mut line = Vec::new();
    let mut offset = 0usize;
    let mut previous_anchor = 0;
    let mut changed = false;
    let mut expanded = 0usize;
    while read_line(&mut input, &mut line, &mut expanded)? {
        if let Ok(text) = std::str::from_utf8(&line) {
            let content = text.trim_end_matches(['\r', '\n']);
            let ending = &text[content.len()..];
            if let Some(record) = content.strip_prefix("Input:") {
                if let Some((tag, filename)) = record.split_once(':') {
                    let filename = Path::new(filename);
                    let portable = crate::portable::active(root).ok().flatten();
                    let mapped = portable
                        .as_ref()
                        .filter(|v| v.get("managedBibliography").is_some())
                        .and_then(|v| {
                            let entry = Path::new(v["entry"].as_str()?);
                            let absolute = if filename.is_absolute() {
                                filename.to_owned()
                            } else {
                                root.join(filename)
                            };
                            let relative = absolute.strip_prefix(root).ok()?;
                            (relative
                                .components()
                                .any(|c| c.as_os_str() == ".texe-generated")
                                && filename.file_name() == entry.file_name())
                            .then(|| root.join(entry))
                        });
                    let filename = mapped.as_deref().unwrap_or(filename);
                    if mapped.is_some() {
                        line = format!(
                            "Input:{tag}:{}{ending}",
                            super::process::engine_path(filename).to_string_lossy()
                        )
                        .into_bytes();
                        changed = true;
                    } else if !filename.as_os_str().is_empty() && filename.is_relative() {
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
        if offset.saturating_add(line.len()) > MAX_EXPANDED_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Rewritten SyncTeX output exceeded its expanded size limit",
            ));
        }
        output.write_all(&line)?;
        offset += line.len();
        line.clear();
    }
    if !input.into_inner().into_inner().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SyncTeX contains trailing compressed data",
        ));
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

    #[test]
    fn compressed_long_lines_and_invalid_trailing_data_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let bomb = compress(&"a".repeat(MAX_LINE_BYTES + 1));
        assert!(bomb.len() < 4096);
        assert!(for_publication(&bomb, root.path()).is_err());
        let mut trailing = compress("SyncTeX Version:1\n");
        trailing.extend_from_slice(b"unverified tail");
        assert!(for_publication(&trailing, root.path()).is_err());
        let mut broken_footer = compress("SyncTeX Version:1\n");
        let footer = broken_footer.len() - 8;
        broken_footer[footer] ^= 1;
        assert!(for_publication(&broken_footer, root.path()).is_err());
    }

    #[test]
    fn an_expanded_stream_stops_at_its_total_byte_budget() {
        let mut input = std::io::Cursor::new(b"ok\nnext\n");
        let mut line = Vec::new();
        let mut total = MAX_EXPANDED_BYTES - 3;
        assert!(read_line(&mut input, &mut line, &mut total).unwrap());
        assert_eq!(line, b"ok\n");
        assert_eq!(total, MAX_EXPANDED_BYTES);
        line.clear();
        assert!(read_line(&mut input, &mut line, &mut total).is_err());
        assert!(line.is_empty());
    }
}
