//! MSIX/OPC container handling: manifest parsing and extraction for the
//! packages the FE3 protocol delivers (`store extract`, `store install`).
//!
//! Store `.msix`/`.appx`/`.msixbundle` files are OPC/zip containers
//! (not the encrypted EAppx/EMSIX variants `eappx-rs` reads), so handling
//! is central-directory zip reading plus manifest XML shape knowledge:
//!
//! - A bundle announces its packages in `AppxMetadata/AppxBundleManifest.xml`
//!   with per-package file name, offset, size and architecture; the
//!   application package matching the host architecture is copied out by
//!   name (see [`manifest::select_arch_application_packages`]).
//! - A single package extracts its OPC entries into a directory; its
//!   `AppxManifest.xml` reports identity, dependencies and the entry point
//!   (see [`manifest::PackageManifest`]).
//!
//! All reads go through the central directory and check entry CRCs. Both
//! are mandatory for FE3-delivered containers: their local headers use
//! ZIP64 method 45 with data descriptors and zeroed local sizes, verified
//! in `scratches/fe3_spike/` (2026-10-12) against a live `WindowsAppRuntime`
//! msix.

pub mod manifest;
pub mod opc;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MsixError {
    #[error("Zip container error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("Manifest XML error: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("Manifest attribute error: {0}")]
    Attribute(#[from] quick_xml::events::attributes::AttrError),
    #[error("Container entry is not valid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Container carries no AppxMetadata/AppxBundleManifest.xml")]
    NoBundleManifest,
    #[error("Bundle lists no application package for architecture {0}")]
    NoApplicationPackage(String),
    #[error("Archive entry name escapes the target directory: {0}")]
    PathTraversal(String),
    #[error("Manifest element {element} has no required attribute {attribute}")]
    MissingAttribute { element: String, attribute: String },
    #[error(
        "Manifest element {element} attribute {attribute} is not an unsigned number: '{value}'"
    )]
    BadNumberAttribute {
        element: String,
        attribute: String,
        value: String,
    },
}
