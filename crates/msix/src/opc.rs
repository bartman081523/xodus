//! OPC (Open Packaging Conventions) container reading.
//!
//! MSIX/Appx containers are zip archives read by name through the central
//! directory ([`zip::read::ZipArchive`] ignores local headers, which
//! FE3-delivered containers render useless: they carry ZIP64 data
//! descriptors and zeroed local sizes). Every decompressed read is
//! CRC-checked by the zip reader against the central directory.

use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::Path;

use zip::ZipArchive;

use crate::MsixError;
use crate::manifest::{BundleManifest, parse_bundle_manifest};

/// The package manifest of a single (non-bundle) package.
pub const MANIFEST_PATH: &str = "AppxManifest.xml";
/// The bundle manifest, inside its metadata directory (live bundles).
pub const BUNDLE_MANIFEST_PATH: &str = "AppxMetadata/AppxBundleManifest.xml";
/// Bundle manifests at the archive root, as older bundles shipped them.
pub const BUNDLE_MANIFEST_ROOT_PATH: &str = "AppxBundleManifest.xml";

/// An opened OPC container (`.msix`/`.appx`/`.msixbundle`).
pub struct Container<R: Read + Seek> {
    archive: ZipArchive<R>,
}

/// File count and uncompressed byte total of one extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extraction {
    pub files: usize,
    pub bytes: u64,
}

impl<R: Read + Seek> Container<R> {
    /// Opens the container's central directory for name-based reads.
    pub fn open(reader: R) -> Result<Self, MsixError> {
        Ok(Self {
            archive: ZipArchive::new(reader)?,
        })
    }

    /// Number of archive entries in the container.
    pub fn len(&self) -> usize {
        self.archive.len()
    }

    pub fn is_empty(&self) -> bool {
        self.archive.len() == 0
    }

    pub fn has_bundle_manifest(&self) -> bool {
        self.archive.index_for_name(BUNDLE_MANIFEST_PATH).is_some()
            || self
                .archive
                .index_for_name(BUNDLE_MANIFEST_ROOT_PATH)
                .is_some()
    }

    /// Reads and parses the bundle manifest of a bundle container.
    pub fn read_bundle_manifest(&mut self) -> Result<BundleManifest, MsixError> {
        let name = if self.archive.index_for_name(BUNDLE_MANIFEST_PATH).is_some() {
            BUNDLE_MANIFEST_PATH
        } else if self
            .archive
            .index_for_name(BUNDLE_MANIFEST_ROOT_PATH)
            .is_some()
        {
            BUNDLE_MANIFEST_ROOT_PATH
        } else {
            return Err(MsixError::NoBundleManifest);
        };
        parse_bundle_manifest(&self.read_entry_str(name)?)
    }

    /// Reads one entry fully; the zip reader CRC-checks the data.
    pub fn read_entry_str(&mut self, name: &str) -> Result<String, MsixError> {
        Ok(String::from_utf8(self.read_entry(name)?)?)
    }

    pub fn read_entry(&mut self, name: &str) -> Result<Vec<u8>, MsixError> {
        let mut file = self.archive.by_name(name)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        Ok(buffer)
    }

    /// Copies one entry's decompressed data to `writer` (CRC-checked).
    pub fn copy_entry(
        &mut self,
        name: &str,
        writer: &mut impl io::Write,
    ) -> Result<u64, MsixError> {
        validate_entry_path(name)?;
        let mut file = self.archive.by_name(name)?;
        Ok(io::copy(&mut file, writer)?)
    }

    /// Extracts the whole package into `destination`, preserving the
    /// entry tree. Returns the file count and uncompressed byte total
    /// (directory entries are created but not counted).
    pub fn extract_all(&mut self, destination: &Path) -> Result<Extraction, MsixError> {
        let mut extraction = Extraction { files: 0, bytes: 0 };
        for index in 0..self.archive.len() {
            let mut file = self.archive.by_index(index)?;
            let name = file.name()?.to_string();
            validate_entry_path(&name)?;
            if file.is_dir() {
                std::fs::create_dir_all(destination.join(&name))?;
                continue;
            }
            if let Some(parent) = Path::new(&name)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(destination.join(parent))?;
            }
            let mut target = File::create(destination.join(&name))?;
            let written = io::copy(&mut file, &mut target)?;
            extraction.files += 1;
            extraction.bytes += written;
        }
        Ok(extraction)
    }

    /// Copies archive entries whose target file does not yet exist into
    /// `destination` — existing files win. Used to merge dependency
    /// packages into a main package's extraction directory (both ship
    /// overlapping framework DLLs; the main package's files, including
    /// its `AppxManifest.xml`, take priority). Returns the file count and
    /// byte total of what was actually copied.
    pub fn merge_into(&mut self, destination: &Path) -> Result<Extraction, MsixError> {
        let mut extraction = Extraction { files: 0, bytes: 0 };
        for index in 0..self.archive.len() {
            let mut file = self.archive.by_index(index)?;
            let name = file.name()?.to_string();
            validate_entry_path(&name)?;
            if file.is_dir() {
                let target = destination.join(&name);
                if !target.exists() {
                    std::fs::create_dir_all(&target)?;
                }
                continue;
            }
            let target = destination.join(&name);
            if target.exists() {
                continue;
            }
            if let Some(parent) = Path::new(&name)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(destination.join(parent))?;
            }
            let mut target_file = File::create(&target)?;
            let written = io::copy(&mut file, &mut target_file)?;
            extraction.files += 1;
            extraction.bytes += written;
        }
        Ok(extraction)
    }
}

/// Guards an archive entry name against escaping the target directory
/// (absolute paths, backslash separation, `..` components). MSIX entries
/// are `/`-separated relative names; anything else is not trusted.
pub fn validate_entry_path(name: &str) -> Result<(), MsixError> {
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.split('/').any(|component| component == "..")
    {
        return Err(MsixError::PathTraversal(name.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CRC-32 (IEEE) for test fixture archives; table-less.
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// Builds a stored-method (uncompressed) zip archive with one
    /// central directory record per entry, like an OPC container.
    fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        #[derive(Clone)]
        struct Record {
            name: Vec<u8>,
            crc: u32,
            compressed_size: u32,
            uncompressed_size: u32,
            offset: u32,
        }
        let mut out = Vec::new();
        let mut records: Vec<Record> = Vec::new();

        fn little32(value: u32, out: &mut Vec<u8>) {
            out.extend_from_slice(&value.to_le_bytes());
        }
        fn little16(value: u16, out: &mut Vec<u8>) {
            out.extend_from_slice(&value.to_le_bytes());
        }

        for (name, data) in entries {
            let name_bytes = name.as_bytes();
            let record = Record {
                name: name_bytes.to_vec(),
                crc: crc32(data),
                compressed_size: data.len() as u32,
                uncompressed_size: data.len() as u32,
                offset: out.len() as u32,
            };
            out.extend_from_slice(b"PK\x03\x04"); // local file header
            little16(20, &mut out); // version needed
            little16(0, &mut out); // flags
            little16(0, &mut out); // method: stored
            little16(0, &mut out); // time
            little16(0, &mut out); // date
            little32(record.crc, &mut out);
            little32(record.compressed_size, &mut out);
            little32(record.uncompressed_size, &mut out);
            little16(name_bytes.len() as u16, &mut out);
            little16(0, &mut out); // extra field length
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(data);
            records.push(record);
        }

        let central_offset = out.len();
        for record in &records {
            out.extend_from_slice(b"PK\x01\x02"); // central directory header
            little16(20, &mut out); // version made by
            little16(20, &mut out); // version needed
            little16(0, &mut out); // flags
            little16(0, &mut out); // method
            little16(0, &mut out); // time
            little16(0, &mut out); // date
            little32(record.crc, &mut out);
            little32(record.compressed_size, &mut out);
            little32(record.uncompressed_size, &mut out);
            little16(record.name.len() as u16, &mut out);
            little16(0, &mut out); // extra
            little16(0, &mut out); // comment
            little16(0, &mut out); // disk number
            little16(0, &mut out); // internal attributes
            little32(0, &mut out); // external attributes
            little32(record.offset, &mut out);
            out.extend_from_slice(&record.name);
        }
        let central_size = (out.len() - central_offset) as u32;
        out.extend_from_slice(b"PK\x05\x06"); // end of central directory
        little16(0, &mut out);
        little16(0, &mut out);
        little16(records.len() as u16, &mut out);
        little16(records.len() as u16, &mut out);
        little32(central_size, &mut out);
        little32(central_offset as u32, &mut out);
        little16(0, &mut out); // comment length
        out
    }

    fn open_container(bytes: &[u8]) -> Container<io::Cursor<&[u8]>> {
        Container::open(io::Cursor::new(bytes)).unwrap()
    }

    #[test]
    fn extract_all_preserves_the_entry_tree() {
        let entries = stored_zip(&[
            ("file.bin", b"payload"),
            ("sub/file.txt", b"text"),
            ("empty-dir/", b""),
        ]);
        let destination = tempfile::tempdir().unwrap();
        let mut container = open_container(&entries);
        let extraction = container.extract_all(destination.path()).unwrap();

        assert_eq!(extraction.files, 2);
        assert_eq!(extraction.bytes, 7 + 4);
        assert_eq!(
            std::fs::read(destination.path().join("file.bin")).unwrap(),
            b"payload"
        );
        assert_eq!(
            std::fs::read(destination.path().join("sub/file.txt")).unwrap(),
            b"text"
        );
        assert!(destination.path().join("empty-dir").is_dir());
    }

    #[test]
    fn crc_mismatch_fails_the_read() {
        let mut entries = stored_zip(&[("file.bin", b"payload")]);
        // Corrupt the central-directory CRC of the only entry: the CRC
        // field sits 16 bytes into the central header record.
        const CENTRAL_HEADER_CRC: usize = 0x10;
        // Locate the central directory header via the EOCD tail and patch.
        let eocd = entries.len() - 22;
        let offset = u32::from_le_bytes([
            entries[eocd + 16],
            entries[eocd + 17],
            entries[eocd + 18],
            entries[eocd + 19],
        ]) as usize;
        let crc_index = offset + CENTRAL_HEADER_CRC;
        entries[crc_index] = entries[crc_index].wrapping_add(1);
        let mut container = open_container(&entries);
        assert!(container.read_entry("file.bin").is_err());
    }

    #[test]
    fn traversal_entry_names_are_rejected() {
        let entries = stored_zip(&[("../evil.txt", b"nope")]);
        let mut container = open_container(&entries);
        let destination = tempfile::tempdir().unwrap();
        assert!(container.extract_all(destination.path()).is_err());
        // Nothing written outside the destination.
        assert!(!tmp_parent_path_exists(destination.path()));
    }

    /// Checks that no `evil.txt` sibling appeared next to the temp dir.
    fn tmp_parent_path_exists(destination: &Path) -> bool {
        destination
            .parent()
            .map(|parent| parent.join("evil.txt").exists())
            .unwrap_or(false)
    }

    #[test]
    fn validate_entry_path_rejects_escapes() {
        assert!(validate_entry_path("AppxManifest.xml").is_ok());
        assert!(validate_entry_path("AppxMetadata/AppxBundleManifest.xml").is_ok());
        assert!(validate_entry_path("").is_err());
        assert!(validate_entry_path("/etc/passwd").is_err());
        assert!(validate_entry_path(r"sub\file.txt").is_err());
        assert!(validate_entry_path("sub/../../file.txt").is_err());
        assert!(validate_entry_path("..").is_err());
    }

    #[test]
    fn bundle_manifest_is_read_from_appx_metadata() {
        let manifest_xml = br#"<Bundle xmlns="http://schemas.microsoft.com/appx/2013/bundle">
            <Identity Name="A" Publisher="CN=P" Version="1.0.0.0"/>
            <Packages>
                <Package Type="application" Version="1.0.0.0" Architecture="x64" FileName="A_1.0.0.0_x64.msix" Offset="0" Size="7"/>
            </Packages>
        </Bundle>"#;
        let bundle_bytes = stored_zip(&[
            (MANIFEST_PATH, b"<Package/>"),
            (BUNDLE_MANIFEST_PATH, manifest_xml),
            ("A_1.0.0.0_x64.msix", b"package"),
        ]);
        let mut container = open_container(&bundle_bytes);
        assert!(container.has_bundle_manifest());
        let manifest = container.read_bundle_manifest().unwrap();
        assert_eq!(manifest.identity.name, "A");
        assert_eq!(manifest.packages.len(), 1);

        // A package container without a bundle manifest.
        let package_bytes = stored_zip(&[(MANIFEST_PATH, b"<Package/>")]);
        let mut container = open_container(&package_bytes);
        assert!(!container.has_bundle_manifest());
        assert!(matches!(
            container.read_bundle_manifest(),
            Err(MsixError::NoBundleManifest)
        ));
    }

    #[test]
    fn entries_are_read_by_name() {
        let bytes = stored_zip(&[("AppxManifest.xml", b"<Package/>"), ("sub/a.txt", b"hi")]);
        let mut container = open_container(&bytes);
        assert_eq!(container.read_entry("sub/a.txt").unwrap(), b"hi");
        assert!(container.read_entry("missing.txt").is_err());
    }

    #[test]
    fn merge_into_skips_existing_files_and_keeps_the_rest() {
        let destination = tempfile::tempdir().unwrap();
        // Main package writes these first.
        std::fs::write(destination.path().join("AppxManifest.xml"), b"main").unwrap();
        std::fs::write(destination.path().join("winmd.dll"), b"\x00existing\x01").unwrap();
        // Dependency package: two shared names (skipped) plus a new one.
        let dep_bytes = stored_zip(&[
            ("AppxManifest.xml", b"dependency"),
            ("winmd.dll", b"\x00different\x02"),
            ("dep_only.dll", b"dep"),
        ]);
        let mut container = open_container(&dep_bytes);
        let merged = container.merge_into(destination.path()).unwrap();

        assert_eq!(merged.files, 1);
        assert_eq!(merged.bytes, 3);
        assert_eq!(
            std::fs::read(destination.path().join("AppxManifest.xml")).unwrap(),
            b"main"
        );
        assert_eq!(
            std::fs::read(destination.path().join("winmd.dll")).unwrap(),
            b"\x00existing\x01"
        );
        assert_eq!(
            std::fs::read(destination.path().join("dep_only.dll")).unwrap(),
            b"dep"
        );
    }
}
