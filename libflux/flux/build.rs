extern crate fluxcore;

use std::{
    env::{self, consts},
    fs,
    io::Write,
    path::{self, Path},
};

use fluxcore::semantic::bootstrap;
use fluxcore::semantic::env::Environment;
use fluxcore::semantic::flatbuffers::types as fb;
use fluxcore::semantic::sub::Substitutable;

use anyhow::{bail, Result};
use walkdir::WalkDir;

fn serialize<'a, T, S, F>(ty: T, f: F, path: &path::Path) -> Result<()>
where
    F: Fn(&mut flatbuffers::FlatBufferBuilder<'a>, T) -> flatbuffers::WIPOffset<S>,
{
    let mut builder = flatbuffers::FlatBufferBuilder::new();
    let buf = fb::serialize(&mut builder, ty, f);
    let mut file = fs::File::create(path)?;
    file.write_all(buf)?;
    Ok(())
}

// Produce OS specific relative path to the stdlib.
fn stdlib_relative_path() -> &'static str {
    if consts::OS == "windows" {
        "..\\..\\stdlib"
    } else {
        "../../stdlib"
    }
}

// Iterate through each all files and canonicalize the
// file path to an absolute path.
// Canonicalize the root path to the absolute directory.
fn canonicalize_all_files(root: &Path) -> Vec<String> {
    let rootpath = std::env::current_dir()
        .unwrap()
        .join(root)
        .canonicalize()
        .unwrap();
    WalkDir::new(rootpath)
        .into_iter()
        .filter_map(|r| r.ok())
        .filter(|r| r.path().is_dir() || (r.path().is_file() && r.path().ends_with(".flux")))
        .map(|r| r.path().to_str().expect("valid path").to_string())
        .collect()
}

fn main() -> Result<()> {
    let dir = path::PathBuf::from(env::var("OUT_DIR")?);

    let stdlib_path = Path::new(stdlib_relative_path());
    // Ensure we rerun the build if the stdlib changes
    for f in canonicalize_all_files(stdlib_path).iter() {
        println!("cargo:rerun-if-changed={}", f);
    }

    let (prelude, imports, _) = bootstrap::infer_stdlib_dir(stdlib_path)?;

    // Validate there aren't any free type variables in the environment
    for (name, ty) in &prelude {
        if !ty.free_vars().is_empty() {
            bail!("found free variables in type of {}: {}", name, ty);
        }
    }
    for (name, ty) in &imports {
        if !ty.free_vars().is_empty() {
            bail!("found free variables in type of package {}: {}", name, ty);
        }
    }

    let path = dir.join("prelude.data");
    serialize(Environment::from(prelude), fb::build_env, &path)?;

    let path = dir.join("stdlib.data");
    serialize(Environment::from(imports), fb::build_env, &path)?;

    Ok(())
}
