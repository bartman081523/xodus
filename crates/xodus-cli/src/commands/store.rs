use std::path::{Path, PathBuf};
use std::process::ExitCode;

use base64::prelude::*;
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use sha1::{Digest, Sha1};
use sha2::Sha256;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use xodus::api::{fe3, store};
use xodus::models::fe3::Fe3File;

use msix::manifest::{
    ActivatableClass, ManifestIdentity, parse_activatable_classes, parse_package_manifest,
    select_arch_application_packages,
};
use msix::opc;
use msix::opc::Container;

const MRT100_STUB_DLL: &[u8] = include_bytes!("../../resources/mrt100.dll");

/// Lists the package files of one Store product category plus their resolved
/// delivery URLs (FE3 protocol, no authentication).
///
/// This is the acceptance harness for the FE3 client: a Store URL or
/// product id is only translated into the category id by `store download`,
/// so this command takes the product's `WuCategoryId` directly (see the
/// `FulfillmentData` of the product endpoint).
pub async fn run_urls(
    client: &reqwest::Client,
    wu_category_id: String,
    release: String,
) -> ExitCode {
    let cookie = match fe3::get_cookie(client).await {
        Ok(cookie) => cookie,
        Err(e) => {
            eprintln!("FE3 GetCookie failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let files = match fe3::sync_updates(client, &cookie, &wu_category_id, &release).await {
        Ok(files) => files,
        Err(e) => {
            eprintln!("FE3 SyncUpdates failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    if files.is_empty() {
        eprintln!("No resolvable package files for category {wu_category_id}");
        return ExitCode::FAILURE;
    }

    let mut failed = 0usize;
    for file in &files {
        println!(
            "{}: {} ({} bytes, revision {})",
            file.package_identity_name, file.file_name, file.size, file.revision_number
        );
        match fe3::file_urls(client, file, &release).await {
            Ok(urls) => {
                for url in urls {
                    println!("  {url}");
                }
            }
            Err(e) => {
                failed += 1;
                eprintln!("  URL resolution failed: {e}");
            }
        }
    }
    if failed > 0 {
        eprintln!("{failed} of {} files could not be resolved", files.len());
        return ExitCode::FAILURE;
    }
    println!("{len} files resolved", len = files.len());
    ExitCode::SUCCESS
}

/// Downloads a Store product's main package and its dependencies as plain
/// files (FE3 protocol, no authentication).
///
/// A Store page URL or bare product id is resolved to the product's
/// `WuCategoryId` through the `storeedgefd` product endpoint, the FE3
/// round trips enumerate and resolve the package files, and one file per
/// package (main package plus dependencies, architecture-preferred) is
/// downloaded and placed in `destination`.
///
/// Downloads are verified while streaming: every file is hashed with SHA1
/// and compared against FE3's reported digest, and the written byte count
/// is compared against FE3's reported size; a mismatch fails the command.
pub async fn run_download(
    client: &reqwest::Client,
    product: String,
    destination: Option<String>,
    market: String,
    release: String,
    dry_run: bool,
) -> ExitCode {
    let Some(product_id) = store::product_id_from_page(&product) else {
        eprintln!("Could not derive a Store product id from '{product}'");
        return ExitCode::FAILURE;
    };

    let fulfillment = match store::find_product_fulfillment_data(client, &product_id, &market).await
    {
        Ok(fulfillment) => fulfillment,
        Err(e) => {
            eprintln!("Store product lookup failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let package_family_name = fulfillment.package_family_name;
    println!(
        "Product {product_id}: {package_family_name} (WuCategoryId {})",
        fulfillment.wu_category_id
    );

    let cookie = match fe3::get_cookie(client).await {
        Ok(cookie) => cookie,
        Err(e) => {
            eprintln!("FE3 GetCookie failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let files =
        match fe3::sync_updates(client, &cookie, &fulfillment.wu_category_id, &release).await {
            Ok(files) => files,
            Err(e) => {
                eprintln!("FE3 SyncUpdates failed: {e}");
                return ExitCode::FAILURE;
            }
        };
    let selection = store::select_download_set(&files, &package_family_name, host_arch());
    if selection.is_empty() {
        eprintln!("No resolvable package files for {package_family_name}");
        return ExitCode::FAILURE;
    }

    let mut resolved = Vec::with_capacity(selection.len());
    for file in &selection {
        match fe3::file_url(client, file, &release).await {
            Ok(Some(url)) => resolved.push((file, url)),
            Ok(None) => {
                eprintln!(
                    "No payload URL for {} ({})",
                    file.file_name, file.installer_specific_identifier
                );
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("URL resolution failed for {}: {e}", file.file_name);
                return ExitCode::FAILURE;
            }
        }
    }

    if dry_run {
        for (file, url) in &resolved {
            println!(
                "{}: {} ({} bytes, revision {})",
                file.package_identity_name, file.file_name, file.size, file.revision_number
            );
            println!("  {url}");
        }
        println!("{len} files resolved", len = resolved.len());
        return ExitCode::SUCCESS;
    }

    let directory = destination.unwrap_or_else(|| ".".to_owned());
    if let Err(e) = tokio::fs::create_dir_all(&directory).await {
        eprintln!("Could not create destination directory '{directory}': {e}");
        return ExitCode::FAILURE;
    }
    for (file, url) in &resolved {
        if let Err(e) = download_file(client, file, url, &directory).await {
            eprintln!("Download of {} failed: {e}", file.file_name);
            return ExitCode::FAILURE;
        }
    }
    println!(
        "{len} files downloaded to {directory}",
        len = resolved.len()
    );
    ExitCode::SUCCESS
}

async fn download_file(
    client: &reqwest::Client,
    file: &Fe3File,
    url: &str,
    directory: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let progress = ProgressBar::new(file.size).with_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}) ({eta})",
        )
        .unwrap()
        .progress_chars("#>-"),
    );

    let response = client.get(url).send().await?;
    let path = Path::new(directory).join(&file.file_name);
    let mut target = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .await?;
    let mut stream = response.bytes_stream();
    let mut hasher = Sha1::new();
    let mut downloaded: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        target.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        progress.inc(chunk.len() as u64);
    }
    progress.finish();
    target.flush().await?;
    // FE3's reported `File@Size` and base64 SHA1 `File@Digest` both verify
    // a full, untruncated, uncorrupted transfer; mismatches fail loudly
    // instead of leaving a container that extraction would reject later.
    verify_transfer(file, downloaded, hasher.finalize().as_slice())?;
    Ok(())
}

/// Verifies a streamed file against FE3's reported size and digest.
///
/// Size first (cheap), then the digest: FE3 reports base64-encoded SHA1
/// digests plus the algorithm, so an unknown algorithm cannot be verified
/// and fails instead of passing silently.
fn verify_transfer(file: &Fe3File, downloaded: u64, digest: &[u8]) -> Result<(), String> {
    if downloaded != file.size {
        return Err(format!(
            "wrote {downloaded} bytes, FE3 reported {} bytes",
            file.size
        ));
    }
    verify_digest(file, digest)
}

fn verify_digest(file: &Fe3File, digest: &[u8]) -> Result<(), String> {
    const SHA1: &str = "SHA1";
    if !file.digest_algorithm.eq_ignore_ascii_case(SHA1) {
        return Err(format!(
            "FE3 reported unverifiable digest algorithm '{}' for {}",
            file.digest_algorithm, file.file_name
        ));
    }
    let computed = BASE64_STANDARD.encode(digest);
    if computed == file.digest {
        Ok(())
    } else {
        Err(format!(
            "digest mismatch for {}: FE3 reported {}, computed {computed}",
            file.file_name, file.digest
        ))
    }
}

/// Extracts an MSIX bundle or package container into `destination`.
///
/// A bundle is recognized by its `AppxMetadata/AppxBundleManifest.xml`;
/// its application package for the target architecture (default: the host
/// architecture, overridable with `--arch`) is extracted by file name,
/// resource packages are skipped. Any other container counts as single
/// package and extracts its whole OPC tree. Reads go through the archive's
/// central directory with CRC checking (the containers' local headers are
/// unusable: ZIP64 data descriptors with zeroed sizes).
pub fn run_extract(package: String, destination: Option<String>, arch: Option<String>) -> ExitCode {
    let destination = destination.unwrap_or_else(|| ".".to_owned());
    let mut container = match open_container(&package) {
        Ok(container) => container,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    if container.has_bundle_manifest() {
        extract_bundle(&mut container, &package, &destination, arch.as_deref())
    } else {
        extract_package(&mut container, &destination)
    }
}

/// Extracts a whole package plus its dependency packages into one
/// directory and reports the install metadata of the merged tree.
///
/// The main package extracts first; dependencies merge with
/// skip-if-exists semantics (existing files win), so overlapping
/// framework DLLs are not clobbered or duplicated and a dependency's
/// own `AppxManifest.xml` cannot displace the main package's manifest.
/// The report afterwards comes from the main `AppxManifest.xml`: identity,
/// applications (entry point), package dependencies and target device
/// families — what Windows would register on installation.
pub fn run_install(
    package: String,
    destination: String,
    mut dependencies: Vec<String>,
    dependencies_dir: Option<String>,
) -> ExitCode {
    if let Some(dir) = &dependencies_dir {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if (ext == "appx" || ext == "msix") && path.to_string_lossy() != package {
                        dependencies.push(path.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }
    if dependencies.is_empty() {
        if let Some(parent) = Path::new(&package).parent() {
            let parent_dir = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            if let Ok(entries) = std::fs::read_dir(parent_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Some(ext) = path.extension() {
                        if (ext == "appx" || ext == "msix") && path.to_string_lossy() != package {
                            dependencies.push(path.to_string_lossy().into_owned());
                        }
                    }
                }
            }
        }
    }
    if !dependencies.is_empty() {
        println!("Resolved {} dependency package(s)", dependencies.len());
    }

    let mut container = match open_container(&package) {
        Ok(container) => container,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&destination) {
        eprintln!("Could not create destination directory '{destination}': {e}");
        return ExitCode::FAILURE;
    }

    if container.has_bundle_manifest() {
        let arch = host_arch();
        let manifest = match container.read_bundle_manifest() {
            Ok(manifest) => manifest,
            Err(e) => {
                eprintln!("Could not read bundle manifest of {package}: {e}");
                return ExitCode::FAILURE;
            }
        };
        let selected = select_arch_application_packages(&manifest, arch);
        if selected.is_empty() {
            eprintln!("Bundle {package} carries no application package for {arch}");
            return ExitCode::FAILURE;
        }
        for entry in &selected {
            println!("Extracting bundled application package: {}", entry.file_name);
            let mut temp_file = match tempfile::tempfile() {
                Ok(file) => file,
                Err(e) => {
                    eprintln!("Could not create temp file for bundled package: {e}");
                    return ExitCode::FAILURE;
                }
            };
            if let Err(e) = container.copy_entry(&entry.file_name, &mut temp_file) {
                eprintln!("Could not extract bundled package {}: {e}", entry.file_name);
                return ExitCode::FAILURE;
            }
            let mut app_container = match Container::open(temp_file) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Could not open bundled package {}: {e}", entry.file_name);
                    return ExitCode::FAILURE;
                }
            };
            match app_container.extract_all(Path::new(&destination)) {
                Ok(ext) => println!(
                    "Extracted {files} files ({bytes} bytes) from {name}",
                    files = ext.files,
                    bytes = ext.bytes,
                    name = entry.file_name
                ),
                Err(e) => {
                    eprintln!("Extraction of {name} failed: {e}", name = entry.file_name);
                    return ExitCode::FAILURE;
                }
            }
        }
    } else {
        let main_extraction = match container.extract_all(Path::new(&destination)) {
            Ok(extraction) => extraction,
            Err(e) => {
                eprintln!("Extraction of {package} failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        println!(
            "Main package {package}: {files} files ({bytes} bytes) extracted to {destination}",
            files = main_extraction.files,
            bytes = main_extraction.bytes
        );
    }

    for dependency in &dependencies {
        let mut dep_container = match open_container(dependency) {
            Ok(container) => container,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        };
        let total = dep_container.len();
        match dep_container.merge_into(Path::new(&destination)) {
            Ok(merged) => println!(
                "Dependency {dependency}: copied {merged} of {total} archive entries ({bytes} bytes)",
                merged = merged.files,
                bytes = merged.bytes
            ),
            Err(e) => {
                eprintln!("Merge of {dependency} failed: {e}");
                return ExitCode::FAILURE;
            }
        }
        match persist_dependency_manifest(&mut dep_container, Path::new(&destination), dependency) {
            Ok(Some(path)) => println!("  Manifest: {}", path.display()),
            Ok(None) => {}
            Err(e) => eprintln!("  {e}"),
        }
    }

    let report_code = report_install_manifest(&destination);
    if report_code != ExitCode::SUCCESS {
        return report_code;
    }

    if let Err(e) = setup_app_environment(Path::new(&destination)) {
        eprintln!("Warning: could not complete launcher setup: {e}");
    }

    ExitCode::SUCCESS
}

fn setup_app_environment(destination: &Path) -> Result<(), String> {
    let manifest_path = destination.join(opc::MANIFEST_PATH);
    let manifest_xml = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("Could not read manifest {}: {e}", manifest_path.display()))?;
    let manifest = parse_package_manifest(&manifest_xml)
        .map_err(|e| format!("Could not parse manifest: {e}"))?;

    // 1. Install .NET Native runtime stub if required
    let needs_mrt = destination.join("mrt100_app.dll").exists()
        || manifest
            .package_dependencies
            .iter()
            .any(|d| d.name.contains("Microsoft.NET.Native"));

    if needs_mrt && !destination.join("mrt100.dll").exists() {
        let mrt_path = destination.join("mrt100.dll");
        std::fs::write(&mrt_path, MRT100_STUB_DLL)
            .map_err(|e| format!("Could not write {}: {e}", mrt_path.display()))?;
        println!("  Installed .NET Native runtime bridge: mrt100.dll");
    }

    // 2. Pre-seed WinRT class registry
    let (classes, _) = collect_activatable_classes(destination)?;
    if !classes.is_empty() {
        let reg_content = winrt_registry_file(&classes, destination);
        let reg_path = destination.join("AppxMetadata").join("xodus-winrt.reg");
        if let Some(parent) = reg_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        write_utf16_file(&reg_path, &reg_content).map_err(|e| format!("{e}"))?;
        println!(
            "  Seeded {} activatable classes into {}",
            classes.len(),
            reg_path.display()
        );
    }

    // 3. Pre-seed AppModel packages registry
    let (packages, _) = collect_app_model_packages(destination, &manifest)?;
    let appmodel_reg_path = destination.join("AppxMetadata").join("xodus-appmodel.reg");
    if let Some(parent) = appmodel_reg_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    write_utf16_file(
        &appmodel_reg_path,
        &appmodel_registry_file(&packages, destination),
    )
    .map_err(|e| format!("{e}"))?;
    println!(
        "  Seeded {} AppModel packages into {}",
        packages.len(),
        appmodel_reg_path.display()
    );

    // 4. Generate executable launch.sh
    let entry = choose_entry_executable(&manifest, None)?;
    let entry_path = entry.replace('\\', "/");
    let launcher_content = generate_launcher_script(&entry_path);
    let launcher_path = destination.join("launch.sh");
    std::fs::write(&launcher_path, launcher_content)
        .map_err(|e| format!("Could not write {}: {e}", launcher_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&launcher_path, std::fs::Permissions::from_mode(0o755));
    }

    println!("  Generated executable launcher: {}", launcher_path.display());
    println!(
        "\nApplication ready! To launch directly:\n  {}",
        launcher_path.display()
    );

    Ok(())
}

fn generate_launcher_script(entry_exe: &str) -> String {
    format!(
        r#"#!/bin/bash
set -e

# Directory where this script and the application reside
APP_DIR="$(cd "$(dirname "${{BASH_SOURCE[0]}}")" && pwd)"

# Allow user to specify WINE and WINEPREFIX, or default to reasonable paths
WINE="${{WINE:-wine}}"
WINEPREFIX="${{WINEPREFIX:-$APP_DIR/.xodus-wine}}"
export WINEPREFIX

# Ensure display is available
if [ -z "$DISPLAY" ] && [ -z "$WAYLAND_DISPLAY" ]; then
    export DISPLAY=:0
fi

# Initialize Wine prefix if needed
if [ ! -f "$WINEPREFIX/system.reg" ]; then
    echo "Initializing Wine prefix in $WINEPREFIX..."
    "$WINE" wineboot --init
    "$WINE" reg add "HKCU\\Software\\Wine\\WineDbg" /v ShowCrashDialog /t REG_DWORD /d 0 /f >/dev/null 2>&1 || true
fi

# Seed WinRT Activatable Classes
if [ -f "$APP_DIR/AppxMetadata/xodus-winrt.reg" ]; then
    "$WINE" regedit /S "Z:${{APP_DIR//\//\\}}\\AppxMetadata\\xodus-winrt.reg" >/dev/null 2>&1 || \
    "$WINE" regedit /S "$APP_DIR/AppxMetadata/xodus-winrt.reg" >/dev/null 2>&1 || true
fi

# Seed AppModel Packages table
if [ -f "$APP_DIR/AppxMetadata/xodus-appmodel.reg" ]; then
    "$WINE" regedit /S "Z:${{APP_DIR//\//\\}}\\AppxMetadata\\xodus-appmodel.reg" >/dev/null 2>&1 || \
    "$WINE" regedit /S "$APP_DIR/AppxMetadata/xodus-appmodel.reg" >/dev/null 2>&1 || true
fi

# Launch application
echo "Starting {entry_exe} with $WINE..."
cd "$APP_DIR"
exec "$WINE" "$APP_DIR/{entry_exe}" "$@"
"#
    )
}

fn extract_bundle(
    container: &mut Container<std::fs::File>,
    package: &str,
    destination: &str,
    arch: Option<&str>,
) -> ExitCode {
    let arch = arch.unwrap_or(host_arch()).to_owned();
    let manifest = match container.read_bundle_manifest() {
        Ok(manifest) => manifest,
        Err(e) => {
            eprintln!("Could not read the bundle manifest of {package}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let available: Vec<&str> = manifest
        .packages
        .iter()
        .filter(|entry| entry.package_type.eq_ignore_ascii_case("application"))
        .filter_map(|entry| entry.architecture.as_deref())
        .collect();
    let selected = select_arch_application_packages(&manifest, &arch);
    if selected.is_empty() {
        eprintln!(
            "Bundle {package} carries no application package for architecture {arch} (candidates: {})",
            available.join(", ")
        );
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::create_dir_all(destination) {
        eprintln!("Could not create destination directory '{destination}': {e}");
        return ExitCode::FAILURE;
    }
    for entry in &selected {
        let target = Path::new(destination).join(&entry.file_name);
        let mut target_file = match std::fs::File::create(&target) {
            Ok(file) => file,
            Err(e) => {
                eprintln!("Could not create {}: {e}", target.display());
                return ExitCode::FAILURE;
            }
        };
        match container.copy_entry(&entry.file_name, &mut target_file) {
            Ok(bytes) => println!("  {} ({bytes} bytes)", entry.file_name),
            Err(e) => {
                eprintln!("Could not extract {}: {e}", entry.file_name);
                return ExitCode::FAILURE;
            }
        }
    }
    println!(
        "Bundle {package}: {count} application package(s) for {arch} extracted to {destination}",
        count = selected.len()
    );
    ExitCode::SUCCESS
}

fn extract_package(container: &mut Container<std::fs::File>, destination: &str) -> ExitCode {
    if let Err(e) = std::fs::create_dir_all(destination) {
        eprintln!("Could not create destination directory '{destination}': {e}");
        return ExitCode::FAILURE;
    }
    match container.extract_all(Path::new(destination)) {
        Ok(extraction) => {
            println!(
                "{files} files ({bytes} bytes) extracted to {destination}",
                files = extraction.files,
                bytes = extraction.bytes
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Extraction failed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn report_install_manifest(destination: &str) -> ExitCode {
    let manifest_path = Path::new(destination).join(opc::MANIFEST_PATH);
    let manifest_xml = match std::fs::read_to_string(&manifest_path) {
        Ok(xml) => xml,
        Err(e) => {
            eprintln!(
                "Could not read the extracted manifest {}: {e}",
                manifest_path.display()
            );
            return ExitCode::FAILURE;
        }
    };
    let manifest = match parse_package_manifest(&manifest_xml) {
        Ok(manifest) => manifest,
        Err(e) => {
            eprintln!("Could not parse the extracted manifest: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("Install report ({})", manifest_path.display());
    println!(
        "  Identity: {name} {version} ({publisher})",
        name = manifest.identity.name,
        version = manifest.identity.version,
        publisher = manifest.identity.publisher
    );
    if let Some(arch) = &manifest.identity.processor_architecture {
        println!("  Architecture: {arch}");
    }
    for application in &manifest.applications {
        println!(
            "  Application {id}: {executable} (EntryPoint {entry})",
            id = application.id,
            executable = application.executable,
            entry = application.entry_point.as_deref().unwrap_or("none")
        );
    }
    for dependency in &manifest.package_dependencies {
        match &dependency.publisher {
            Some(publisher) => println!(
                "  PackageDependency {name} (MinVersion {version}, Publisher {publisher})",
                name = dependency.name,
                version = dependency.min_version
            ),
            None => println!(
                "  PackageDependency {name} (MinVersion {version})",
                name = dependency.name,
                version = dependency.min_version
            ),
        }
    }
    for family in &manifest.target_device_families {
        println!(
            "  TargetDeviceFamily {name}: MinVersion {min}, MaxVersionTested {max}",
            name = family.name,
            min = family.min_version,
            max = family.max_version_tested
        );
    }
    ExitCode::SUCCESS
}

fn open_container(path: &str) -> Result<Container<std::fs::File>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Could not open '{path}': {e}"))?;
    Container::open(file).map_err(|e| format!("Could not read OPC container '{path}': {e}"))
}

/// Persists a dependency package's own `AppxManifest.xml` under
/// `AppxMetadata/dependency-<file stem>.AppxManifest.xml` in the merged
/// tree.
///
/// The merge keeps the main package's `AppxManifest.xml` at the root
/// (`store install` semantics: existing files win), but `store run`
/// seeds WinRT activation from every manifest of the package tree — so
/// each dependency's manifest is copied out under a collision-free name.
/// Returns `Ok(None)` when the dependency carries no manifest at all.
fn persist_dependency_manifest(
    container: &mut Container<std::fs::File>,
    destination: &Path,
    dependency: &str,
) -> Result<Option<PathBuf>, String> {
    let xml = match container.read_entry_str(opc::MANIFEST_PATH) {
        Ok(xml) => xml,
        Err(e) => {
            eprintln!(
                "  Dependency {dependency} carries no readable {path}: {e}",
                path = opc::MANIFEST_PATH
            );
            return Ok(None);
        }
    };
    let stem = Path::new(dependency)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("dependency-{len}", len = container.len()));
    let metadata_dir = destination.join("AppxMetadata");
    std::fs::create_dir_all(&metadata_dir)
        .map_err(|e| format!("Could not create {}: {e}", metadata_dir.display()))?;
    let target = metadata_dir.join(format!("dependency-{stem}.AppxManifest.xml"));
    std::fs::write(&target, &xml)
        .map_err(|e| format!("Could not write {}: {e}", target.display()))?;
    Ok(Some(target))
}

/// Runs a Store package tree (as assembled by `store install`) under a
/// wine of the user's choice — a vanilla build or a patched one.
///
/// Steps, all verified in the Phase-4 spike (`scratches/fe3_spike/run_spike/`):
/// 1. Resolve the entry executable from `--exe` or the package manifest's
///    first `Application/@Executable`.
/// 2. Create the prefix on first use with `wineboot --init` and switch
///    off wine's crash dialog (its modal blocks headless runs and stalls
///    script use exactly like a hung application).
/// 3. Seed WinRT activation: every activatable class declared by the
///    main and persisted dependency manifests maps to its in-process
///    server DLL under `HKLM\Software\Microsoft\WindowsRuntime\
///    ActivatableClassId\<Class>` @ `DllPath` — wine combase's registry
///    fallback for class names that are no builtin factories
///    (`dlls/combase/roapi.c`).
/// 4. Seed the package-identity table at `HKCU\Software\Wine\AppModel`:
///    one entry per package of the tree, each derived entirely from its
///    manifest (including the 13-character publisher id, hashed from the
///    publisher string) — the data the patched appmodel code answers
///    identity queries from, so the wine side carries no app specifics.
/// 5. Launch the executable with the package directory as its working
///    directory and wait for wine to exit.
///
/// Documented limit (spike, fake_run8.log): wine 11 implements no ALPC,
/// so WinUI 3 (XAML) apps abort once their CoreMessaging dispatcher
/// falls back to `NtAlpcCreatePort`; the registry seed and the launch
/// are still correct up to that point.
pub async fn run_run(
    directory: String,
    wine: String,
    exe: Option<String>,
    prefix: Option<String>,
) -> ExitCode {
    let directory = match std::fs::canonicalize(&directory) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Could not resolve package directory '{directory}': {e}");
            return ExitCode::FAILURE;
        }
    };
    let manifest_xml = match std::fs::read_to_string(directory.join(opc::MANIFEST_PATH)) {
        Ok(xml) => xml,
        Err(e) => {
            eprintln!(
                "Could not read the package manifest in {}: {e}",
                directory.display()
            );
            return ExitCode::FAILURE;
        }
    };
    let manifest = match parse_package_manifest(&manifest_xml) {
        Ok(manifest) => manifest,
        Err(e) => {
            eprintln!("Could not parse the package manifest: {e}");
            return ExitCode::FAILURE;
        }
    };
    let entry = match choose_entry_executable(&manifest, exe.as_deref()) {
        Ok(entry) => entry,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    // Manifest spellings may be Windows paths; wine accepts forward
    // slashes and resolves relative to the package directory.
    let entry_path = entry.replace('\\', "/");
    if !directory.join(&entry_path).exists() {
        eprintln!(
            "Entry executable {} not found in {}",
            entry,
            directory.display()
        );
        return ExitCode::FAILURE;
    }

    let prefix = prefix
        .map(PathBuf::from)
        .unwrap_or_else(|| directory.join(".xodus-wine"));
    if !prefix.join("system.reg").exists() {
        println!("Initializing wine prefix {} with {wine}", prefix.display());
        match Command::new(&wine)
            .env("WINEPREFIX", &prefix)
            .arg("wineboot")
            .arg("--init")
            .status()
            .await
        {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("wineboot --init failed ({status:?})");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("Could not run wineboot ({wine}): {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    // Crashes of child processes would otherwise open a modal dialog.
    if let Err(e) = Command::new(&wine)
        .env("WINEPREFIX", &prefix)
        .args([
            "reg",
            "add",
            "HKCU\\Software\\Wine\\WineDbg",
            "/v",
            "ShowCrashDialog",
            "/t",
            "REG_DWORD",
            "/d",
            "0",
            "/f",
        ])
        .status()
        .await
    {
        eprintln!("Could not set ShowCrashDialog: {e}");
    }

    let (classes, manifest_count) = match collect_activatable_classes(&directory) {
        Ok(collected) => collected,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if !classes.is_empty() {
        let reg_content = winrt_registry_file(&classes, &directory);
        let reg_path = directory.join("AppxMetadata").join("xodus-winrt.reg");
        if let Err(e) = std::fs::create_dir_all(reg_path.parent().unwrap_or(&directory)) {
            eprintln!("Could not create {}: {e}", reg_path.display());
            return ExitCode::FAILURE;
        }
        if let Err(e) = write_utf16_file(&reg_path, &reg_content) {
            eprintln!("Could not write {}: {e}", reg_path.display());
            return ExitCode::FAILURE;
        }
        println!(
            "Seeded {count} activatable classes from {manifests} manifests",
            count = classes.len(),
            manifests = manifest_count
        );
        let import = Command::new(&wine)
            .env("WINEPREFIX", &prefix)
            .args(["regedit", "/S", &windows_z_path(&reg_path)])
            .status()
            .await;
        match import {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("Registry import failed ({status:?})");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("Could not run regedit ({wine}): {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    let (packages, _) = match collect_app_model_packages(&directory, &manifest) {
        Ok(collected) => collected,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let appmodel_reg_path = directory.join("AppxMetadata").join("xodus-appmodel.reg");
    if let Err(e) = write_utf16_file(
        &appmodel_reg_path,
        &appmodel_registry_file(&packages, &directory),
    ) {
        eprintln!("Could not write {}: {e}", appmodel_reg_path.display());
        return ExitCode::FAILURE;
    }
    println!(
        "Seeded {count} packages into HKCU\\Software\\Wine\\AppModel",
        count = packages.len()
    );
    match Command::new(&wine)
        .env("WINEPREFIX", &prefix)
        .args(["regedit", "/S", &windows_z_path(&appmodel_reg_path)])
        .status()
        .await
    {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!("Registry import failed ({status:?})");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("Could not run regedit ({wine}): {e}");
            return ExitCode::FAILURE;
        }
    }

    println!(
        "Launching {entry} with {wine} (prefix {})",
        prefix.display()
    );
    let child = Command::new(&wine)
        .env("WINEPREFIX", &prefix)
        .current_dir(&directory)
        .arg(&entry_path)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            eprintln!("Could not start {wine}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let pid = child.id().unwrap_or(0);
    ctrlc::set_handler(move || {
        if pid > 0 {
            let _ = kill(Pid::from_raw(pid as i32), Signal::SIGINT);
        }
    })
    .expect("failed to install Ctrl+C handler");
    let status = child.wait().await.unwrap();
    ExitCode::from(status.code().map(|c| c as u8).unwrap_or(0))
}

/// The executable to launch: the explicit `--exe` override or the first
/// `Application/@Executable` of the package manifest.
fn choose_entry_executable(
    manifest: &msix::manifest::PackageManifest,
    exe: Option<&str>,
) -> Result<String, String> {
    if let Some(exe) = exe {
        return Ok(exe.to_owned());
    }
    manifest
        .applications
        .first()
        .map(|application| application.executable.clone())
        .ok_or_else(|| {
            "The package manifest declares no Application entry and no --exe was given".to_owned()
        })
}

/// Every activatable class of a package tree with the count of parsed
/// manifests.
fn collect_activatable_classes(directory: &Path) -> Result<(Vec<ActivatableClass>, usize), String> {
    let paths = collect_manifest_paths(directory)?;
    let manifest_count = paths.len();
    let mut classes = Vec::new();
    for path in &paths {
        let xml = std::fs::read_to_string(path)
            .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        let parsed = parse_activatable_classes(&xml)
            .map_err(|e| format!("Could not parse {}: {e}", path.display()))?;
        classes.extend(parsed);
    }
    Ok((classes, manifest_count))
}

/// The manifests of a package tree in deterministic order: the root
/// `AppxManifest.xml` first, then the persisted dependency manifests
/// (`AppxMetadata/dependency-*.AppxManifest.xml`, sorted for
/// deterministic seeding order).
fn collect_manifest_paths(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let metadata_dir = directory.join("AppxMetadata");
    let mut paths = vec![directory.join(opc::MANIFEST_PATH)];
    if metadata_dir.is_dir() {
        let mut dependencies: Vec<PathBuf> = std::fs::read_dir(&metadata_dir)
            .map_err(|e| format!("Could not read {}: {e}", metadata_dir.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                name.starts_with("dependency-") && name.ends_with(".AppxManifest.xml")
            })
            .collect();
        dependencies.sort();
        paths.extend(dependencies);
    }
    Ok(paths)
}

/// One package identity of the AppModel registry table, derived from a
/// package manifest — no app specifics anywhere.
#[derive(Clone)]
struct AppModelPackage {
    name: String,
    version: String,
    publisher: String,
    publisher_id: String,
    architecture: String,
    app_id: Option<String>,
    is_framework: bool,
}

impl AppModelPackage {
    /// The current (main) package; carries its first application id so
    /// wine can form the user model id.
    fn main(manifest: &msix::manifest::PackageManifest) -> Self {
        Self {
            app_id: manifest.applications.first().map(|a| a.id.clone()),
            is_framework: false,
            ..Self::framework(&manifest.identity)
        }
    }

    /// A dependency package: a framework package in appmodel terms.
    fn framework(identity: &ManifestIdentity) -> Self {
        Self {
            name: identity.name.clone(),
            version: identity.version.clone(),
            publisher: identity.publisher.clone(),
            publisher_id: publisher_id(&identity.publisher),
            architecture: identity
                .processor_architecture
                .clone()
                .unwrap_or_else(|| host_arch().to_owned()),
            app_id: None,
            is_framework: true,
        }
    }

    /// `Name_PublisherId` — the family a package and its updates share.
    fn family_name(&self) -> String {
        format!("{}_{}", self.name, self.publisher_id)
    }

    /// `Name_Version_Architecture__PublisherId`: the empty resource id
    /// renders as the doubled separator (the full-name form observed on
    /// real packages).
    fn full_name(&self) -> String {
        format!(
            "{}_{}_{}__{}",
            self.name, self.version, self.architecture, self.publisher_id
        )
    }

    /// `FamilyName!ApplicationId`, only for packages that declare a
    /// user-facing application.
    fn aumid(&self) -> Option<String> {
        self.app_id
            .as_ref()
            .map(|app| format!("{}!{}", self.family_name(), app))
    }
}

/// The MSIX publisher id — the 13-character hash tail of package family
/// names (Microsoft's is `8wekyb3d8bbwe`): the first 8 bytes of SHA-256
/// over the UTF-16LE publisher string, Crockford-base32 encoded as 13
/// groups of 5 bits over the 64 digest bits plus one zero pad bit.
/// Verified against both publisher strings of the Phase 0–4 acceptance
/// package set and the documented reference derivation.
fn publisher_id(publisher: &str) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let xml16: Vec<u8> = publisher
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let digest = Sha256::digest(&xml16);
    let prefix = u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("SHA-256 digests are 32 bytes"),
    );
    // The 64 digest bits plus the single zero pad bit form a 65-bit value
    // whose most-significant bit is the digest prefix's own; 13 groups of
    // five bits, read most-significant group first.
    let padded = u128::from(prefix) << 1;
    (0..13)
        .map(|digit| {
            let index = ((padded >> ((12 - digit) * 5)) & 31) as usize;
            (ALPHABET[index] as char).to_ascii_lowercase()
        })
        .collect()
}

/// The AppModel registry rows: the main package (the current package)
/// first, then every persisted dependency in deterministic order.
/// Dependencies register as frameworks.
fn collect_app_model_packages(
    directory: &Path,
    manifest: &msix::manifest::PackageManifest,
) -> Result<(Vec<AppModelPackage>, usize), String> {
    let paths = collect_manifest_paths(directory)?;
    let manifest_count = paths.len();
    let mut packages = vec![AppModelPackage::main(manifest)];
    for path in &paths[1..] {
        let xml = std::fs::read_to_string(path)
            .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        let parsed = parse_package_manifest(&xml)
            .map_err(|e| format!("Could not parse {}: {e}", path.display()))?;
        packages.push(AppModelPackage::framework(&parsed.identity));
    }
    Ok((packages, manifest_count))
}

/// Builds the regedit file seeding wine's package-identity table at
/// `HKCU\Software\Wine\AppModel`: `CurrentPackageFullName` names the
/// main package, one key per package carries the identity values the
/// patched appmodel code consumes. All identities derive from the
/// manifests; the table exists only for this run and is imported before
/// launch.
///
/// regedit syntax requires every backslash inside a value doubled.
fn appmodel_registry_file(packages: &[AppModelPackage], package_dir: &Path) -> String {
    const ROOT: &str = "HKEY_CURRENT_USER\\Software\\Wine\\AppModel";
    let mut file = String::from("Windows Registry Editor Version 5.00\n\n");
    let Some(current) = packages.first() else {
        return file;
    };
    let value = |text: &str| text.replace('\\', "\\\\");
    let path = value(&windows_z_path(package_dir));
    file.push_str(&format!("[{ROOT}]\n"));
    file.push_str(&format!(
        "\"CurrentPackageFullName\"=\"{}\"\n\n",
        value(&current.full_name())
    ));
    for package in packages {
        file.push_str(&format!(
            "[{ROOT}\\Packages\\{}]\n\
             \"Name\"=\"{}\"\n\
             \"Version\"=\"{}\"\n\
             \"PublisherId\"=\"{}\"\n\
             \"Publisher\"=\"{}\"\n\
             \"Architecture\"=\"{}\"\n\
             \"Path\"=\"{path}\"\n",
            package.full_name(),
            value(&package.name),
            value(&package.version),
            value(&package.publisher_id),
            value(&package.publisher),
            value(&package.architecture),
        ));
        if let Some(app_id) = &package.app_id {
            file.push_str(&format!("\"AppId\"=\"{}\"\n", value(app_id)));
        }
        file.push_str(&format!(
            "\"IsFramework\"=dword:{:08x}\n\n",
            u32::from(package.is_framework)
        ));
    }
    file
}

/// The Windows form of an absolute POSIX path through wine's default
/// Z: drive, which maps to the filesystem root
/// (`/run/media/julian/App` → `Z:\run\media\julian\App`).
fn windows_z_path(path: &Path) -> String {
    let body = path
        .to_string_lossy()
        .replace('/', "\\")
        .trim_start_matches('\\')
        .to_owned();
    format!("Z:\\{body}")
}

/// Builds the regedit file seeding wine's WinRT registry fallback: one
/// `ActivatableClassId` key per class, `DllPath` pointing at the
/// package's in-process server DLL through Z:.
///
/// regedit syntax requires every backslash inside a value doubled.
fn winrt_registry_file(classes: &[ActivatableClass], package_dir: &Path) -> String {
    let mut file = String::from("Windows Registry Editor Version 5.00\n\n");
    let directory = windows_z_path(package_dir);
    for class in classes {
        let dll = format!("{directory}\\{}", class.server_path.replace('/', "\\"));
        file.push_str(&format!(
            "[HKEY_LOCAL_MACHINE\\Software\\Microsoft\\WindowsRuntime\\ActivatableClassId\\{}]\n\
             \"DllPath\"=\"{}\"\n\n",
            class.class_id,
            dll.replace('\\', "\\\\")
        ));
    }
    file
}

/// Writes a UTF-16LE file with byte order mark; wine's regedit expects
/// the "Windows Registry Editor Version 5.00" import format in UTF-16.
fn write_utf16_file(path: &Path, content: &str) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(2 + content.len() * 2);
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    bytes.extend(content.encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(path, bytes)
}

/// The host architecture in Windows Update package spelling, the `os_arc`
/// analog (`url_gen.py:30-40`).
fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86" | "i686" => "x86",
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "arm" => "arm",
        _ => "arm",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fe3_file(digest: &str, algorithm: &str, size: u64) -> Fe3File {
        Fe3File {
            file_name: "test.msix".to_owned(),
            installer_specific_identifier: "identity_neutral_~_test".to_owned(),
            package_identity_name: "TestApp".to_owned(),
            update_id: "update-id".to_owned(),
            revision_number: "1".to_owned(),
            size,
            digest: digest.to_owned(),
            digest_algorithm: algorithm.to_owned(),
            modified: String::new(),
        }
    }

    #[test]
    fn size_mismatch_fails_the_transfer() {
        let file = fe3_file("Ku5sNclPz7QV2+lfQQue6R7oRu0=", "SHA1", 5);
        assert!(verify_transfer(&file, 4, &[]).is_err());
        assert!(verify_transfer(&file, 6, &[]).is_err());
    }

    #[test]
    fn digest_verifies_against_the_public_sha1_vector_of_hello() {
        // SHA-1("hello") = aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d;
        // FE3 reports that digest base64-encoded.
        let file = fe3_file("qvTGHdzF6KLavt4PO0gs2a6pQ00=", "SHA1", 5);
        assert_eq!(
            BASE64_STANDARD.encode(Sha1::digest(b"hello")),
            "qvTGHdzF6KLavt4PO0gs2a6pQ00="
        );
        assert!(verify_transfer(&file, 5, Sha1::digest(b"hello").as_slice()).is_ok());
    }

    #[test]
    fn digest_mismatch_fails_the_transfer() {
        let file = fe3_file("qvTGHdzF6KLavt4PO0gs2a6pQ00=", "SHA1", 5);
        assert!(verify_transfer(&file, 5, Sha1::digest(b"jello").as_slice()).is_err());
    }

    #[test]
    fn unknown_digest_algorithm_fails_the_transfer() {
        let file = fe3_file("Ku5sNclPz7QV2+lfQQue6R7oRu0=", "MD5", 5);
        assert!(verify_transfer(&file, 5, &[0]).is_err());
    }

    #[test]
    fn registry_file_maps_classes_to_their_server_dlls() {
        let file = winrt_registry_file(
            &[
                ActivatableClass {
                    class_id: "Microsoft.UI.Xaml.Application".to_owned(),
                    server_path: "Microsoft.UI.Xaml.dll".to_owned(),
                },
                ActivatableClass {
                    class_id: "A.B.C".to_owned(),
                    server_path: "sub/dir/Deep.dll".to_owned(),
                },
            ],
            Path::new("/run/xodus"),
        );
        assert!(file.starts_with("Windows Registry Editor Version 5.00\n\n"));
        assert!(file.contains(
            "[HKEY_LOCAL_MACHINE\\Software\\Microsoft\\WindowsRuntime\\\
             ActivatableClassId\\Microsoft.UI.Xaml.Application]\n\
             \"DllPath\"=\"Z:\\\\run\\\\xodus\\\\Microsoft.UI.Xaml.dll\"\n\n"
        ));
        // Path segments from the manifest (forward or Windows spelling)
        // land backslashed, regedit-escaped as doubled backslashes.
        assert!(file.contains("\"DllPath\"=\"Z:\\\\run\\\\xodus\\\\sub\\\\dir\\\\Deep.dll\"\n\n"));
    }

    #[test]
    fn entry_executable_defaults_to_the_first_application() {
        let manifest = msix::manifest::PackageManifest {
            applications: vec![msix::manifest::Application {
                id: "WhatsApp.Root".to_owned(),
                executable: "WhatsApp.Root.exe".to_owned(),
                entry_point: None,
            }],
            ..Default::default()
        };
        assert_eq!(
            choose_entry_executable(&manifest, None).unwrap(),
            "WhatsApp.Root.exe"
        );
        assert_eq!(
            choose_entry_executable(&manifest, Some("Other.exe")).unwrap(),
            "Other.exe"
        );
        assert!(choose_entry_executable(&Default::default(), None).is_err());
    }

    #[test]
    fn utf16_file_carries_a_bom_and_little_endian_units() {
        let target = tempfile::Builder::new().tempfile().unwrap();
        write_utf16_file(target.path(), "A\n").unwrap();
        let bytes = std::fs::read(target.path()).unwrap();
        assert_eq!(bytes, vec![0xFF, 0xFE, 0x41, 0x00, 0x0A, 0x00]);
    }

    #[test]
    fn publisher_id_matches_both_live_publisher_vectors() {
        // Ground truth from the phase 0-4 acceptance package set: the
        // app's own short-form publisher and Microsoft's full X500
        // subject (the well-known `8wekyb3d8bbwe` family suffix).
        assert_eq!(
            publisher_id("CN=24803D75-212C-471A-BC57-9EF86AB91435"),
            "cv1g1gvanyjgm"
        );
        assert_eq!(
            publisher_id(
                "CN=Microsoft Corporation, O=Microsoft Corporation, \
                 L=Redmond, S=Washington, C=US"
            ),
            "8wekyb3d8bbwe"
        );
    }

    #[test]
    fn package_names_derive_full_family_and_user_model_id() {
        let main = AppModelPackage::framework(&ManifestIdentity {
            name: "5319275A.WhatsAppDesktop".to_owned(),
            publisher: "CN=24803D75-212C-471A-BC57-9EF86AB91435".to_owned(),
            version: "2.2639.100.0".to_owned(),
            processor_architecture: Some("x64".to_owned()),
        });
        // The full name's publisher tail is the hash of the manifest
        // publisher, verified against the Store FulfillmentData PFN
        // 5319275A.WhatsAppDesktop_cv1g1gvanyjgm (Phase 0 log) — NOT the
        // spike-era v6f14k30rsarw constant, which no log ever tied to a
        // manifest or a store response (fake_run4.log answers it blind).
        assert_eq!(
            main.full_name(),
            "5319275A.WhatsAppDesktop_2.2639.100.0_x64__cv1g1gvanyjgm"
        );
        assert_eq!(main.family_name(), "5319275A.WhatsAppDesktop_cv1g1gvanyjgm");
        let mut with_app = main.clone();
        with_app.app_id = Some("App".to_owned());
        assert_eq!(
            with_app.aumid().as_deref(),
            Some("5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App")
        );
    }

    #[test]
    fn appmodel_registry_file_seeds_current_package_and_frameworks() {
        let packages = vec![
            AppModelPackage {
                name: "5319275A.WhatsAppDesktop".to_owned(),
                version: "2.2639.100.0".to_owned(),
                publisher: "CN=24803D75-212C-471A-BC57-9EF86AB91435".to_owned(),
                publisher_id: publisher_id("CN=24803D75-212C-471A-BC57-9EF86AB91435"),
                architecture: "x64".to_owned(),
                app_id: Some("App".to_owned()),
                is_framework: false,
            },
            AppModelPackage {
                name: "Microsoft.WindowsAppRuntime.2".to_owned(),
                version: "2.5.1.0".to_owned(),
                publisher: "CN=Microsoft Corporation".to_owned(),
                publisher_id: publisher_id("CN=Microsoft Corporation"),
                architecture: "x64".to_owned(),
                app_id: None,
                is_framework: true,
            },
        ];
        let file = appmodel_registry_file(&packages, Path::new("/run/xodus/pkg"));
        assert!(file.starts_with("Windows Registry Editor Version 5.00\n\n"));
        let main_full = "5319275A.WhatsAppDesktop_2.2639.100.0_x64__cv1g1gvanyjgm";
        assert!(file.contains(&format!(
            "[HKEY_CURRENT_USER\\Software\\Wine\\AppModel]\n\
             \"CurrentPackageFullName\"=\"{main_full}\"\n\n"
        )));
        let main_key =
            format!("[HKEY_CURRENT_USER\\Software\\Wine\\AppModel\\Packages\\{main_full}]\n");
        assert!(file.contains(&main_key));
        assert!(file.contains("\"AppId\"=\"App\"\n"));
        // Path values land regedit-escaped with doubled backslashes.
        assert!(file.contains("\"Path\"=\"Z:\\\\run\\\\xodus\\\\pkg\"\n"));
        assert!(file.contains("\"IsFramework\"=dword:00000000\n\n"));
        assert!(file.contains(
            "[HKEY_CURRENT_USER\\Software\\Wine\\AppModel\\Packages\\\
             Microsoft.WindowsAppRuntime.2_2.5.1.0_x64__"
        ));
        assert!(file.contains("\"IsFramework\"=dword:00000001\n\n"));
    }

    #[test]
    fn appmodel_packages_derive_from_the_manifest_tree() {
        const MAIN_MANIFEST: &str = r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="5319275A.WhatsAppDesktop" Publisher="CN=24803D75-212C-471A-BC57-9EF86AB91435" Version="2.2639.100.0" ProcessorArchitecture="x64" />
  <Applications><Application Id="App" Executable="WhatsApp.Root.exe" /></Applications>
</Package>"#;
        const DEPENDENCY_MANIFEST: &str = r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="Microsoft.WindowsAppRuntime.2" Publisher="CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US" Version="2.5.1.0" ProcessorArchitecture="x64" />
</Package>"#;
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("AppxManifest.xml"), MAIN_MANIFEST).unwrap();
        std::fs::create_dir_all(directory.path().join("AppxMetadata")).unwrap();
        std::fs::write(
            directory
                .path()
                .join("AppxMetadata")
                .join("dependency-de2aac7e.AppxManifest.xml"),
            DEPENDENCY_MANIFEST,
        )
        .unwrap();
        let manifest = parse_package_manifest(
            &std::fs::read_to_string(directory.path().join("AppxManifest.xml")).unwrap(),
        )
        .unwrap();
        let (packages, manifests) =
            collect_app_model_packages(directory.path(), &manifest).unwrap();
        assert_eq!(manifests, 2);
        assert_eq!(packages.len(), 2);
        assert_eq!(
            packages[0].full_name(),
            "5319275A.WhatsAppDesktop_2.2639.100.0_x64__cv1g1gvanyjgm"
        );
        assert!(!packages[0].is_framework);
        assert_eq!(packages[0].app_id.as_deref(), Some("App"));
        assert_eq!(
            packages[1].full_name(),
            "Microsoft.WindowsAppRuntime.2_2.5.1.0_x64__8wekyb3d8bbwe"
        );
        assert!(packages[1].is_framework);
        assert!(packages[1].app_id.is_none());
    }
}
