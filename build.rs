use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

fn main() {
    emit_suite_metadata();
    embed_toolchain_recipes();
}

fn emit_suite_metadata() {
    println!("cargo:rerun-if-changed=suite.lock.toml");
    let target = env::var("TARGET").unwrap_or_else(|_| "unknown-target".to_string());
    println!("cargo:rustc-env=TEXE_BUILD_TARGET={target}");

    let suite = fs::read_to_string("suite.lock.toml").expect("read suite.lock.toml");
    for (key, environment) in [
        ("pqty_version", "TEXE_PQTY_VERSION"),
        ("pqty_capabilities_schema", "TEXE_PQTY_CAPABILITIES"),
    ] {
        let value = suite
            .lines()
            .find_map(|line| {
                line.strip_prefix(&format!("{key} = \""))
                    .and_then(|value| value.strip_suffix('"'))
            })
            .unwrap_or_else(|| panic!("suite.lock.toml is missing {key}"));
        println!("cargo:rustc-env={environment}={value}");
    }
}

fn embed_toolchain_recipes() {
    println!("cargo:rerun-if-changed=toolchains/recipes");

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo provides CARGO_MANIFEST_DIR"),
    );
    let recipe_dir = manifest_dir.join("toolchains/recipes");
    let mut recipes = fs::read_dir(&recipe_dir)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", recipe_dir.display()))
        .map(|entry| {
            entry
                .expect("could not read toolchain recipe directory entry")
                .path()
        })
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .collect::<Vec<_>>();
    recipes.sort();
    assert!(
        !recipes.is_empty(),
        "toolchains/recipes must contain at least one TOML recipe"
    );

    let mut generated = String::from("const RECIPE_DOCUMENTS: &[(&str, &str)] = &[\n");
    for path in recipes {
        println!("cargo:rerun-if-changed={}", path.display());
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("toolchain recipe filename must be UTF-8");
        let path = path.to_str().expect("toolchain recipe path must be UTF-8");
        writeln!(
            &mut generated,
            "    ({filename:?}, include_str!({path:?})),"
        )
        .expect("writing to a String cannot fail");
    }
    generated.push_str("];\n");

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo provides OUT_DIR"))
        .join("toolchain_recipe_documents.rs");
    fs::write(&output, generated)
        .unwrap_or_else(|error| panic!("could not write {}: {error}", output.display()));
}
