//! Data models of the FE3 (Windows Update delivery) protocol.
//!
//! Documented against live captures against the FE3 endpoints (2026-10-12,
//! `scratches/fe3_spike/transcripts/`); protocol flow adapted from the
//! MIT-licensed `alt-app-installer` project (see `resources/fe3/LICENSE.alt-app-installer`).

/// A single downloadable package file discovered through a FE3 `SyncUpdates`
/// round trip.
///
/// The FE3 `SyncUpdates` response describes each package twice: the update
/// identity lives under `NewUpdates/UpdateInfo` while the file metadata lives
/// in a parallel `ExtendedUpdateInfo/Updates/Update/Files/File` tree; both are
/// joined by the numeric FE3 update id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fe3File {
    /// GUID-named file on the delivery CDN, e.g.
    /// `473405aa-5d34-4e7e-b8e2-8afc194a5e66.msixbundle`.
    pub file_name: String,
    /// Rich package identifier as reported in the `File`'s
    /// `InstallerSpecificIdentifier` attribute, e.g.
    /// `5319275A.WhatsAppDesktop_2.2639.100.0_neutral_~_cv1g1gvanyjgm`.
    pub installer_specific_identifier: String,
    /// Package identity from `ExtendedProperties`, e.g.
    /// `5319275A.WhatsAppDesktop`.
    pub package_identity_name: String,
    /// Windows Update identity required by the `secured` step.
    pub update_id: String,
    /// Windows Update revision matching [`Fe3File::update_id`].
    pub revision_number: String,
    /// Expected file size in bytes as reported by FE3 (`File@Size`).
    ///
    /// Known to differ slightly (a few bytes) from what a HEAD request on the
    /// delivery URL reports; trust this value and verify the download with
    /// [`Fe3File::digest`] instead.
    pub size: u64,
    /// Base64-encoded digest of the file as reported by FE3 (`File@Digest`).
    pub digest: String,
    /// Digest algorithm of [`Fe3File::digest`] (`File@DigestAlgorithm`).
    pub digest_algorithm: String,
    /// Last modification timestamp as reported by FE3 (`File@Modified`).
    pub modified: String,
}
