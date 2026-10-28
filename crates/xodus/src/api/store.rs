//! Microsoft Store edge product endpoint — the FE3 glue.
//!
//! The FE3 protocol ([`crate::api::fe3`]) addresses a product by its
//! `WuCategoryId`, which the `storeedgefd` product endpoint reports in the
//! `FulfillmentData` of its first SKU. The endpoint shape is the spec:
//! flow and JSON paths follow the live endpoint as captured by the
//! MIT-licensed `alt-app-installer` project (see
//! `resources/fe3/LICENSE.alt-app-installer`; `url_gen.py:342-366`).

use std::collections::BTreeMap;

use thiserror::Error;

use crate::models::fe3::Fe3File;
use crate::models::store::{FulfillmentData, StoreProductDetailsResponse};

pub const STORE_EDGE_PRODUCTS_URL: &str = "https://storeedgefd.dsx.mp.microsoft.com/v9.0/products";

#[derive(Debug, Error)]
pub enum StoreEdgeError {
    #[error("Product endpoint request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// The endpoint replies with a `Payload` only for products that ship as
    /// UWP/MSIX Store packages.
    #[error("Product {0} carries no FulfillmentData (not a UWP/MSIX Store package)")]
    NoFulfillmentData(String),
    #[error("FulfillmentData of product {product} is malformed: {source}")]
    MalformedFulfillmentData {
        product: String,
        #[source]
        source: serde_json::Error,
    },
}

/// Extracts the Store product id from a product page URL.
///
/// Returns the last non-empty, query-less URL path segment, e.g.
/// `9NBLGGH4NNS1` out of `https://apps.microsoft.com/detail/9NBLGGH4NNS1?hl=de`.
/// Input that is not an absolute URL (the `url` crate cannot parse it) is
/// treated as a bare product id. The same heuristic as `alt-app-installer`
/// (`url_gen.py:342-345`), plus a URL parse to avoid mistaking the bare
/// hostname of a path-less product URL for the id. Returns `None` for input
/// that carries no usable id.
pub fn product_id_from_page(input: &str) -> Option<String> {
    if let Ok(url) = url::Url::parse(input) {
        return url
            .path()
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .map(ToOwned::to_owned);
    }
    let trimmed = input.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Fetches and decodes the `FulfillmentData` of one Store product.
///
/// `product_id` is a Store listing id (see [`product_id_from_page`]). The
/// first SKU with a decodable `FulfillmentData` wins, mirroring
/// `alt-app-installer`'s `Payload.Skus[0].FulfillmentData` access.
pub async fn find_product_fulfillment_data(
    client: &reqwest::Client,
    product_id: &str,
    market: &str,
) -> Result<FulfillmentData, StoreEdgeError> {
    let response = client
        .get(format!(
            "{STORE_EDGE_PRODUCTS_URL}/{product_id}?market={market}&locale=en-us&deviceFamily=Windows.Desktop"
        ))
        .send()
        .await?
        .error_for_status()?
        .json::<StoreProductDetailsResponse>()
        .await?;

    let Some(payload) = response.payload else {
        return Err(StoreEdgeError::NoFulfillmentData(product_id.to_owned()));
    };
    let blob = payload
        .skus
        .into_iter()
        .find_map(|mut sku| sku.fulfillment_data.take())
        .ok_or_else(|| StoreEdgeError::NoFulfillmentData(product_id.to_owned()))?;
    serde_json::from_str(&blob).map_err(|source| StoreEdgeError::MalformedFulfillmentData {
        product: product_id.to_owned(),
        source,
    })
}

/// Selects the files a `store download` delivers: the product's main package
/// (the file whose package identity equals the first component of the
/// `PackageFamilyName`, `url_gen.py:138`) plus exactly one file per remaining
/// dependency package.
///
/// The per-package pick follows `alt-app-installer`'s `select_latest`
/// (`url_gen.py:47-75`, scoring in [`score`]): prefer files with the
/// requested architecture over `neutral` ones and both over the rest, then
/// package files with favorable extensions, then the newest modification
/// date, then the largest version tuple. Dependencies are picked for the main
/// package's effective architecture (its `arch`, except that a `neutral`
/// main package selects dependencies by the host's architecture,
/// `url_gen.py:114`).
///
/// Returns the main package first, then the dependencies in package identity
/// (lexicographic) order.
pub fn select_download_set(
    files: &[Fe3File],
    package_family_name: &str,
    arch: &str,
) -> Vec<Fe3File> {
    let main_identity = package_family_name
        .split('_')
        .next()
        .unwrap_or(package_family_name);
    let grouped: BTreeMap<&str, Vec<&Fe3File>> =
        files.iter().fold(BTreeMap::new(), |mut groups, file| {
            groups
                .entry(file.package_identity_name.as_str())
                .or_default()
                .push(file);
            groups
        });

    let main_arch = grouped
        .get(main_identity)
        .and_then(|group| pick_latest(group, arch))
        .and_then(|main| identity_arch(&main.installer_specific_identifier))
        .filter(|main_arch| *main_arch != "neutral")
        .unwrap_or(arch);

    let mut selection = Vec::new();
    if let Some(main) = grouped
        .get(main_identity)
        .and_then(|group| pick_latest(group, arch))
    {
        selection.push(main.clone());
    }
    for (identity, group) in &grouped {
        if *identity == main_identity {
            continue;
        }
        if let Some(dependency) = pick_latest(group, main_arch) {
            selection.push(dependency.clone());
        }
    }
    selection
}

/// Picks the best file of one package identity group; see
/// [`select_download_set`] for the scoring.
fn pick_latest<'a>(group: &[&'a Fe3File], arch: &str) -> Option<&'a Fe3File> {
    let mut candidates: Vec<&Fe3File> = group
        .iter()
        .copied()
        .filter(|file| matches!(identity_arch(&file.installer_specific_identifier), Some(file_arch) if file_arch == arch || file_arch == "neutral"))
        .collect();
    if candidates.is_empty() {
        candidates = group.to_vec();
    }
    candidates
        .into_iter()
        .max_by(|a, b| score(a, arch).cmp(&score(b, arch)))
}

/// Comparison key of one file; higher is better (see `select_latest`,
/// `url_gen.py:50-66`).
fn score(file: &Fe3File, arch: &str) -> (u8, u8, String, Vec<u64>) {
    let file_arch = identity_arch(&file.installer_specific_identifier);
    let arch_score = match file_arch {
        Some(file_arch) if file_arch == arch => 2,
        Some("neutral") => 1,
        _ => 0,
    };
    // Favorable types of `select_latest`; every file FE3 reports as a
    // package file is a package (`msix`/`msixbundle`/`appx`/`appxbundle`),
    // but keep the check for shape parity.
    let type_score = if matches!(
        file_extension(&file.file_name).as_deref(),
        Some("appx" | "msix" | "msixbundle" | "appxbundle")
    ) {
        1
    } else {
        0
    };
    (
        arch_score,
        type_score,
        file.modified.clone(),
        version_tuple(&file.installer_specific_identifier),
    )
}

/// Architecture component of an `InstallerSpecificIdentifier`
/// (`Name_version_arch__publisher_hash`).
fn identity_arch(identity: &str) -> Option<&str> {
    identity.split('_').nth(2)
}

/// Numeric version tuple of an `InstallerSpecificIdentifier`'s version
/// component (e.g. `14.0.33519.0`).
fn version_tuple(identity: &str) -> Vec<u64> {
    identity
        .split('_')
        .nth(1)
        .map(|version| {
            version
                .split('.')
                .map(|part| part.parse().unwrap_or(0))
                .collect()
        })
        .unwrap_or_default()
}

fn file_extension(file_name: &str) -> Option<String> {
    let (_, extension) = file_name.rsplit_once('.')?;
    Some(extension.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(identity_name: &str, identity: &str, file_name: &str, modified: &str) -> Fe3File {
        Fe3File {
            file_name: file_name.to_owned(),
            installer_specific_identifier: identity.to_owned(),
            package_identity_name: identity_name.to_owned(),
            update_id: "u".to_owned(),
            revision_number: "1".to_owned(),
            size: 1,
            digest: "d".to_owned(),
            digest_algorithm: "SHA1".to_owned(),
            modified: modified.to_owned(),
        }
    }

    #[test]
    fn product_id_comes_from_last_path_segment() {
        assert_eq!(
            product_id_from_page("https://apps.microsoft.com/detail/9NBLGGH4NNS1?hl=de&gl=DE"),
            Some("9NBLGGH4NNS1".to_owned())
        );
        assert_eq!(
            product_id_from_page("https://www.microsoft.com/store/productId/9NTXVN5BNVQC"),
            Some("9NTXVN5BNVQC".to_owned())
        );
        assert_eq!(
            product_id_from_page("9NBLGGH4NNS1"),
            Some("9NBLGGH4NNS1".to_owned())
        );
        assert_eq!(product_id_from_page("https://apps.microsoft.com/"), None);
        assert_eq!(product_id_from_page(""), None);
    }

    #[test]
    fn download_set_prefers_main_package_and_matching_arch() {
        let files = vec![
            file(
                "5319275A.WhatsAppDesktop",
                "5319275A.WhatsAppDesktop_2.2639.0.0_neutral_~_cv1g1gvanyjgm",
                "473405aa-5d34-4e7e-b8e2-8afc194a5e66.msixbundle",
                "2026-06-09T11:21:21.1414049Z",
            ),
            file(
                "5319275A.WhatsAppDesktop",
                "5319275A.WhatsAppDesktop_2.2639.100.0_neutral_~_cv1g1gvanyjgm",
                "7fd9017e-c11e-476b-9266-b870073f0914.msixbundle",
                "2026-06-09T11:21:21.1414049Z",
            ),
            file(
                "Microsoft.VCLibs.140.00",
                "Microsoft.VCLibs.140.00_14.0.33519.0_x64__8wekyb3d8bbwe",
                "9683459a-02fa-4bd6-9ae6-af8ddfbeef35.appx",
                "2024-01-29T20:03:05.7437236Z",
            ),
            file(
                "Microsoft.VCLibs.140.00",
                "Microsoft.VCLibs.140.00_14.0.33519.0_arm__8wekyb3d8bbwe",
                "f52e53fc-9664-4720-800e-a637f6f5c125.appx",
                "2024-01-29T20:03:05.7437236Z",
            ),
            file(
                "Microsoft.VCLibs.140.00.UWPDesktop",
                "Microsoft.VCLibs.140.00.UWPDesktop_14.0.33728.0_x64__8wekyb3d8bbwe",
                "8b19ae8f-1fe9-45e1-aed3-1672ee33bf71.appx",
                "2026-05-13T21:44:41.096317Z",
            ),
            file(
                "Microsoft.VCLibs.140.00.UWPDesktop",
                "Microsoft.VCLibs.140.00.UWPDesktop_14.0.33728.0_arm__8wekyb3d8bbwe",
                "2a0007f4-9769-4709-9244-a28f54f70828.appx",
                "2026-05-13T21:44:41.096317Z",
            ),
        ];
        let selected = select_download_set(&files, "5319275A.WhatsAppDesktop_cv1g1gvanyjgm", "x64");
        let names: Vec<&str> = selected
            .iter()
            .map(|file| file.file_name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "7fd9017e-c11e-476b-9266-b870073f0914.msixbundle",
                "9683459a-02fa-4bd6-9ae6-af8ddfbeef35.appx",
                "8b19ae8f-1fe9-45e1-aed3-1672ee33bf71.appx",
            ]
        );
    }

    #[test]
    fn download_set_ties_break_by_newest_date_before_version() {
        let files = vec![
            file(
                "5319275A.WhatsAppDesktop",
                "5319275A.WhatsAppDesktop_2.2639.0.0_neutral_~_cv1g1gvanyjgm",
                "older.msixbundle",
                "2026-06-09T11:21:21.1414049Z",
            ),
            file(
                "5319275A.WhatsAppDesktop",
                "5319275A.WhatsAppDesktop_2.2639.100.0_neutral_~_cv1g1gvanyjgm",
                "newer-date-lower-version.msixbundle",
                "2026-06-10T11:21:21Z",
            ),
        ];
        let selected = select_download_set(&files, "5319275A.WhatsAppDesktop_cv1g1gvanyjgm", "x64");
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].file_name, "newer-date-lower-version.msixbundle");
    }
}
