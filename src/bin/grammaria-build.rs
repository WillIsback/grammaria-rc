//! Makes a data directory for the checker from the shipped assets: copies
//! LanguageTool's French resources (`assets/fr/lt`) and this project's rule
//! files (`assets/fr/grammaria`) to `OUT/lt` and `OUT/grammaria`.
//!
//! The engine reads these files as they are: there is nothing to compile.
//! The copy exists so that an application can ship one directory.
//!
//!   grammaria-build [--assets DIR] [--out DIR]

use std::fs;
use std::io;
use std::path::Path;

fn copy_dir(from: &Path, to: &Path) -> io::Result<u64> {
    fs::create_dir_all(to)?;
    let mut bytes = 0;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            bytes += copy_dir(&entry.path(), &target)?;
        } else {
            bytes += fs::copy(entry.path(), &target)?;
        }
    }
    Ok(bytes)
}

fn main() -> io::Result<()> {
    let mut assets = "assets/fr".to_string();
    let mut out = "data".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--assets" => assets = args.next().unwrap_or(assets),
            "--out" => out = args.next().unwrap_or(out),
            "-h" | "--help" => {
                println!("usage: grammaria-build [--assets DIR] [--out DIR]");
                return Ok(());
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let (assets, out) = (Path::new(&assets), Path::new(&out));
    let mut bytes = copy_dir(&assets.join("lt"), &out.join("lt"))?;
    if assets.join("grammaria").is_dir() {
        bytes += copy_dir(&assets.join("grammaria"), &out.join("grammaria"))?;
    }
    println!("{}: {:.1} MiB", out.display(), bytes as f64 / (1024.0 * 1024.0));
    Ok(())
}
