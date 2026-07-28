use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use crate::TexeError;
use crate::atomic::write as atomic_write;
use crate::build::process::{
    engine_input_environment, engine_path_from, managed_path, raw_engine_output, search_path_from,
    source_date_environment,
};
use crate::package::PackageEnvironment;
use crate::progress::{PhaseKind, Progress};
use crate::toolchain::ResolvedToolchain;

/// Formats are cached per toolchain and package environment, so their embedded
/// timestamp cannot come from a project's source-date lock.
const FORMAT_SOURCE_DATE_EPOCH: &str = "0";

#[derive(Debug, Clone)]
pub(super) struct ManagedFormat {
    pub(super) root: PathBuf,
    pub(super) formats: PathBuf,
}

#[derive(Debug, Clone, Copy)]
struct ManagedFormatRecipe {
    name: &'static str,
    ini: &'static str,
    translate_file: Option<&'static str>,
    language_lua: bool,
    extended_mode: bool,
}

struct FormatBuild<'a> {
    project_root: &'a Path,
    toolchain: &'a ResolvedToolchain,
    texmf: &'a Path,
    package_environment: &'a PackageEnvironment,
    recipe: &'static ManagedFormatRecipe,
    target: PathBuf,
    staging: PathBuf,
    formats: PathBuf,
    config: PathBuf,
}

const PDFLATEX_FORMAT: ManagedFormatRecipe = ManagedFormatRecipe {
    name: "pdflatex",
    ini: "pdflatex.ini",
    translate_file: Some("cp227.tcx"),
    language_lua: false,
    extended_mode: true,
};

const LUALATEX_FORMAT: ManagedFormatRecipe = ManagedFormatRecipe {
    name: "lualatex",
    ini: "lualatex.ini",
    translate_file: None,
    language_lua: true,
    extended_mode: false,
};

const XELATEX_FORMAT: ManagedFormatRecipe = ManagedFormatRecipe {
    name: "xelatex",
    ini: "xelatex.ini",
    translate_file: None,
    language_lua: false,
    extended_mode: true,
};

fn format_recipe(engine: &str) -> Result<&'static ManagedFormatRecipe, TexeError> {
    match engine {
        "pdflatex" => Ok(&PDFLATEX_FORMAT),
        "lualatex" => Ok(&LUALATEX_FORMAT),
        "xelatex" => Ok(&XELATEX_FORMAT),
        _ => Err(TexeError::Build(format!(
            "engine `{engine}` has no locked format recipe"
        ))),
    }
}

pub(super) fn name(engine: &str) -> Result<&'static str, TexeError> {
    Ok(format_recipe(engine)?.name)
}

pub(super) fn ensure(
    project_root: &Path,
    toolchain: &ResolvedToolchain,
    texmf: &Path,
    package_environment: &PackageEnvironment,
    build_root: &Path,
    packages_remote: bool,
    progress: &Progress,
) -> Result<Option<ManagedFormat>, TexeError> {
    let managed = toolchain.managed.as_ref();
    if managed.is_none() && !packages_remote {
        return Ok(None);
    }
    let recipe = format_recipe(&toolchain.engine)?;
    let environment_key = environment_cache_key(&package_environment.fingerprint)?;
    let format_cache = managed.map_or_else(
        || build_root.join("formats"),
        |runtime| runtime.format_cache.clone(),
    );
    let target = format_cache
        .join(&toolchain.identity.fingerprint)
        .join(environment_key)
        .join(recipe.name);
    let format = ManagedFormat {
        root: target.clone(),
        formats: target.join("formats"),
    };
    let format_file = format.formats.join(format!("{}.fmt", recipe.name));
    if format_file.is_file() {
        return Ok(Some(format));
    }

    let build = prepare_format_build(
        project_root,
        toolchain,
        texmf,
        package_environment,
        recipe,
        target,
    )?;
    let result = progress.phase(
        PhaseKind::Format,
        format!("generating locked {} format", recipe.name),
        || generate_format(&build),
    );
    if result.is_err() && build.staging.exists() {
        let _ = remove_format_staging(&build.staging, recipe.name);
    }
    result?;
    Ok(Some(format))
}

fn environment_cache_key(fingerprint: &str) -> Result<&str, TexeError> {
    let key = fingerprint.strip_prefix("sha256:").unwrap_or(fingerprint);
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(TexeError::Build(format!(
            "pqty emitted an unsafe environment fingerprint: {fingerprint}"
        )));
    }
    Ok(key)
}

fn prepare_format_build<'a>(
    project_root: &'a Path,
    toolchain: &'a ResolvedToolchain,
    texmf: &'a Path,
    package_environment: &'a PackageEnvironment,
    recipe: &'static ManagedFormatRecipe,
    target: PathBuf,
) -> Result<FormatBuild<'a>, TexeError> {
    let parent = target.parent().ok_or_else(|| {
        TexeError::Build(format!(
            "format cache path has no parent: {}",
            target.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|source| TexeError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let staging = parent.join(format!(".{}.{}.tmp", recipe.name, std::process::id()));
    if staging.exists() {
        remove_format_staging(&staging, recipe.name)?;
    }
    let formats = staging.join("formats");
    let config = staging.join("config/tex/generic/config");
    for directory in [
        &formats,
        &config,
        &staging.join("var"),
        &staging.join("user-config"),
    ] {
        fs::create_dir_all(directory).map_err(|source| TexeError::Io {
            path: (*directory).clone(),
            source,
        })?;
    }
    Ok(FormatBuild {
        project_root,
        toolchain,
        texmf,
        package_environment,
        recipe,
        target,
        staging,
        formats,
        config,
    })
}

fn generate_format(build: &FormatBuild<'_>) -> Result<(), TexeError> {
    prepare_format_configuration(build)?;
    let arguments = format_arguments(build);
    let environment = format_environment(build);
    let output = raw_engine_output(
        &build.toolchain.engine_executable,
        &arguments,
        build.project_root,
        &environment,
    )?;
    validate_generated_format(build, &output)?;
    publish_generated_format(build)
}

fn prepare_format_configuration(build: &FormatBuild<'_>) -> Result<(), TexeError> {
    let ini = locked_format_ini(build.texmf, build.recipe);
    if !ini.is_file() {
        return Err(TexeError::Build(format!(
            "locked format bootstrap file is absent from pqty.lock: {}",
            ini.display()
        )));
    }
    copy_format_config(
        &build.texmf.join("tex/generic/config/language.us"),
        &build.config.join("language.dat"),
    )?;
    copy_format_config(
        &build.texmf.join("tex/generic/config/language.us.def"),
        &build.config.join("language.def"),
    )?;
    if build.recipe.language_lua {
        copy_format_config(
            &build.texmf.join("tex/generic/config/language.us.lua"),
            &build.config.join("language.dat.lua"),
        )?;
        write_luaotfload_config(&build.config.join("luaotfload.conf"))?;
    }
    write_pdftex_map(
        build.texmf,
        &["pdftex35.map"],
        &build.package_environment.font_maps,
        &build
            .staging
            .join("user-config/fonts/map/pdftex/updmap/pdftex.map"),
    )
}

fn format_arguments(build: &FormatBuild<'_>) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("-ini"),
        OsString::from("-recorder"),
        OsString::from("-interaction=nonstopmode"),
        OsString::from("-halt-on-error"),
        OsString::from(format!("-jobname={}", build.recipe.name)),
        OsString::from(format!("-progname={}", build.recipe.name)),
    ];
    if build.recipe.extended_mode {
        arguments.push(OsString::from("-etex"));
    }
    if let Some(translate_file) = build.recipe.translate_file {
        let runtime_texmf = build.toolchain.managed.as_ref().map_or_else(
            || build.toolchain.texmf_dist.clone(),
            |runtime| {
                // The managed runtime owns only engine bootstrap data; LaTeX
                // macros still come from the locked pqty tree.
                runtime.root.join("texmf-dist")
            },
        );
        arguments.push(prefixed_engine_path(
            "-translate-file=",
            &runtime_texmf.join("web2c").join(translate_file),
            build.project_root,
        ));
    }
    arguments.extend([
        prefixed_engine_path("-output-directory=", &build.formats, build.project_root),
        format_ini_argument(build.texmf, build.recipe, build.project_root),
    ]);
    arguments
}

fn locked_format_ini(texmf: &Path, recipe: &ManagedFormatRecipe) -> PathBuf {
    texmf.join("tex/latex/tex-ini-files").join(recipe.ini)
}

fn format_ini_argument(
    texmf: &Path,
    recipe: &ManagedFormatRecipe,
    working_directory: &Path,
) -> OsString {
    engine_path_from(&locked_format_ini(texmf, recipe), working_directory)
}

fn prefixed_engine_path(prefix: &str, path: &Path, working_directory: &Path) -> OsString {
    let mut argument = OsString::from(prefix);
    argument.push(engine_path_from(path, working_directory));
    argument
}

fn format_environment(build: &FormatBuild<'_>) -> Vec<(OsString, OsString)> {
    let managed = build.toolchain.managed.as_ref();
    let staging_config = build.staging.join("config");
    let managed_texmf_dist = managed.map(|runtime| runtime.root.join("texmf-dist"));
    let mut tex_search_roots = vec![
        build.staging.as_path(),
        staging_config.as_path(),
        build.texmf,
    ];
    if let Some(texmf_dist) = &managed_texmf_dist {
        tex_search_roots.push(texmf_dist);
    }
    let system_var = managed.map_or_else(
        || build.staging.join("var"),
        |runtime| runtime.root.join("texmf-var"),
    );
    let system_config = managed.map_or_else(
        || build.staging.join("user-config"),
        |runtime| runtime.root.join("texmf-config"),
    );
    let mut font_map_roots = vec![
        build.staging.join("user-config/fonts/map"),
        build.texmf.join("fonts/map"),
    ];
    if let Some(texmf_dist) = &managed_texmf_dist {
        font_map_roots.push(texmf_dist.join("fonts/map"));
    }
    let font_map_root_refs = font_map_roots
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let search = search_path_from(&tex_search_roots, build.project_root);
    let mut environment = vec![
        (
            OsString::from("TEXMFHOME"),
            engine_path_from(build.texmf, build.project_root),
        ),
        (
            OsString::from("TEXMFVAR"),
            engine_path_from(&build.staging.join("var"), build.project_root),
        ),
        (
            OsString::from("TEXMFCACHE"),
            engine_path_from(&build.staging.join("var"), build.project_root),
        ),
        (
            OsString::from("TEXMFCONFIG"),
            engine_path_from(&build.staging.join("user-config"), build.project_root),
        ),
        (
            OsString::from("TEXMFSYSVAR"),
            engine_path_from(&system_var, build.project_root),
        ),
        (
            OsString::from("TEXMFSYSCONFIG"),
            engine_path_from(&system_config, build.project_root),
        ),
        (
            OsString::from("TEXFORMATS"),
            engine_path_from(&build.formats, build.project_root),
        ),
        (
            OsString::from("TEXFONTMAPS"),
            search_path_from(&font_map_root_refs, build.project_root),
        ),
    ];
    environment.extend(engine_input_environment(build.recipe.name, search));
    if let Some(managed) = managed {
        environment.extend([
            (
                OsString::from("OSFONTDIR"),
                engine_path_from(&build.texmf.join("fonts"), build.project_root),
            ),
            (OsString::from("PATH"), managed_path(&managed.binary_dir)),
        ]);
    }
    environment.extend(source_date_environment(FORMAT_SOURCE_DATE_EPOCH));
    environment
}

fn validate_generated_format(
    build: &FormatBuild<'_>,
    output: &std::process::Output,
) -> Result<(), TexeError> {
    if output.status.success()
        && build
            .formats
            .join(format!("{}.fmt", build.recipe.name))
            .is_file()
    {
        return Ok(());
    }
    let log = fs::read_to_string(build.formats.join(format!("{}.log", build.recipe.name)))
        .unwrap_or_default();
    let detail = if log.is_empty() {
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    } else {
        log.lines()
            .rev()
            .take(40)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    };
    let detail = detail.trim();
    let detail = if detail.is_empty() {
        format!("engine exited with {}", output.status)
    } else {
        detail.to_string()
    };
    Err(TexeError::Build(format!(
        "could not generate locked {} format:\n{detail}",
        build.recipe.name
    )))
}

fn publish_generated_format(build: &FormatBuild<'_>) -> Result<(), TexeError> {
    match fs::rename(&build.staging, &build.target) {
        Ok(()) => Ok(()),
        Err(_)
            if build
                .target
                .join("formats")
                .join(format!("{}.fmt", build.recipe.name))
                .is_file() =>
        {
            remove_format_staging(&build.staging, build.recipe.name)
        }
        Err(source) => Err(TexeError::Io {
            path: build.target.clone(),
            source,
        }),
    }
}

fn copy_format_config(source: &Path, target: &Path) -> Result<(), TexeError> {
    if !source.is_file() {
        return Err(TexeError::Build(format!(
            "locked format bootstrap file is absent from pqty.lock: {}",
            source.display()
        )));
    }
    fs::copy(source, target)
        .map(|_| ())
        .map_err(|source_error| TexeError::Io {
            path: target.to_path_buf(),
            source: source_error,
        })
}

fn write_luaotfload_config(target: &Path) -> Result<(), TexeError> {
    // LuaLaTeX normally indexes both TeX and operating-system fonts. A managed
    // lock describes only the former, so make that boundary explicit instead
    // of allowing luaotfload to read /etc/fonts or a user's font directories.
    atomic_write(
        target,
        b"; Generated by texe: managed LuaLaTeX sees only locked TEXMF fonts.\n\
          [db]\n\
          location-precedence = texmf\n",
    )
}

fn write_pdftex_map(
    texmf: &Path,
    foundational_maps: &[&str],
    map_names: &[String],
    target: &Path,
) -> Result<(), TexeError> {
    let mut bytes = b"% Generated by texe from locked TeX Live map declarations.\n".to_vec();
    for map_name in foundational_maps {
        append_font_map(texmf, map_name, &mut bytes)?;
    }
    for map_name in map_names {
        append_font_map(texmf, map_name, &mut bytes)?;
    }
    atomic_write(target, &bytes)
}

fn append_font_map(texmf: &Path, map_name: &str, bytes: &mut Vec<u8>) -> Result<(), TexeError> {
    let map = find_unique_font_map(&texmf.join("fonts/map"), map_name)?;
    bytes.extend_from_slice(format!("\n% {map_name}\n").as_bytes());
    let fragment = fs::read(&map).map_err(|source| TexeError::Io {
        path: map.clone(),
        source,
    })?;
    bytes.extend_from_slice(&fragment);
    if !fragment.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    Ok(())
}

fn find_unique_font_map(root: &Path, map_name: &str) -> Result<PathBuf, TexeError> {
    if Path::new(map_name).file_name().and_then(OsStr::to_str) != Some(map_name) {
        return Err(TexeError::Build(format!(
            "pqty emitted an invalid font-map name: {map_name}"
        )));
    }
    let mut matches = Vec::new();
    collect_named_files(root, map_name, &mut matches)?;
    match matches.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(TexeError::Build(format!(
            "locked font-map fragment is missing from the pqty tree: {map_name}"
        ))),
        _ => Err(TexeError::Build(format!(
            "locked font-map fragment is ambiguous in the pqty tree: {map_name}"
        ))),
    }
}

fn collect_named_files(
    directory: &Path,
    name: &str,
    matches: &mut Vec<PathBuf>,
) -> Result<(), TexeError> {
    let entries = fs::read_dir(directory).map_err(|source| TexeError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| TexeError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| TexeError::Io {
            path: path.clone(),
            source,
        })?;
        // Recurse only into real directories, so a linked tree cannot loop, but
        // resolve the leaves: a symlinked or hardlinked package tree stores its
        // font maps as links into the pqty store, and `file_type` here does not
        // follow them.
        if file_type.is_dir() {
            collect_named_files(&path, name, matches)?;
        } else if entry.file_name() == OsStr::new(name) && path.is_file() {
            matches.push(path);
        }
    }
    Ok(())
}

fn remove_format_staging(path: &Path, format_name: &str) -> Result<(), TexeError> {
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    if !name.starts_with(&format!(".{format_name}."))
        || !Path::new(name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
    {
        return Err(TexeError::Build(format!(
            "refusing to remove unexpected format staging path: {}",
            path.display()
        )));
    }
    fs::remove_dir_all(path).map_err(|source| TexeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    use crate::build::format::{
        PDFLATEX_FORMAT, format_ini_argument, locked_format_ini, write_luaotfload_config,
        write_pdftex_map,
    };

    #[test]
    fn format_bootstrap_uses_the_exact_locked_ini() {
        let texmf = Path::new("/locked/texmf");
        assert_eq!(
            locked_format_ini(texmf, &PDFLATEX_FORMAT),
            texmf.join("tex/latex/tex-ini-files/pdflatex.ini")
        );
        let argument = format_ini_argument(texmf, &PDFLATEX_FORMAT, Path::new("/project"));
        let expected = OsString::from("/locked/texmf/tex/latex/tex-ini-files/pdflatex.ini");
        assert_eq!(argument, expected);
    }

    #[test]
    fn pdftex_map_places_foundational_aliases_before_package_maps() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let texmf = directory.path().join("texmf");
        let maps = texmf.join("fonts/map/dvips/example");
        fs::create_dir_all(&maps).expect("font map directory");
        fs::write(maps.join("pdftex35.map"), b"pbkl8r URWBookman\n").expect("foundational map");
        fs::write(maps.join("example.map"), b"Example Example-Regular\n").expect("package map");
        let target = directory.path().join("pdftex.map");

        write_pdftex_map(
            &texmf,
            &["pdftex35.map"],
            &["example.map".to_string()],
            &target,
        )
        .expect("combined map");

        let map = fs::read_to_string(target).expect("combined map text");
        assert!(map.contains("pbkl8r URWBookman"));
        assert!(map.contains("Example Example-Regular"));
        assert!(
            map.find("pbkl8r URWBookman").expect("foundational entry")
                < map.find("Example Example-Regular").expect("package entry")
        );
    }

    #[test]
    fn managed_luaotfload_config_disables_system_font_locations() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("luaotfload.conf");
        write_luaotfload_config(&path).expect("write configuration");
        let configuration = fs::read_to_string(path).expect("read configuration");
        assert!(configuration.contains("location-precedence = texmf"));
        assert!(!configuration.contains("location_precedence"));
        assert!(!configuration.contains("location-precedence = system"));
    }
}
