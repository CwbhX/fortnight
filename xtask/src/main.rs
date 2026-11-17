//! `cargo xtask bundle`: wraps the macOS binary in a minimal `Fortnight.app`
//! (PLAN.md section 3.6). std only, no dependencies.
//!
//! It builds with the `cargo` that runs it (`$CARGO`) and `--locked`, into
//! `$CARGO_TARGET_DIR` if set, else `target/`. The bundle itself always goes
//! to `target/bundle/` in the workspace.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  bundle [--universal]   build the release binary and wrap it in
                         target/bundle/Fortnight.app (macOS only).
                         --universal builds aarch64 and x86_64 and joins
                         them with lipo.";

const BUNDLE_ID: &str = "app.fortnight.Fortnight";
const APP_TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("bundle") => bundle(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(message) = result {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

type Result<T> = std::result::Result<T, String>;

fn bundle(args: &[String]) -> Result<()> {
    let mut universal = false;
    for arg in args {
        match arg.as_str() {
            "--universal" => universal = true,
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    if !cfg!(target_os = "macos") {
        return Err("`bundle` makes a macOS .app and only runs on macOS. \
                    On other platforms use `cargo build --release --locked`."
            .into());
    }

    let root = workspace_root();
    let manifest = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("reading workspace Cargo.toml: {e}"))?;
    let version = workspace_version(&manifest)
        .ok_or("no `version` under [workspace.package] in Cargo.toml")?;

    let cwd = std::env::current_dir().map_err(|e| format!("reading the current folder: {e}"))?;
    let target = target_dir(std::env::var_os("CARGO_TARGET_DIR"), &cwd, &root);
    let cargo = cargo_program(std::env::var_os("CARGO"));
    let binary = build_binary(&cargo, &root, &target, universal)?;

    let app = root.join("target/bundle/Fortnight.app");
    if app.exists() {
        std::fs::remove_dir_all(&app).map_err(|e| format!("removing old bundle: {e}"))?;
    }
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    for dir in [&macos, &resources] {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    std::fs::copy(&binary, macos.join("Fortnight"))
        .map_err(|e| format!("copying the binary: {e}"))?;

    let icon = match make_icon(
        &root.join("assets/icon-light.svg"),
        &resources.join("Fortnight.icns"),
    ) {
        Ok(()) => true,
        Err(why) => {
            eprintln!("note: no app icon ({why}); the bundle is made without one");
            false
        }
    };
    std::fs::write(contents.join("Info.plist"), info_plist(&version, icon))
        .map_err(|e| format!("writing Info.plist: {e}"))?;

    println!("Bundled {}", app.display());
    println!(
        "The data folder sits beside the .app, not inside it: \
         fortnight-data/ next to Fortnight.app, or use --data-dir / FORTNIGHT_DATA_DIR. \
         fortnight-cli is not in the bundle and uses the folder beside itself, \
         so give it --data-dir to share the app's."
    );
    Ok(())
}

/// Where cargo puts build output: `$CARGO_TARGET_DIR` (relative to the
/// folder `cargo xtask` was run from, as cargo reads it) if set and not
/// empty, else `<root>/target`.
fn target_dir(env: Option<OsString>, cwd: &Path, root: &Path) -> PathBuf {
    match env.filter(|v| !v.is_empty()) {
        Some(dir) => cwd.join(dir),
        None => root.join("target"),
    }
}

/// The cargo that runs xtask (`$CARGO`), else `cargo` from PATH.
fn cargo_program(env: Option<OsString>) -> OsString {
    env.filter(|v| !v.is_empty())
        .unwrap_or_else(|| OsString::from("cargo"))
}

/// `cargo build --release --locked -p fortnight-app`, into `target_dir`
/// (passed explicitly, so the build lands where the bundle looks for it),
/// for `triple` if given.
fn build_command(cargo: &OsStr, root: &Path, target_dir: &Path, triple: Option<&str>) -> Command {
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .args(["build", "--release", "--locked", "-p", "fortnight-app"])
        .arg("--target-dir")
        .arg(target_dir);
    if let Some(triple) = triple {
        command.args(["--target", triple]);
    }
    command
}

/// The executable a release build of the app leaves in `target_dir`.
fn built_binary(target_dir: &Path, triple: Option<&str>) -> PathBuf {
    let dir = match triple {
        Some(triple) => target_dir.join(triple),
        None => target_dir.to_path_buf(),
    };
    dir.join("release").join("Fortnight")
}

/// The folder holding the workspace `Cargo.toml` (xtask lives one level down).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent directory")
        .to_path_buf()
}

/// Builds the GUI and returns the path of the resulting executable.
fn build_binary(cargo: &OsStr, root: &Path, target_dir: &Path, universal: bool) -> Result<PathBuf> {
    if !universal {
        run(&mut build_command(cargo, root, target_dir, None))?;
        return Ok(built_binary(target_dir, None));
    }

    let installed = installed_targets()?;
    for target in APP_TARGETS {
        if !installed.iter().any(|t| t == target) {
            return Err(format!(
                "the Rust target {target} is not installed; run `rustup target add {target}`"
            ));
        }
    }
    let mut outputs = Vec::new();
    for target in APP_TARGETS {
        run(&mut build_command(cargo, root, target_dir, Some(target)))?;
        outputs.push(built_binary(target_dir, Some(target)));
    }
    let joined = target_dir.join("release").join("fortnight-universal");
    let joined_dir = joined.parent().expect("has parent");
    std::fs::create_dir_all(joined_dir)
        .map_err(|e| format!("creating {}: {e}", joined_dir.display()))?;
    run(Command::new("lipo")
        .arg("-create")
        .args(&outputs)
        .arg("-output")
        .arg(&joined))?;
    Ok(joined)
}

fn installed_targets() -> Result<Vec<String>> {
    let out = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map_err(|e| format!("could not run `rustup target list --installed`: {e}"))?;
    if !out.status.success() {
        return Err("`rustup target list --installed` failed".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

fn run(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .map_err(|e| format!("could not run {:?}: {e}", command.get_program()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{:?} failed ({status})", command.get_program()))
    }
}

/// Renders the icon SVG to a PNG with Quick Look, then builds an .icns with
/// `sips` and `iconutil`, all of which ship with macOS. Any failure only means
/// the bundle has no icon.
fn make_icon(svg: &Path, icns: &Path) -> Result<()> {
    let work = std::env::temp_dir().join(format!("fortnight-icon-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = make_icon_in(&work, svg, icns);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn make_icon_in(work: &Path, svg: &Path, icns: &Path) -> Result<()> {
    let quiet = |c: &mut Command| {
        c.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    };
    let mut ql = Command::new("qlmanage");
    ql.args(["-t", "-s", "1024", "-o"]).arg(work).arg(svg);
    quiet(&mut ql);
    run(&mut ql).map_err(|e| format!("qlmanage: {e}"))?;
    // Quick Look names its output after the input: `<file name>.png`.
    let name = svg.file_name().ok_or("the icon path has no file name")?;
    let rendered = work.join(format!("{}.png", name.to_string_lossy()));
    if !rendered.exists() {
        return Err("qlmanage did not render the SVG".into());
    }

    let iconset = work.join("Fortnight.iconset");
    std::fs::create_dir_all(&iconset).map_err(|e| e.to_string())?;
    for (name, px) in iconset_sizes() {
        let mut sips = Command::new("sips");
        sips.args(["-z", &px.to_string(), &px.to_string()])
            .arg(&rendered)
            .arg("--out")
            .arg(iconset.join(name));
        quiet(&mut sips);
        run(&mut sips).map_err(|e| format!("sips: {e}"))?;
    }
    let mut util = Command::new("iconutil");
    util.args(["-c", "icns"]).arg(&iconset).arg("-o").arg(icns);
    quiet(&mut util);
    run(&mut util).map_err(|e| format!("iconutil: {e}"))
}

/// File names and pixel sizes of an .iconset.
fn iconset_sizes() -> Vec<(String, u32)> {
    let mut sizes = Vec::new();
    for base in [16u32, 32, 128, 256, 512] {
        sizes.push((format!("icon_{base}x{base}.png"), base));
        sizes.push((format!("icon_{base}x{base}@2x.png"), base * 2));
    }
    sizes
}

/// The `version` under `[workspace.package]`, from the workspace manifest.
fn workspace_version(manifest: &str) -> Option<String> {
    let mut in_section = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[workspace.package]";
        } else if in_section && let Some(rest) = line.strip_prefix("version") {
            let value = rest.trim_start().strip_prefix('=')?.trim();
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}

fn info_plist(version: &str, with_icon: bool) -> String {
    let icon = if with_icon {
        "\t<key>CFBundleIconFile</key>\n\t<string>Fortnight</string>\n"
    } else {
        ""
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
\t<key>CFBundleName</key>\n\t<string>Fortnight</string>\n\
\t<key>CFBundleDisplayName</key>\n\t<string>Fortnight</string>\n\
\t<key>CFBundleIdentifier</key>\n\t<string>{BUNDLE_ID}</string>\n\
\t<key>CFBundleExecutable</key>\n\t<string>Fortnight</string>\n\
\t<key>CFBundlePackageType</key>\n\t<string>APPL</string>\n\
\t<key>CFBundleVersion</key>\n\t<string>{version}</string>\n\
\t<key>CFBundleShortVersionString</key>\n\t<string>{version}</string>\n\
\t<key>LSMinimumSystemVersion</key>\n\t<string>11.0</string>\n\
\t<key>NSHighResolutionCapable</key>\n\t<true/>\n\
{icon}</dict>\n\
</plist>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comes_from_workspace_package() {
        let manifest = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"1.2.3\"\nedition = \"2024\"\n\n[workspace.dependencies]\nversion = \"9\"\n";
        assert_eq!(workspace_version(manifest).as_deref(), Some("1.2.3"));
    }

    #[test]
    fn missing_version_is_none() {
        assert_eq!(workspace_version("[workspace]\n"), None);
    }

    #[test]
    fn real_manifest_has_a_version() {
        let text = std::fs::read_to_string(workspace_root().join("Cargo.toml")).unwrap();
        assert!(workspace_version(&text).is_some());
    }

    #[test]
    fn plist_has_required_keys() {
        let plist = info_plist("0.1.0", false);
        for needle in [
            "<key>CFBundleName</key>",
            "<string>app.fortnight.Fortnight</string>",
            "<key>CFBundleExecutable</key>\n\t<string>Fortnight</string>",
            "<string>APPL</string>",
            "<key>CFBundleShortVersionString</key>\n\t<string>0.1.0</string>",
            "<key>LSMinimumSystemVersion</key>\n\t<string>11.0</string>",
            "<key>NSHighResolutionCapable</key>\n\t<true/>",
        ] {
            assert!(plist.contains(needle), "missing {needle}");
        }
        assert!(!plist.contains("CFBundleIconFile"));
        assert!(plist.ends_with("</dict>\n</plist>\n"));
    }

    #[test]
    fn plist_names_the_icon_only_when_present() {
        assert!(info_plist("0.1.0", true).contains("<key>CFBundleIconFile</key>"));
    }

    #[test]
    fn target_dir_defaults_to_the_workspace_target() {
        let (cwd, root) = (Path::new("/work/sub"), Path::new("/work"));
        assert_eq!(target_dir(None, cwd, root), PathBuf::from("/work/target"));
        assert_eq!(
            target_dir(Some(OsString::new()), cwd, root),
            PathBuf::from("/work/target")
        );
    }

    #[test]
    fn target_dir_follows_cargo_target_dir() {
        let (cwd, root) = (Path::new("/work/sub"), Path::new("/work"));
        assert_eq!(
            target_dir(Some("/elsewhere/out".into()), cwd, root),
            PathBuf::from("/elsewhere/out")
        );
        // Relative to where `cargo xtask` was run, as cargo reads it.
        assert_eq!(
            target_dir(Some("out".into()), cwd, root),
            PathBuf::from("/work/sub/out")
        );
    }

    #[test]
    fn cargo_comes_from_the_environment() {
        assert_eq!(cargo_program(None), OsString::from("cargo"));
        assert_eq!(
            cargo_program(Some(OsString::new())),
            OsString::from("cargo")
        );
        assert_eq!(
            cargo_program(Some("/toolchain/bin/cargo".into())),
            OsString::from("/toolchain/bin/cargo")
        );
    }

    #[test]
    fn builds_are_locked_and_land_in_the_target_dir() {
        let target = Path::new("/elsewhere/out");
        let cargo = OsString::from("/toolchain/bin/cargo");
        let command = build_command(
            &cargo,
            Path::new("/work"),
            target,
            Some("x86_64-apple-darwin"),
        );
        assert_eq!(command.get_program(), "/toolchain/bin/cargo");
        assert_eq!(command.get_current_dir(), Some(Path::new("/work")));
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "build",
                "--release",
                "--locked",
                "-p",
                "fortnight-app",
                "--target-dir",
                "/elsewhere/out",
                "--target",
                "x86_64-apple-darwin",
            ]
        );
        assert_eq!(
            built_binary(target, Some("x86_64-apple-darwin")),
            PathBuf::from("/elsewhere/out/x86_64-apple-darwin/release/Fortnight")
        );
        assert_eq!(
            built_binary(target, None),
            PathBuf::from("/elsewhere/out/release/Fortnight")
        );
    }

    #[test]
    fn iconset_covers_the_standard_sizes() {
        let sizes = iconset_sizes();
        assert_eq!(sizes.len(), 10);
        assert!(sizes.contains(&("icon_512x512@2x.png".to_string(), 1024)));
        assert!(sizes.contains(&("icon_16x16.png".to_string(), 16)));
    }
}
