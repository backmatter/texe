use std::ffi::OsString;
use std::fs;
use std::path::Path;

use crate::TexeError;
use crate::atomic::write as atomic_write;
use crate::build::environment::{EngineEnvironmentContext, engine_environment};
use crate::build::filesystem::job_stem;
use crate::build::format::{self, ManagedFormat};
use crate::build::process::{engine_path_from, raw_engine_output, raw_output};
use crate::build::warnings::remove_if_file;
use crate::build::{BuildContext, EngineRun};
use crate::config::GeneratedInput;

impl BuildContext<'_> {
    pub(super) fn run_engine(
        &self,
        output_dir: &Path,
        managed_format: Option<&ManagedFormat>,
        discovery: bool,
        halt_on_error: bool,
    ) -> Result<EngineRun, TexeError> {
        let stem = job_stem(&self.manifest.project.entry)?;
        let log_path = output_dir.join(format!("{stem}.log"));
        let fls_path = output_dir.join(format!("{stem}.fls"));
        remove_if_file(&log_path)?;
        remove_if_file(&fls_path)?;
        self.materialize_generated_inputs(output_dir)?;
        if self.toolchain.managed.is_none() && managed_format.is_some() {
            write_system_fontconfig(output_dir, &self.texmf)?;
        }

        let mut arguments = Vec::new();
        if managed_format.is_some() {
            let format_name = format::name(&self.toolchain.engine)?;
            arguments.extend([
                OsString::from(format!("-fmt={format_name}")),
                OsString::from(format!("-progname={format_name}")),
            ]);
        }
        arguments.extend(engine_interaction_arguments(halt_on_error));
        arguments.extend([
            OsString::from("-recorder"),
            OsString::from("-synctex=1"),
            // Restricted shell escape would otherwise let a document reach host
            // binaries, which no locked toolchain can account for.
            OsString::from(if self.manifest.toolchain.shell_escape {
                "-shell-escape"
            } else {
                "-no-shell-escape"
            }),
            prefixed_engine_path("-output-directory=", output_dir, self.project_root),
            engine_path_from(&self.entry, self.project_root),
        ]);
        let environment = engine_environment(
            self.toolchain,
            &EngineEnvironmentContext {
                working_directory: self.project_root,
                texmf: &self.texmf,
                build_root: output_dir,
                input_roots: &self.manifest.inputs.roots,
                managed_format,
                discovery,
                source_date_epoch: self.timestamp.effective,
                shell_escape: self.manifest.toolchain.shell_escape,
            },
        );
        let output = if self.toolchain.managed.is_some() || managed_format.is_some() {
            raw_engine_output(
                &self.toolchain.engine_executable,
                &arguments,
                self.project_root,
                &environment,
            )?
        } else {
            raw_output(
                &self.toolchain.engine_executable,
                &arguments,
                self.project_root,
                &environment,
            )?
        };
        Ok(EngineRun {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
            log_path,
            fls_path,
        })
    }

    fn materialize_generated_inputs(&self, output_dir: &Path) -> Result<(), TexeError> {
        write_generated_inputs(output_dir, &self.manifest.project.generated)
    }
}

fn prefixed_engine_path(prefix: &str, path: &Path, working_directory: &Path) -> OsString {
    let mut argument = OsString::from(prefix);
    argument.push(engine_path_from(path, working_directory));
    argument
}

pub(super) fn write_generated_inputs(
    output_dir: &Path,
    inputs: &[GeneratedInput],
) -> Result<(), TexeError> {
    let generated_root = output_dir.join(".texe-generated");
    match fs::symlink_metadata(&generated_root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            fs::remove_file(&generated_root).map_err(|source| TexeError::Io {
                path: generated_root.clone(),
                source,
            })?;
        }
        Ok(_) => {
            fs::remove_dir_all(&generated_root).map_err(|source| TexeError::Io {
                path: generated_root.clone(),
                source,
            })?;
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(TexeError::Io {
                path: generated_root,
                source,
            });
        }
    }
    for input in inputs {
        atomic_write(&generated_root.join(&input.path), input.content.as_bytes())?;
    }
    Ok(())
}

pub(super) fn engine_interaction_arguments(halt_on_error: bool) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("-interaction=nonstopmode"),
        OsString::from("-file-line-error"),
    ];
    // Normal discovery deliberately keeps going after a missing input so one
    // trace can report every dependency reachable in that pass. Verification
    // and final passes stop at the first real error.
    if halt_on_error {
        arguments.push(OsString::from("-halt-on-error"));
    }
    arguments
}

pub(super) fn write_system_fontconfig(output_dir: &Path, texmf: &Path) -> Result<(), TexeError> {
    let cache = output_dir.join("fontconfig-cache");
    fs::create_dir_all(&cache).map_err(|source| TexeError::Io {
        path: cache.clone(),
        source,
    })?;
    let locked_fonts = xml_text(&texmf.join("fonts").to_string_lossy());
    let cache = xml_text(&cache.to_string_lossy());
    let configuration = format!(
        "<?xml version=\"1.0\"?>\n\
         <!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n\
         <fontconfig>\n\
           <include ignore_missing=\"yes\">/etc/fonts/fonts.conf</include>\n\
           <include ignore_missing=\"yes\">/usr/local/etc/fonts/fonts.conf</include>\n\
           <dir>{locked_fonts}</dir>\n\
           <cachedir>{cache}</cachedir>\n\
         </fontconfig>\n"
    );
    atomic_write(
        &output_dir.join("fontconfig.conf"),
        configuration.as_bytes(),
    )
}

fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
