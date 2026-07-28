use std::fs;
use std::path::Path;

use crate::command::repo_root;
use crate::{Result, message};

pub(crate) fn homebrew(version: &str, digest: &str, output: &Path) -> Result<()> {
    render(
        version,
        digest,
        output,
        "packaging/homebrew/texe.rb.in",
        false,
    )
}

pub(crate) fn winget(version: &str, digest: &str, output: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    let root = repo_root()?.join("packaging/winget");
    let mut templates = fs::read_dir(&root)?.collect::<std::result::Result<Vec<_>, _>>()?;
    templates.sort_by_key(fs::DirEntry::file_name);
    for entry in templates {
        let source = entry.path();
        if source.extension().and_then(|value| value.to_str()) != Some("in") {
            continue;
        }
        let filename = source
            .file_stem()
            .ok_or_else(|| message("WinGet template has no filename"))?;
        let rendered = fs::read_to_string(&source)?
            .replace("@VERSION@", version)
            .replace("@SHA256@", &digest.to_ascii_uppercase());
        fs::write(output.join(filename), rendered)?;
    }
    Ok(())
}

fn render(
    version: &str,
    digest: &str,
    output: &Path,
    template: &str,
    uppercase: bool,
) -> Result<()> {
    let mut digest = digest.to_string();
    if uppercase {
        digest.make_ascii_uppercase();
    } else {
        digest.make_ascii_lowercase();
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let rendered = fs::read_to_string(repo_root()?.join(template))?
        .replace("@VERSION@", version)
        .replace("@SHA256@", &digest);
    fs::write(output, rendered)?;
    Ok(())
}
