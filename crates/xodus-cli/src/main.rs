use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use xodus::api::fe3;
use xodus::tokens::TokenManager;

mod commands;
mod license;
mod package;
mod webview;

#[derive(Subcommand)]
enum SubCommand {
    #[command(about = "Download msixvc or xsp files for given game")]
    Download {
        product: String,
        #[arg(short, long)]
        market: Option<String>,
        #[arg(
            long,
            default_value_t = false,
            help = "Display download URLs instead of downloading"
        )]
        dry_run: bool,
    },
    #[command(about = "Dump CIKs for use with XvdTool")]
    License {
        #[clap(help = "Content Id of a license")]
        content_id: String,
        #[clap(help = "A path where to dump CIKs")]
        ciks: String,
        #[arg(short, long)]
        market: Option<String>,
    },
    #[command(about = "Extract locally stored msixvc file")]
    Extract {
        path: String,
        destination: String,
        #[arg(short, long)]
        market: Option<String>,
    },
    #[command(
        about = "Extract a locally stored EAppx/EMSIX package (research task, see issue #91)"
    )]
    ExtractEappx {
        path: String,
        destination: String,
        #[arg(short, long, help = "Path to a keyfile with content decryption keys")]
        key_file: Option<String>,
    },
    Login,
    Logout {
        #[arg(long, default_value_t = false, help = "Remove device license")]
        device: bool,
    },
    #[command(about = "Download and extract the game through streaming algorithm")]
    Streaming {
        source: String,
        destination: String,
        #[arg(
            long,
            default_value_t = false,
            help = "Attempt to skip downloading NTFS metadata to be faste while missing some files"
        )]
        try_skip_ntfs: bool,
        #[arg(short, long)]
        parallel: Option<usize>,
        #[arg(short, long)]
        market: Option<String>,
    },
    #[cfg(unix)]
    #[command(about = "Run a Game with xodus wine")]
    Run {
        source: String,
        wine: String,
        #[arg(short, long)]
        exe: Option<String>,
        #[arg(short, long)]
        market: Option<String>,
    },
    #[command(about = "Generate or decrypt base64-encoded CLEP challenge data")]
    Clep {
        #[command(subcommand)]
        action: ClepAction,
    },
    #[command(about = "Decode SPLicenseBlock")]
    SpLicense {
        block: String,
    },
    #[command(about = "Microsoft Store packages through the undocumented FE3 protocol")]
    Store {
        #[command(subcommand)]
        action: StoreAction,
    },
}

#[derive(Subcommand)]
enum ClepAction {
    #[command(
        about = "Generate a base64-encoded CLEP challenge (V2 and V4) from SMBIOS/disk serial data"
    )]
    Generate {
        #[arg(
            long,
            help = "Base64-encoded SMBIOS data (up to 256 bytes, zero-padded)"
        )]
        smbios: Option<String>,
        #[arg(
            long,
            help = "Base64-encoded disk serial (up to 64 bytes, zero-padded)"
        )]
        disk_serial: Option<String>,
    },
    #[command(about = "Decrypt a base64-encoded CLEP challenge back into its plaintext fields")]
    Decrypt {
        #[clap(help = "Base64-encoded, obfuscated CLEP challenge data (2048 bytes)")]
        data: String,
    },
}

#[derive(Subcommand)]
enum StoreAction {
    #[command(about = "List a product category's package files with their delivery URLs")]
    Urls {
        #[clap(help = "WuCategoryId of the Store product (see its FulfillmentData)")]
        wu_category_id: String,
        #[arg(short, long, default_value = "Retail", help = "Windows Update ring")]
        release: String,
    },
    #[command(
        about = "Download a Store product's main package and dependencies through the FE3 protocol"
    )]
    Download {
        #[clap(help = "Store product page URL or product id")]
        product: String,
        #[clap(help = "Directory to place the downloaded files (default: current directory)")]
        destination: Option<String>,
        #[arg(short, long, default_value = "US", help = "Store market")]
        market: String,
        #[arg(short, long, default_value = "Retail", help = "Windows Update ring")]
        release: String,
        #[arg(
            long,
            default_value_t = false,
            help = "Display the chosen package files and URLs instead of downloading"
        )]
        dry_run: bool,
    },
    #[command(about = "Extract an MSIX bundle or package container into a directory")]
    Extract {
        #[clap(help = "Path to an .msixbundle or .msix/.appx container")]
        package: String,
        #[clap(help = "Directory to extract into (default: current directory)")]
        destination: Option<String>,
        #[arg(
            short,
            long,
            help = "Architecture selecting a bundle's application package (default: host architecture)"
        )]
        arch: Option<String>,
    },
    #[command(
        about = "Extract a Store package and its dependencies into one directory, reporting install metadata"
    )]
    Install {
        #[clap(help = "Path to the main .msix/.appx container")]
        package: String,
        #[clap(help = "Directory to extract the package tree into")]
        destination: String,
        #[arg(
            short,
            long = "dependency",
            help = "Path to a dependency container (.appx/.msix); repeat for each"
        )]
        dependencies: Vec<String>,
    },
    #[cfg(unix)]
    #[command(
        about = "Run an installed Store package's entry executable under wine (vanilla or patched)"
    )]
    Run {
        #[clap(help = "Directory of the installed Store package (as produced by store install)")]
        directory: String,
        #[clap(help = "Path to the wine binary to launch the package with")]
        wine: String,
        #[arg(
            short,
            long,
            help = "Executable inside the package (default: the manifest's first Application @Executable)"
        )]
        exe: Option<String>,
        #[arg(
            short,
            long,
            help = "Wine prefix to create or reuse (default: <directory>/.xodus-wine)"
        )]
        prefix: Option<String>,
    },
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct CliArgs {
    #[command(subcommand)]
    command: SubCommand,
}

#[tokio::main]
async fn main() -> ExitCode {
    let filter = tracing_subscriber::EnvFilter::from_env("XODUS_LOG");
    let registry =
        tracing_subscriber::registry().with(tracing_subscriber::fmt::layer().with_filter(filter));

    #[cfg(feature = "tokio_console")]
    {
        use tracing::level_filters::LevelFilter;
        use tracing_subscriber::filter::Targets;

        let console_filter = Targets::new()
            .with_target("tokio", LevelFilter::TRACE)
            .with_target("runtime", LevelFilter::TRACE);
        let console_layer = console_subscriber::spawn().with_filter(console_filter);
        registry.with(console_layer).init();
    }
    #[cfg(not(feature = "tokio_console"))]
    {
        registry.init();
    }
    // The FE3 hosts chain to a private Microsoft root; the library adds
    // exactly that root (wider trust, no verification relaxation).
    let client = fe3::add_fe3_trust(
        reqwest::ClientBuilder::new()
            .user_agent(format!("xodus-cli/{}", env!("CARGO_PKG_VERSION")))
            .connection_verbose(true),
    )
    .build()
    .unwrap();
    let args = CliArgs::parse();

    xodus::secrets::init_secrets().expect("Unable to initialize credentials");
    let tokens = TokenManager::with_keychain_and_memory();

    // Clep/SpLicense are pure local data transforms, Logout only removes
    // stored credentials, and the store commands speak the unauthenticated
    // FE3 flow - none of them need a device identity, so don't force
    // provisioning (network + keychain access) just to run them. This
    // matters in practice: on a session with no usable secret-service
    // keychain, provisioning fails outright, which previously meant even
    // these fully offline commands were unusable.
    let needs_device_credentials = !matches!(
        args.command,
        SubCommand::Clep { .. }
            | SubCommand::SpLicense { .. }
            | SubCommand::Logout { .. }
            | SubCommand::Store { .. }
    );
    if needs_device_credentials {
        xodus::tokens::device::ensure_device_credentials(&client, &tokens).await;
    }

    let code = match args.command {
        SubCommand::Download {
            product,
            market,
            dry_run,
        } => commands::download::run(&client, &tokens, product, market, dry_run).await,
        SubCommand::License {
            content_id,
            market,
            ciks,
        } => {
            commands::license::run(
                &client,
                &tokens,
                content_id,
                market.unwrap_or("neutral".to_string()),
                ciks,
            )
            .await
        }
        SubCommand::Login => commands::login::run(&client, &tokens).await,
        SubCommand::Logout { device } => commands::logout::run(&tokens, device).await,
        SubCommand::Extract {
            path,
            destination,
            market,
        } => {
            commands::extract::run(
                &client,
                &tokens,
                path,
                destination,
                market.unwrap_or("neutral".to_string()),
            )
            .await
        }
        SubCommand::ExtractEappx {
            path,
            destination,
            key_file,
        } => commands::extract_eappx::run(path, destination, key_file).await,
        SubCommand::Streaming {
            source,
            destination,
            try_skip_ntfs,
            market,
            parallel,
        } => {
            commands::streaming::run(
                &client,
                &tokens,
                source,
                destination,
                try_skip_ntfs,
                parallel,
                market,
            )
            .await
        }
        #[cfg(unix)]
        SubCommand::Run {
            source,
            wine,
            exe,
            market,
        } => commands::run::run(&client, &tokens, source, wine, exe, market).await,
        SubCommand::Clep { action } => match action {
            ClepAction::Generate {
                smbios,
                disk_serial,
            } => commands::clep::generate(smbios, disk_serial),
            ClepAction::Decrypt { data } => commands::clep::decrypt(data),
        },
        SubCommand::SpLicense { block } => commands::splicense::run(block),
        SubCommand::Store { action } => match action {
            StoreAction::Urls {
                wu_category_id,
                release,
            } => commands::store::run_urls(&client, wu_category_id, release).await,
            StoreAction::Download {
                product,
                destination,
                market,
                release,
                dry_run,
            } => {
                commands::store::run_download(
                    &client,
                    product,
                    destination,
                    market,
                    release,
                    dry_run,
                )
                .await
            }
            StoreAction::Extract {
                package,
                destination,
                arch,
            } => commands::store::run_extract(package, destination, arch),
            StoreAction::Install {
                package,
                destination,
                dependencies,
            } => commands::store::run_install(package, destination, dependencies),
            #[cfg(unix)]
            StoreAction::Run {
                directory,
                wine,
                exe,
                prefix,
            } => commands::store::run_run(directory, wine, exe, prefix).await,
        },
    };

    xodus::secrets::destroy_secrets();

    code
}
