//! Build the release images, check them the way a board would, and publish the
//! manifest that says what exists.
//!
//! # Why this is a host tool and stays one
//!
//! The device cannot fetch a GitHub release. That is measured rather than
//! assumed — `crates/firmware/src/heap.rs` carries the arithmetic and
//! `docs/provenance.md` the experiments — and it is why this program exists in
//! the shape it does: everything about a release that needs a network, a
//! certificate store or a clock happens here, on a machine that has all three,
//! and the device is left with the one job it can do well, which is writing a
//! slot and checking what it wrote.
//!
//! So the manifest is not a device-facing API. It is a description of a release
//! that a *person* — or the web UI running in their browser, which does have
//! TLS — reads in order to decide what to install.
//!
//! # What it does, in order
//!
//! 1. Reads the version out of `crates/firmware/Cargo.toml`. The version is
//!    never passed in: `esp_app_desc!()` bakes `CARGO_PKG_VERSION` into the
//!    image, so the manifest and the binary agree by construction rather than
//!    by care, and step 4 proves it.
//! 2. Builds the web UI, because `crates/firmware/build.rs` embeds `ui/dist/`
//!    and it is a build artefact rather than a tracked file.
//! 3. Builds each chip's firmware and turns the ELF into a flashable image with
//!    `espflash save-image`, run from `crates/firmware` so that
//!    `espflash.toml` points it at this project's partition table — which is
//!    also what makes espflash refuse an image too large for the slot.
//! 4. **Runs `somfy_ota::image::Verifier` over each image**: the same streaming
//!    verifier the device runs on an upload, so a release cannot contain an
//!    image a board would refuse. It also cross-checks the chip id and the
//!    app-descriptor version against what this program thinks it built, which
//!    is what catches a stale `target/` directory.
//! 5. Hashes each image and writes `manifest.json`.
//! 6. With `--publish`, attaches the lot to a GitHub release with `gh`.
//!
//! # The two digests, which are not the same number
//!
//! An ESP-IDF image carries a SHA-256 **of itself** in its last thirty-two
//! bytes, and that is the one the bootloader checks and the one
//! `somfy_ota::image::Verifier` now checks on the device. The manifest's
//! `sha256` is the digest of **the whole file including those thirty-two
//! bytes** — what `sha256sum` prints, and what GitHub's own release API
//! publishes per asset. They are different values over different ranges and
//! confusing them is the obvious mistake, so: the manifest's digest is for
//! whoever is holding the file, and the image's own digest is for whoever is
//! about to run it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};
use somfy_ota::image::{Chip, Verifier};

/// One chip this project publishes an image for.
struct Target {
    /// What the manifest, the asset name and `espflash --chip` call it.
    espflash: &'static str,
    /// The Rust target triple.
    triple: &'static str,
    /// The arguments that select this chip's **shipping** feature set.
    ///
    /// Spelled out per chip rather than derived, because they are not derivable:
    /// the ESP32-C3 ships `mqtt,ui` and no more, since `heap.rs` refuses it
    /// `mdns` and `sntp`. **These must agree with the shipping rows of
    /// `.github/workflows/ci.yml`** — a release built from a feature set CI
    /// never builds is a release nobody has linted.
    features: &'static [&'static str],
    /// What the image's `chip_id` has to say, checked through the same enum the
    /// firmware compares against.
    chip: Chip,
}

/// The chips a release carries.
const TARGETS: &[Target] = &[
    Target {
        espflash: "esp32s3",
        triple: "xtensa-esp32s3-none-elf",
        features: &["--features", "chip-s3"],
        chip: Chip::Esp32S3,
    },
    Target {
        espflash: "esp32c3",
        triple: "riscv32imc-unknown-none-elf",
        features: &["--no-default-features", "--features", "chip-c3,mqtt,ui"],
        chip: Chip::Esp32C3,
    },
];

/// The manifest format's own version.
///
/// It is in the file rather than only in this program because the reader is a
/// browser that may be older or newer than the release it is looking at: a
/// consumer that does not recognise the number should say so rather than guess
/// at the fields.
const SCHEMA: u32 = 1;

fn main() {
    if let Err(problem) = run() {
        eprintln!("xtask: {problem}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    let mut publish = false;
    let mut skip_ui = false;
    let mut only: Vec<String> = Vec::new();
    let mut out: Option<PathBuf> = None;

    let mut rest = args.collect::<Vec<_>>().into_iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--publish" => publish = true,
            "--skip-ui" => skip_ui = true,
            "--chip" => only.push(rest.next().ok_or("--chip needs a value")?),
            "--out" => out = Some(PathBuf::from(rest.next().ok_or("--out needs a value")?)),
            other => return Err(format!("unknown argument '{other}'\n\n{USAGE}")),
        }
    }

    match command.as_deref() {
        Some("release") => {}
        Some(other) => return Err(format!("unknown command '{other}'\n\n{USAGE}")),
        None => return Err(USAGE.to_string()),
    }

    let root = repo_root()?;
    let version = version_of(&root.join("crates/firmware/Cargo.toml"))?;
    let repository = repository_of(&root.join("Cargo.toml"))?;
    let out = out.unwrap_or_else(|| root.join("target/release-artifacts"));

    let targets: Vec<&Target> = if only.is_empty() {
        TARGETS.iter().collect()
    } else {
        only.iter()
            .map(|name| {
                TARGETS
                    .iter()
                    .find(|target| target.espflash == name)
                    .ok_or_else(|| format!("no such chip '{name}'"))
            })
            .collect::<Result<_, _>>()?
    };

    std::fs::create_dir_all(&out).map_err(|error| format!("{}: {error}", out.display()))?;
    println!("xtask: somfy-rs {version} for {} chip(s)", targets.len());

    if skip_ui {
        // Said out loud because the image is firmware *and* UI in one file, so
        // a stale `ui/dist/` is a release whose web pages are older than its
        // firmware — and nothing downstream can tell.
        println!("xtask: --skip-ui, so ui/dist/ is whatever was there already");
    } else {
        build_ui(&root)?;
    }

    let mut images = Vec::new();
    for target in targets {
        images.push(build_image(&root, &out, &version, target)?);
    }

    let tag = format!("v{version}");
    let manifest = render_manifest(SCHEMA, &version, &tag, &repository, &images)?;
    let manifest_path = out.join("manifest.json");
    std::fs::write(&manifest_path, &manifest)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    println!("xtask: wrote {}", manifest_path.display());

    if publish {
        publish_release(&out, &tag, &images)?;
    } else {
        println!(
            "xtask: not published. Re-run with --publish to create the '{tag}' release, or \
             attach {} by hand.",
            out.display(),
        );
    }
    Ok(())
}

/// What to type.
const USAGE: &str = "usage: cargo run -p xtask -- release [--chip <esp32s3|esp32c3>]... \
                     [--out <dir>] [--skip-ui] [--publish]";

/// One built, verified, hashed image.
struct Image {
    chip: &'static str,
    asset: String,
    bytes: u64,
    sha256: String,
}

/// The repository root, found from this crate rather than from the shell's idea
/// of where it is.
///
/// `CARGO_MANIFEST_DIR` is `xtask/`, whose parent is the root. Deriving it means
/// `cargo run -p xtask` works from anywhere in the tree, which matters because
/// half of what this program shells out to has to run in a *different*
/// directory to be correct.
fn repo_root() -> Result<PathBuf, String> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} has no parent", manifest.display()))
}

/// Read `version = "…"` out of a manifest's `[package]` section.
///
/// Deliberately not a TOML parser. This reads one key from one file this
/// repository owns, and the alternative — a dependency, in a tool whose whole
/// job is to be reproducible — buys nothing: the failure mode of the naive read
/// is that it finds the wrong `version`, and stopping at the first
/// `[dependencies]`-style header is what rules that out.
fn version_of(manifest: &Path) -> Result<String, String> {
    let text =
        std::fs::read_to_string(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line != "[package]" {
            break;
        }
        if let Some(value) = line.strip_prefix("version") {
            if let Some(quoted) = value.split('"').nth(1) {
                return validated(quoted, "version");
            }
        }
    }
    Err(format!("no [package] version in {}", manifest.display()))
}

/// Read `repository = "…"` out of the workspace manifest, as `owner/repo`.
///
/// The asset URLs in the manifest are built from this, so it is the one string
/// in a release that has to be right and cannot be checked by anything
/// downstream — a wrong owner produces a manifest full of 404s that look
/// exactly like a release that has not finished uploading.
fn repository_of(manifest: &Path) -> Result<String, String> {
    let text =
        std::fs::read_to_string(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("repository"))
        .ok_or_else(|| format!("no repository in {}", manifest.display()))?;
    let url = line
        .split('"')
        .nth(1)
        .ok_or_else(|| format!("unquoted repository in {}", manifest.display()))?;
    let slug = url
        .strip_prefix("https://github.com/")
        .ok_or_else(|| format!("repository '{url}' is not a github.com URL"))?
        .trim_end_matches('/')
        .trim_end_matches(".git");
    if slug.split('/').count() != 2 || slug.split('/').any(str::is_empty) {
        return Err(format!("repository '{url}' is not owner/repo"));
    }
    validated(slug, "repository")
}

/// Refuse anything that would need escaping rather than escaping it.
///
/// Every string this program puts in JSON is a version, a chip name, a hex
/// digest or a URL built from those — none of which has any business carrying a
/// quote, a backslash or a control character. Validating says so; escaping would
/// quietly accept a release tag that breaks whoever reads the file.
fn validated(value: &str, what: &str) -> Result<String, String> {
    let ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | '/');
    if value.is_empty() || !value.chars().all(ok) {
        return Err(format!(
            "{what} '{value}' has characters this tool will not put in a manifest",
        ));
    }
    Ok(value.to_string())
}

/// Build `ui/dist/`, which the firmware image embeds.
fn build_ui(root: &Path) -> Result<(), String> {
    println!("xtask: building the web UI");
    shell("bun", &["install", "--frozen-lockfile"], &root.join("ui"))?;
    shell("bun", &["run", "build"], &root.join("ui"))
}

/// Build one chip, save its image, verify it, and hash it.
fn build_image(root: &Path, out: &Path, version: &str, target: &Target) -> Result<Image, String> {
    let firmware = root.join("crates/firmware");
    println!("xtask: building {} ({})", target.espflash, target.triple);

    let mut args = vec![
        "build",
        "--release",
        "--target",
        target.triple,
        "--bin",
        "firmware",
    ];
    args.extend_from_slice(target.features);
    shell("cargo", &args, &firmware)?;

    let elf = firmware
        .join("target")
        .join(target.triple)
        .join("release/firmware");
    let asset = format!("somfy-rs-{version}-{}.bin", target.espflash);
    let bin = out.join(&asset);

    // Run from `crates/firmware` so `espflash.toml` applies: it is what makes
    // espflash read this project's partition table, and therefore what makes it
    // refuse an image larger than the 0x1F0000 slot. That check is deliberately
    // left here rather than duplicated below — espflash is the tool that will
    // flash it, so its opinion of the slot is the one that matters.
    shell(
        "espflash",
        &[
            "save-image",
            "--chip",
            target.espflash,
            &elf.to_string_lossy(),
            &bin.to_string_lossy(),
        ],
        &firmware,
    )?;

    let bytes = std::fs::read(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;
    verify(&bytes, version, target)?;

    let sha256 = hex(&Sha256::digest(&bytes));
    println!("xtask:   {asset} — {} bytes, sha256 {sha256}", bytes.len(),);
    Ok(Image {
        chip: target.espflash,
        asset,
        bytes: bytes.len() as u64,
        sha256,
    })
}

/// Put the image through the device's own verifier before publishing it.
///
/// `slot_bytes` is the image's own length, so `ImageError::TooLarge` cannot
/// fire here — that refusal belongs to espflash, which has just read the real
/// partition table. What this establishes is everything else the device checks:
/// the magic, the chip id, the segment walk, the checksum and the image's own
/// appended SHA-256.
///
/// It is fed in 256-byte slices for one reason: that is `crate::ota::PAGE_BYTES`
/// on the device, so a fault that only appears at a particular slicing appears
/// here too.
fn verify(bytes: &[u8], version: &str, target: &Target) -> Result<(), String> {
    let mut verifier = Verifier::new(target.chip, bytes.len(), bytes.len())
        .map_err(|error| format!("{}: {error:?}", target.espflash))?;
    for slice in bytes.chunks(256) {
        verifier
            .feed(slice)
            .map_err(|error| format!("{}: {error:?}", target.espflash))?;
    }
    let accepted = verifier
        .finish()
        .map_err(|error| format!("{}: {error:?}", target.espflash))?;

    // **The check that catches a stale `target/`.** Everything above would pass
    // on a perfectly good image built from a different commit; only the
    // descriptor's own version can say that the bytes are the version this
    // manifest is about to claim they are.
    if accepted.version.as_str() != version {
        return Err(format!(
            "{}: the image says version '{}' and this release is '{version}' — the build did \
             not pick up the manifest, or `target/` is stale",
            target.espflash, accepted.version,
        ));
    }
    Ok(())
}

/// Render the manifest.
///
/// Hand-written rather than serialised, for the same reason `somfy-mqtt` writes
/// its payloads by hand: the shape here is a wire format read by a browser, so
/// it is worth seeing it written out, and every value in it has been through
/// [`validated`] or is a number.
fn render_manifest(
    schema: u32,
    version: &str,
    tag: &str,
    repository: &str,
    images: &[Image],
) -> Result<String, String> {
    let mut out = String::new();
    let _ = writeln!(out, "{{");
    let _ = writeln!(out, "  \"schema\": {schema},");
    let _ = writeln!(out, "  \"project\": \"somfy-rs\",");
    let _ = writeln!(out, "  \"version\": \"{version}\",");
    let _ = writeln!(out, "  \"tag\": \"{tag}\",");
    let _ = writeln!(out, "  \"images\": [");
    for (at, image) in images.iter().enumerate() {
        let comma = if at + 1 == images.len() { "" } else { "," };
        let url = format!(
            "https://github.com/{repository}/releases/download/{tag}/{}",
            image.asset
        );
        let _ = writeln!(out, "    {{");
        let _ = writeln!(out, "      \"chip\": \"{}\",", image.chip);
        let _ = writeln!(out, "      \"asset\": \"{}\",", image.asset);
        let _ = writeln!(out, "      \"bytes\": {},", image.bytes);
        let _ = writeln!(out, "      \"sha256\": \"{}\",", image.sha256);
        let _ = writeln!(out, "      \"url\": \"{url}\"");
        let _ = writeln!(out, "    }}{comma}");
    }
    let _ = writeln!(out, "  ]");
    let _ = writeln!(out, "}}");
    Ok(out)
}

/// Create the release and attach every artefact.
///
/// `gh` rather than the REST API by hand: it already holds the credential, and
/// a release-publishing tool that wants a token of its own is a second place for
/// one to leak from.
fn publish_release(out: &Path, tag: &str, images: &[Image]) -> Result<(), String> {
    let mut assets: Vec<String> = images
        .iter()
        .map(|image| out.join(&image.asset).to_string_lossy().into_owned())
        .collect();
    assets.push(out.join("manifest.json").to_string_lossy().into_owned());

    let mut args = vec![
        "release".to_string(),
        "create".to_string(),
        tag.to_string(),
        "--title".to_string(),
        tag.to_string(),
        "--generate-notes".to_string(),
    ];
    args.extend(assets.iter().cloned());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    // `gh release create` fails if the tag already has a release, which is the
    // right default — a release is a thing people have already downloaded — so
    // the fallback is `upload --clobber` on an existing one rather than a
    // silent recreate.
    if shell("gh", &args, out).is_err() {
        println!("xtask: '{tag}' already exists, uploading into it instead");
        let mut args = vec!["release", "upload", tag, "--clobber"];
        args.extend(assets.iter().map(String::as_str));
        shell("gh", &args, out)?;
    }
    Ok(())
}

/// Run a command, inheriting its output, and fail if it does.
///
/// # Why the environment is stripped, which is not optional here
///
/// This program is launched by `cargo run`, and rustup pins the toolchain for
/// everything below it by exporting `RUSTUP_TOOLCHAIN` — plus `RUSTC`, `CARGO`
/// and `RUSTDOC` pointing into that toolchain. Inherited, they defeat the whole
/// point of `crates/firmware/rust-toolchain.toml`: the firmware build would run
/// under the host's `stable` rather than the `esp` fork, and stable has no
/// Xtensa backend. The failure is loud but misleading —
///
/// ```text
/// 'esp32s3' is not a recognized processor for this target (ignoring processor)
/// error[E0463]: can't find crate for `core`
///   = note: the `xtensa-esp32s3-none-elf` target may not be installed
/// ```
///
/// — which reads as a missing target rather than the wrong compiler, and was
/// observed before it was reasoned about. Removing the four lets rustup resolve
/// from the directory again, which is what makes running this identical to
/// typing the commands by hand.
fn shell(program: &str, args: &[&str], cwd: &Path) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("RUSTC")
        .env_remove("RUSTDOC")
        .env_remove("CARGO")
        .status()
        .map_err(|error| format!("could not run '{program}': {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("'{program} {}' failed ({status})", args.join(" ")))
    }
}

/// Lower-case hex, because that is what `sha256sum` prints and what GitHub's
/// own asset `digest` field carries — so the three can be compared by eye.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_names_every_image_and_builds_its_url_from_the_tag() {
        // Arrange
        let images = vec![
            Image {
                chip: "esp32s3",
                asset: "somfy-rs-0.2.0-esp32s3.bin".to_string(),
                bytes: 1_276_112,
                sha256: "ab".repeat(32),
            },
            Image {
                chip: "esp32c3",
                asset: "somfy-rs-0.2.0-esp32c3.bin".to_string(),
                bytes: 1_100_000,
                sha256: "cd".repeat(32),
            },
        ];

        // Act
        let manifest = render_manifest(1, "0.2.0", "v0.2.0", "owner/somfy-rs", &images).unwrap();

        // Assert
        assert!(manifest.contains("\"schema\": 1"));
        assert!(manifest.contains("\"version\": \"0.2.0\""));
        assert!(manifest.contains(
            "\"url\": \"https://github.com/owner/somfy-rs/releases/download/v0.2.0/\
             somfy-rs-0.2.0-esp32s3.bin\""
        ));
        // The separator between array members is the thing a hand-written
        // renderer gets wrong, and it is invisible until something parses it.
        assert_eq!(manifest.matches("\"chip\"").count(), 2);
        assert!(!manifest.contains("},\n  ]"));
    }

    #[test]
    fn a_manifest_this_tool_writes_is_json_a_reader_can_split_on() {
        // Arrange: there is no JSON parser in this crate on purpose, so the
        // check that the output is well formed is structural — every brace and
        // bracket balances, and no value carries a character that would need
        // escaping.
        let images = vec![Image {
            chip: "esp32s3",
            asset: "somfy-rs-0.2.0-esp32s3.bin".to_string(),
            bytes: 1,
            sha256: "00".repeat(32),
        }];

        // Act
        let manifest = render_manifest(1, "0.2.0", "v0.2.0", "o/r", &images).unwrap();

        // Assert
        assert_eq!(
            manifest.matches('{').count(),
            manifest.matches('}').count(),
            "braces do not balance:\n{manifest}",
        );
        assert_eq!(manifest.matches('[').count(), manifest.matches(']').count());
        assert!(!manifest.contains('\\'), "an escape reached the output");
        assert_eq!(manifest.matches('"').count() % 2, 0);
    }

    #[test]
    fn a_version_that_would_need_escaping_is_refused_rather_than_escaped() {
        // Arrange + Act + Assert
        for bad in ["0.1.0\"", "0.1.0\\", "0.1 0", "", "0.1.0\n"] {
            assert!(
                validated(bad, "version").is_err(),
                "'{bad}' was accepted into a manifest",
            );
        }
        assert_eq!(validated("1.2.3-rc.1", "version").unwrap(), "1.2.3-rc.1");
    }

    #[test]
    fn the_chips_this_tool_publishes_are_the_chips_the_firmware_verifier_knows() {
        // Arrange + Act + Assert: a target whose `chip` disagreed with its
        // `espflash` name would build one chip's image and check it against
        // another's id, which is exactly the mistake the verifier exists to
        // catch and the one place it could be miswired.
        for target in TARGETS {
            assert_eq!(
                target.chip.name(),
                target.espflash,
                "the target table names one chip two ways",
            );
        }
    }

    #[test]
    fn hex_is_lower_case_and_two_characters_a_byte() {
        // Arrange + Act + Assert
        assert_eq!(hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
    }
}
