use std::error::Error;
use std::fs;
use std::path::PathBuf;

use clap::Parser;
use msixvc_common::resources::pri::Pri;
use msixvc_common::resources::query::{QualifierContext, ResolvedValue};

#[derive(Parser)]
#[command(
    author,
    version,
    about = "Extract embedded /Files/ resources from a Package Resource Index (PRI) file"
)]
struct Args {
    /// Path to the PRI file to read.
    file: PathBuf,

    /// Destination root directory.
    #[arg(short, long)]
    out_dir: PathBuf,

    /// Create .xaml duplicates alongside .xbf files for URI compatibility.
    #[arg(long, default_value_t = true)]
    create_xaml_aliases: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let pri_bytes = tokio::fs::read(&args.file).await?;
    let pri = Pri::read(&*pri_bytes).await?;

    let ctx = QualifierContext::default();
    let mut extracted_count = 0;

    for resource in pri.resource_paths() {
        if let Some(rel_path) = resource.strip_prefix("/Files/") {
            let out_path = args.out_dir.join(rel_path);

            if let Some(ResolvedValue::Blob(bytes)) = pri.resolve(resource, &ctx)? {
                if let Some(parent) = out_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&out_path, &bytes)?;
                extracted_count += 1;
                println!("Extracted: {} ({} bytes)", out_path.display(), bytes.len());

                if args.create_xaml_aliases
                    && out_path.extension().and_then(|s| s.to_str()) == Some("xbf")
                {
                    let xaml_path = out_path.with_extension("xaml");
                    fs::write(&xaml_path, &bytes)?;
                    println!("  -> Alias: {}", xaml_path.display());
                }
            }
        }
    }

    println!(
        "\nTotal extracted from {}: {} file(s)",
        args.file.display(),
        extracted_count
    );
    Ok(())
}
