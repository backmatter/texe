use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::build::filesystem::package_search_path_value;
use crate::build::format::ManagedFormat;
use crate::build::process::{
    engine_input_environment, engine_path_from, managed_path, search_path_from, shell_escape_path,
    source_date_environment,
};
use crate::toolchain::ResolvedToolchain;

pub(super) struct EngineEnvironmentContext<'a> {
    pub(super) working_directory: &'a Path,
    pub(super) texmf: &'a Path,
    pub(super) build_root: &'a Path,
    pub(super) input_roots: &'a [PathBuf],
    pub(super) managed_format: Option<&'a ManagedFormat>,
    pub(super) discovery: bool,
    pub(super) source_date_epoch: u64,
    pub(super) shell_escape: bool,
}

pub(super) fn engine_environment(
    toolchain: &ResolvedToolchain,
    context: &EngineEnvironmentContext<'_>,
) -> Vec<(OsString, OsString)> {
    let source_date = source_date_environment(&context.source_date_epoch.to_string());
    let Some(format) = context.managed_format else {
        let package_search_path = package_search_path_value(
            context.working_directory,
            context.build_root,
            context.texmf,
            context.input_roots,
            context.discovery,
        );
        let mut environment = Vec::from(engine_input_environment(
            &toolchain.engine,
            package_search_path,
        ));
        environment.extend(source_date);
        return environment;
    };
    managed_engine_environment(toolchain, context, format, source_date)
}

fn managed_engine_environment(
    toolchain: &ResolvedToolchain,
    context: &EngineEnvironmentContext<'_>,
    format: &ManagedFormat,
    source_date: [(OsString, OsString); 2],
) -> Vec<(OsString, OsString)> {
    let texmf = context.texmf;
    let build_root = context.build_root;
    let managed = toolchain.managed.as_ref();
    let format_config = format.root.join("config");
    let generated_root = build_root.join(".texe-generated");
    let managed_texmf_dist = managed.map(|runtime| runtime.root.join("texmf-dist"));
    let mut search_roots = vec![generated_root.as_path(), build_root, Path::new(".")];
    search_roots.extend(context.input_roots.iter().map(PathBuf::as_path));
    search_roots.extend([format_config.as_path(), texmf]);
    if let Some(texmf_dist) = &managed_texmf_dist {
        search_roots.push(texmf_dist);
    }
    let search = search_path_from(&search_roots, context.working_directory);
    let system_var = managed.map_or_else(
        || format.root.join("var"),
        |runtime| runtime.root.join("texmf-var"),
    );
    let system_config = managed.map_or_else(
        || format.root.join("user-config"),
        |runtime| runtime.root.join("texmf-config"),
    );
    let mut font_map_roots = vec![
        format.root.join("user-config/fonts/map"),
        texmf.join("fonts/map"),
    ];
    if let Some(texmf_dist) = &managed_texmf_dist {
        font_map_roots.push(texmf_dist.join("fonts/map"));
    }
    let font_map_root_refs = font_map_roots
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let mut environment = vec![
        (
            OsString::from("TEXMFHOME"),
            engine_path_from(texmf, context.working_directory),
        ),
        (
            OsString::from("TEXMFVAR"),
            engine_path_from(&build_root.join("texmf-var"), context.working_directory),
        ),
        (
            OsString::from("TEXMFCACHE"),
            engine_path_from(&build_root.join("texmf-var"), context.working_directory),
        ),
        (
            OsString::from("TEXMFCONFIG"),
            engine_path_from(&format.root.join("user-config"), context.working_directory),
        ),
        (
            OsString::from("TEXMFSYSVAR"),
            engine_path_from(&system_var, context.working_directory),
        ),
        (
            OsString::from("TEXMFSYSCONFIG"),
            engine_path_from(&system_config, context.working_directory),
        ),
        (
            OsString::from("TEXFORMATS"),
            engine_path_from(&format.formats, context.working_directory),
        ),
        (
            OsString::from("TEXFONTMAPS"),
            search_path_from(&font_map_root_refs, context.working_directory),
        ),
    ];
    environment.extend(engine_input_environment(&toolchain.engine, search));
    if let Some(managed) = managed {
        environment.extend([
            (
                OsString::from("OSFONTDIR"),
                engine_path_from(&texmf.join("fonts"), context.working_directory),
            ),
            (
                OsString::from("PATH"),
                if context.shell_escape {
                    shell_escape_path(&managed.binary_dir)
                } else {
                    managed_path(&managed.binary_dir)
                },
            ),
        ]);
    } else {
        environment.extend(system_font_environment(
            toolchain,
            texmf,
            build_root,
            context.working_directory,
        ));
    }
    environment.extend(source_date);
    environment
}

fn system_font_environment(
    toolchain: &ResolvedToolchain,
    texmf: &Path,
    build_root: &Path,
    working_directory: &Path,
) -> [(OsString, OsString); 2] {
    let locked_fonts = texmf.join("fonts");
    let mut font_roots = vec![locked_fonts.as_path()];
    font_roots.extend(
        toolchain
            .engine_roots
            .iter()
            .filter(|root| {
                root.file_name()
                    .and_then(OsStr::to_str)
                    .is_some_and(|name| name.eq_ignore_ascii_case("fonts"))
            })
            .map(PathBuf::as_path),
    );
    [
        (
            OsString::from("OSFONTDIR"),
            search_path_from(&font_roots, working_directory),
        ),
        (
            OsString::from("FONTCONFIG_FILE"),
            engine_path_from(&build_root.join("fontconfig.conf"), working_directory),
        ),
    ]
}
