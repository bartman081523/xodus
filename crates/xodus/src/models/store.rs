//! Data models of the Microsoft Store edge product endpoint
//! (`storeedgefd.dsx.mp.microsoft.com`).
//!
//! Documented against live responses (2026-10-12,
//! `scratches/fe3_spike/transcripts/`). The endpoint shape follows the
//! MIT-licensed `alt-app-installer` project (see
//! `resources/fe3/LICENSE.alt-app-installer`); unknown response fields are
//! ignored.

/// Response of `storeedgefd…/v9.0/products/{product_id}`.
///
/// Only the fields the FE3 flow needs are decoded; products that do not ship
/// as UWP/MSIX Store packages carry no `Payload` at all.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct StoreProductDetailsResponse {
    #[serde(rename = "Payload")]
    pub payload: Option<StoreProductPayload>,
}

/// `Payload` of the product details response.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct StoreProductPayload {
    #[serde(rename = "Skus")]
    pub skus: Vec<StoreProductSku>,
}

/// One purchasable variant of a Store product.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct StoreProductSku {
    /// JSON-encoded fulfillment blob (`FulfillmentData` arrives double
    /// encoded, as a string inside the JSON); decoding is the caller's job.
    #[serde(rename = "FulfillmentData")]
    pub fulfillment_data: Option<String>,
}

/// Decoded `FulfillmentData` of a UWP/MSIX Store product.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct FulfillmentData {
    /// Windows Update category id; the category FE3's `SyncUpdates` speaks.
    #[serde(rename = "WuCategoryId")]
    pub wu_category_id: String,
    /// Package family name, e.g. `5319275A.WhatsAppDesktop_cv1g1gvanyjgm`.
    #[serde(rename = "PackageFamilyName")]
    pub package_family_name: String,
}
